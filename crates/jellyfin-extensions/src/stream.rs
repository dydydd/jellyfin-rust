use std::io;
use std::path::Path;

use thiserror::Error;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, SeekFrom};

const STREAM_COMPARISON_BUFFER_SIZE: usize = 81_920;

trait AsyncReadSeek: AsyncRead + AsyncSeek + Unpin {}

impl<T> AsyncReadSeek for T where T: AsyncRead + AsyncSeek + Unpin + ?Sized {}

enum ComparableStreamInner<'a> {
    NonSeekable(&'a mut (dyn AsyncRead + Unpin)),
    Seekable(&'a mut dyn AsyncReadSeek),
}

/// An async readable stream with an explicit optional seek capability.
pub struct ComparableStream<'a> {
    inner: ComparableStreamInner<'a>,
}

impl<'a> ComparableStream<'a> {
    /// Wraps an async reader that must be compared from its current position.
    pub fn non_seekable<R>(stream: &'a mut R) -> Self
    where
        R: AsyncRead + Unpin + 'a,
    {
        Self {
            inner: ComparableStreamInner::NonSeekable(stream),
        }
    }

    /// Wraps an async reader that can be compared from the beginning.
    pub fn seekable<S>(stream: &'a mut S) -> Self
    where
        S: AsyncRead + AsyncSeek + Unpin + 'a,
    {
        Self {
            inner: ComparableStreamInner::Seekable(stream),
        }
    }

    async fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            ComparableStreamInner::NonSeekable(stream) => stream.read(buffer).await,
            ComparableStreamInner::Seekable(stream) => stream.read(buffer).await,
        }
    }

    async fn rewind_if_seekable(&mut self) -> io::Result<()> {
        if let ComparableStreamInner::Seekable(stream) = &mut self.inner {
            stream.seek(SeekFrom::Start(0)).await?;
        }
        Ok(())
    }

    async fn position(&mut self) -> Result<u64, StreamCompareError> {
        match &mut self.inner {
            ComparableStreamInner::NonSeekable(_) => Err(StreamCompareError::NonSeekable),
            ComparableStreamInner::Seekable(stream) => stream
                .stream_position()
                .await
                .map_err(StreamCompareError::Io),
        }
    }

    async fn seek_to(&mut self, position: u64) -> io::Result<()> {
        match &mut self.inner {
            ComparableStreamInner::NonSeekable(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "stream does not support seeking",
            )),
            ComparableStreamInner::Seekable(stream) => {
                stream.seek(SeekFrom::Start(position)).await?;
                Ok(())
            }
        }
    }
}

/// Failure while comparing streams or restoring a file-comparison stream.
#[derive(Debug, Error)]
pub enum StreamCompareError {
    /// The supplied file path was empty.
    #[error("comparison file path is required")]
    EmptyPath,
    /// File comparison requires a seekable stream.
    #[error("stream must support seeking for file comparison")]
    NonSeekable,
    /// An asynchronous file, read, or seek operation failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Comparison succeeded, but restoring the original position failed.
    #[error("failed to restore the stream position: {0}")]
    Restore(#[source] io::Error),
    /// Both comparison and restoration failed.
    #[error("stream comparison failed ({comparison}) and position restoration failed ({restore})")]
    CompareAndRestore {
        comparison: io::Error,
        restore: io::Error,
    },
}

/// Compares two streams byte-for-byte.
///
/// Seekable streams are reset to the beginning. Non-seekable streams are read
/// from their current positions. Positions are not restored.
///
/// # Errors
///
/// Returns [`StreamCompareError::Io`] when a read or seek fails.
pub async fn is_stream_identical(
    mut first: ComparableStream<'_>,
    mut second: ComparableStream<'_>,
) -> Result<bool, StreamCompareError> {
    first.rewind_if_seekable().await?;
    second.rewind_if_seekable().await?;
    compare_current(&mut first, &mut second)
        .await
        .map_err(StreamCompareError::Io)
}

/// Compares a seekable stream with a file from the beginning.
///
/// The stream's original position is restored after a match, mismatch, or
/// comparison failure.
///
/// # Errors
///
/// Returns a typed error for empty paths, non-seekable streams, I/O failures,
/// restoration failures, or simultaneous comparison and restoration failures.
pub async fn is_file_identical(
    mut stream: ComparableStream<'_>,
    path: impl AsRef<Path>,
) -> Result<bool, StreamCompareError> {
    let path = path.as_ref();
    if path.as_os_str().is_empty() {
        return Err(StreamCompareError::EmptyPath);
    }

    let original_position = stream.position().await?;
    let comparison = async {
        stream.seek_to(0).await?;
        let mut file = File::open(path).await?;
        let mut file = ComparableStream::non_seekable(&mut file);
        compare_current(&mut stream, &mut file).await
    }
    .await;
    let restoration = stream.seek_to(original_position).await;

    match (comparison, restoration) {
        (Ok(identical), Ok(())) => Ok(identical),
        (Err(comparison), Ok(())) => Err(StreamCompareError::Io(comparison)),
        (Ok(_), Err(restore)) => Err(StreamCompareError::Restore(restore)),
        (Err(comparison), Err(restore)) => Err(StreamCompareError::CompareAndRestore {
            comparison,
            restore,
        }),
    }
}

async fn compare_current(
    first: &mut ComparableStream<'_>,
    second: &mut ComparableStream<'_>,
) -> io::Result<bool> {
    let mut first_buffer = vec![0; STREAM_COMPARISON_BUFFER_SIZE];
    let mut second_buffer = vec![0; STREAM_COMPARISON_BUFFER_SIZE];

    loop {
        let first_read = read_chunk(first, &mut first_buffer).await?;
        let second_read = read_chunk(second, &mut second_buffer).await?;
        if first_read != second_read {
            return Ok(false);
        }
        if first_read == 0 {
            return Ok(true);
        }
        if first_buffer[..first_read] != second_buffer[..second_read] {
            return Ok(false);
        }
    }
}

async fn read_chunk(stream: &mut ComparableStream<'_>, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = stream.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}
