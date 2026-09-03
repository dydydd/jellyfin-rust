use std::io::{self, Cursor, SeekFrom};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use jellyfin_extensions::{
    ComparableStream, StreamCompareError, is_file_identical, is_stream_identical,
};
use tokio::io::{AsyncRead, AsyncSeek, AsyncSeekExt, BufReader, ReadBuf};

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn seekable_streams_with_different_lengths_return_false() {
    let mut first = Cursor::new(vec![1, 2, 3]);
    let mut second = Cursor::new(vec![1, 2, 3, 4]);

    let identical = is_stream_identical(
        ComparableStream::seekable(&mut first),
        ComparableStream::seekable(&mut second),
    )
    .await
    .unwrap();

    assert!(!identical);
}

#[tokio::test]
async fn non_seekable_identical_streams_return_true() {
    let mut first = ChunkedReader::new(&[1, 2, 3, 4], usize::MAX);
    let mut second = ChunkedReader::new(&[1, 2, 3, 4], usize::MAX);

    let identical = is_stream_identical(
        ComparableStream::non_seekable(&mut first),
        ComparableStream::non_seekable(&mut second),
    )
    .await
    .unwrap();

    assert!(identical);
}

#[tokio::test]
async fn non_seekable_different_streams_return_false() {
    let mut first = ChunkedReader::new(&[1, 2, 3, 4], usize::MAX);
    let mut second = ChunkedReader::new(&[1, 2, 9, 4], usize::MAX);

    let identical = is_stream_identical(
        ComparableStream::non_seekable(&mut first),
        ComparableStream::non_seekable(&mut second),
    )
    .await
    .unwrap();

    assert!(!identical);
}

#[tokio::test]
async fn file_comparison_rejects_a_non_seekable_stream() {
    let file = TempFile::new(&[1, 2, 3, 4]).await;
    let mut stream = ChunkedReader::new(&[1, 2, 3, 4], usize::MAX);

    let error = is_file_identical(ComparableStream::non_seekable(&mut stream), file.path())
        .await
        .unwrap_err();

    assert!(matches!(error, StreamCompareError::NonSeekable));
}

#[tokio::test]
async fn file_comparison_uses_start_and_restores_position_on_match() {
    let bytes = [10, 20, 30, 40, 50];
    let file = TempFile::new(&bytes).await;

    let mut direct = Cursor::new(bytes.to_vec());
    assert_file_result_and_position(&mut direct, file.path(), 3, true).await;

    let mut buffered = BufReader::new(Cursor::new(bytes.to_vec()));
    assert_file_result_and_position(&mut buffered, file.path(), 3, true).await;
}

#[tokio::test]
async fn file_comparison_restores_position_on_mismatch() {
    let file = TempFile::new(&[10, 20, 30, 40, 99]).await;

    let mut direct = Cursor::new(vec![10, 20, 30, 40, 50]);
    assert_file_result_and_position(&mut direct, file.path(), 2, false).await;

    let mut buffered = BufReader::new(Cursor::new(vec![10, 20, 30, 40, 50]));
    assert_file_result_and_position(&mut buffered, file.path(), 2, false).await;
}

#[tokio::test]
async fn two_seekable_streams_compare_from_the_start() {
    let bytes = vec![1, 2, 3, 4, 5];
    let mut first = Cursor::new(bytes.clone());
    let mut second = Cursor::new(bytes.clone());
    first.set_position(3);
    second.set_position(1);
    assert!(compare_seekable(&mut first, &mut second).await);

    let mut first = BufReader::new(Cursor::new(bytes.clone()));
    let mut second = BufReader::new(Cursor::new(bytes));
    first.seek(SeekFrom::Start(3)).await.unwrap();
    second.seek(SeekFrom::Start(1)).await.unwrap();
    assert!(compare_seekable(&mut first, &mut second).await);
}

#[tokio::test]
async fn seekable_direct_stream_and_buffered_stream_compare_from_the_start() {
    let bytes = vec![1, 2, 3, 4];
    let mut first = Cursor::new(bytes.clone());
    let mut second = BufReader::new(Cursor::new(bytes.clone()));
    first.set_position(2);
    second.seek(SeekFrom::Start(3)).await.unwrap();
    assert!(compare_seekable(&mut first, &mut second).await);

    let mut first = BufReader::new(Cursor::new(bytes.clone()));
    let mut second = BufReader::new(Cursor::new(bytes));
    first.seek(SeekFrom::Start(2)).await.unwrap();
    second.seek(SeekFrom::Start(3)).await.unwrap();
    assert!(compare_seekable(&mut first, &mut second).await);
}

#[tokio::test]
async fn buffered_stream_paired_with_direct_stream_returns_true() {
    let bytes = vec![1, 2, 3, 4];
    let mut first = BufReader::new(Cursor::new(bytes.clone()));
    let mut second = Cursor::new(bytes.clone());
    assert!(compare_seekable(&mut first, &mut second).await);

    let mut first = BufReader::new(Cursor::new(bytes.clone()));
    let mut second = BufReader::new(Cursor::new(bytes));
    assert!(compare_seekable(&mut first, &mut second).await);
}

#[tokio::test]
async fn two_buffered_seekable_streams_compare_from_the_start() {
    let mut first = BufReader::new(Cursor::new(vec![1, 2, 3, 4]));
    let mut second = BufReader::new(Cursor::new(vec![1, 2, 3, 4]));
    first.seek(SeekFrom::Start(1)).await.unwrap();
    second.seek(SeekFrom::Start(2)).await.unwrap();

    assert!(compare_seekable(&mut first, &mut second).await);
}

#[tokio::test]
async fn non_seekable_short_reads_with_different_chunks_compare_identically() {
    let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    let mut first = ChunkedReader::new(&data, 3);
    let mut second = ChunkedReader::new(&data, 5);

    let identical = is_stream_identical(
        ComparableStream::non_seekable(&mut first),
        ComparableStream::non_seekable(&mut second),
    )
    .await
    .unwrap();

    assert!(identical);
}

#[tokio::test]
async fn non_seekable_short_reads_with_different_lengths_return_false() {
    let mut first = ChunkedReader::new(&[1, 2, 3, 4], 3);
    let mut second = ChunkedReader::new(&[1, 2, 3, 4, 5], 5);

    let identical = is_stream_identical(
        ComparableStream::non_seekable(&mut first),
        ComparableStream::non_seekable(&mut second),
    )
    .await
    .unwrap();

    assert!(!identical);
}

#[tokio::test]
async fn comparison_and_restore_failures_are_both_preserved() {
    let missing_path = unique_temp_path();
    let mut stream = RestoreFailingReader::new(&[1, 2, 3, 4]);
    stream.inner.set_position(2);

    let error = is_file_identical(ComparableStream::seekable(&mut stream), &missing_path)
        .await
        .unwrap_err();

    match error {
        StreamCompareError::CompareAndRestore {
            comparison,
            restore,
        } => {
            assert_eq!(comparison.kind(), io::ErrorKind::NotFound);
            assert_eq!(restore.kind(), io::ErrorKind::Other);
        }
        other => panic!("expected both failures, got {other:?}"),
    }
}

async fn compare_seekable<A, B>(first: &mut A, second: &mut B) -> bool
where
    A: AsyncRead + AsyncSeek + Unpin,
    B: AsyncRead + AsyncSeek + Unpin,
{
    is_stream_identical(
        ComparableStream::seekable(first),
        ComparableStream::seekable(second),
    )
    .await
    .unwrap()
}

async fn assert_file_result_and_position<S>(
    stream: &mut S,
    path: &std::path::Path,
    position: u64,
    expected: bool,
) where
    S: AsyncRead + AsyncSeek + Unpin,
{
    stream.seek(SeekFrom::Start(position)).await.unwrap();
    let actual = is_file_identical(ComparableStream::seekable(&mut *stream), path)
        .await
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(stream.stream_position().await.unwrap(), position);
}

struct ChunkedReader {
    data: Vec<u8>,
    position: usize,
    max_read_size: usize,
}

impl ChunkedReader {
    fn new(data: &[u8], max_read_size: usize) -> Self {
        Self {
            data: data.to_vec(),
            position: 0,
            max_read_size,
        }
    }
}

impl AsyncRead for ChunkedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let available = self.data.len().saturating_sub(self.position);
        let amount = available.min(self.max_read_size).min(buffer.remaining());
        if amount > 0 {
            let end = self.position + amount;
            buffer.put_slice(&self.data[self.position..end]);
            self.position = end;
        }
        Poll::Ready(Ok(()))
    }
}

struct RestoreFailingReader {
    inner: Cursor<Vec<u8>>,
    seek_starts: usize,
}

impl RestoreFailingReader {
    fn new(data: &[u8]) -> Self {
        Self {
            inner: Cursor::new(data.to_vec()),
            seek_starts: 0,
        }
    }
}

impl AsyncRead for RestoreFailingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl AsyncSeek for RestoreFailingReader {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        self.seek_starts += 1;
        if self.seek_starts == 3 {
            return Err(io::Error::other("restore failed"));
        }
        Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.inner).poll_complete(context)
    }
}

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    async fn new(contents: &[u8]) -> Self {
        let path = unique_temp_path();
        tokio::fs::write(&path, contents).await.unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn unique_temp_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "jellyfin-extensions-stream-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}
