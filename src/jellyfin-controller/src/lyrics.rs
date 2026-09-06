use std::{error::Error, fmt::Write as _, future::Future, pin::Pin, sync::Arc, time::Duration};

use futures_util::{StreamExt, stream::FuturesUnordered};
use jellyfin_providers::lyrics::{LrcLyricParser, LyricFile};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::{Value, json};

const MAX_CONCURRENT_LYRIC_SEARCHES: usize = 4;
const LYRIC_PROVIDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Decodes uploaded or local lyric bytes with the BOM detection used by
/// Jellyfin's `StreamReader`, falling back to lossy UTF-8 when no BOM exists.
#[must_use]
pub fn decode_lyric_bytes(bytes: &[u8]) -> String {
    if let Some(content) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(content).into_owned();
    }
    // UTF-32LE starts with the UTF-16LE BOM, so detect the longer mark first.
    if let Some(content) = bytes.strip_prefix(&[0xFF, 0xFE, 0x00, 0x00]) {
        return decode_utf32_lossy(content, true);
    }
    if let Some(content) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        return decode_utf32_lossy(content, false);
    }
    if let Some(content) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16_lossy(content, true);
    }
    if let Some(content) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16_lossy(content, false);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16_lossy(bytes: &[u8], little_endian: bool) -> String {
    let (chunks, remainder) = bytes.as_chunks::<2>();
    let code_units = chunks
        .iter()
        .map(|chunk| {
            if little_endian {
                u16::from_le_bytes(*chunk)
            } else {
                u16::from_be_bytes(*chunk)
            }
        })
        .collect::<Vec<_>>();
    let mut decoded = String::from_utf16_lossy(&code_units);
    if !remainder.is_empty() {
        decoded.push(char::REPLACEMENT_CHARACTER);
    }
    decoded
}

fn decode_utf32_lossy(bytes: &[u8], little_endian: bool) -> String {
    let (chunks, remainder) = bytes.as_chunks::<4>();
    let mut decoded = chunks
        .iter()
        .map(|chunk| {
            let code_point = if little_endian {
                u32::from_le_bytes(*chunk)
            } else {
                u32::from_be_bytes(*chunk)
            };
            char::from_u32(code_point).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect::<String>();
    if !remainder.is_empty() {
        decoded.push(char::REPLACEMENT_CHARACTER);
    }
    decoded
}

/// Raw remote lyric search result returned by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteLyricInfo {
    pub id: String,
    pub provider_name: String,
    pub metadata: Value,
    pub lyrics: LyricFile,
}

/// Remote lyric search result matching Jellyfin's `RemoteLyricInfoDto`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteLyricInfoDto {
    pub id: String,
    pub provider_name: String,
    pub lyrics: Value,
}

/// Provider lookup values used by remote lyric search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricSearchRequest {
    pub media_path: Option<String>,
    pub song_name: Option<String>,
    pub album_name: Option<String>,
    pub artist_names: Vec<String>,
    pub album_artist_names: Vec<String>,
    pub duration_ticks: Option<i64>,
    pub search_all_providers: bool,
    pub disabled_lyric_fetchers: Vec<String>,
    pub lyric_fetcher_order: Vec<String>,
    pub is_automated: bool,
}

impl Default for LyricSearchRequest {
    fn default() -> Self {
        Self {
            media_path: None,
            song_name: None,
            album_name: None,
            artist_names: Vec::new(),
            album_artist_names: Vec::new(),
            duration_ticks: None,
            search_all_providers: true,
            disabled_lyric_fetchers: Vec::new(),
            lyric_fetcher_order: Vec::new(),
            is_automated: false,
        }
    }
}

pub type LyricProviderError = Box<dyn Error + Send + Sync>;
pub type LyricProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, LyricProviderError>> + Send + 'a>>;

/// Remote lyric provider boundary matching Jellyfin's plugin contract.
pub trait LyricProvider: Send + Sync {
    fn name(&self) -> &str;
    fn order(&self) -> i32 {
        0
    }
    fn search<'a>(
        &'a self,
        request: &'a LyricSearchRequest,
    ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>>;
    fn get_lyrics(&self, id: &str) -> Option<LyricFile>;
}

/// Aggregates remote lyric providers and parses their responses.
#[derive(Clone)]
pub struct LyricManager {
    providers: Arc<Vec<Arc<dyn LyricProvider>>>,
    max_concurrent_searches: usize,
    provider_timeout: Duration,
}

impl Default for LyricManager {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl LyricManager {
    #[must_use]
    pub fn new(mut providers: Vec<Arc<dyn LyricProvider>>) -> Self {
        providers.sort_by_key(|provider| provider.order());
        Self {
            providers: Arc::new(providers),
            max_concurrent_searches: MAX_CONCURRENT_LYRIC_SEARCHES,
            provider_timeout: LYRIC_PROVIDER_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_search_limits(
        providers: Vec<Arc<dyn LyricProvider>>,
        max_concurrent_searches: usize,
        provider_timeout: Duration,
    ) -> Self {
        let mut manager = Self::new(providers);
        manager.max_concurrent_searches = max_concurrent_searches.max(1);
        manager.provider_timeout = provider_timeout;
        manager
    }

    /// Returns configured provider names in provider execution order.
    #[must_use]
    pub fn provider_names(&self) -> impl Iterator<Item = &str> {
        self.providers.iter().map(|provider| provider.name())
    }

    /// Searches configured providers with bounded concurrency while preserving official order.
    pub async fn search(&self, request: &LyricSearchRequest) -> Vec<RemoteLyricInfoDto> {
        let providers = self.selected_providers(request);
        if !request.search_all_providers {
            for provider in providers {
                let results = self.search_provider(provider, request).await;
                if !results.is_empty() {
                    return results;
                }
            }
            return Vec::new();
        }

        let provider_count = providers.len();
        let mut providers = providers.into_iter().enumerate();
        let mut searches = FuturesUnordered::new();
        for (index, provider) in providers.by_ref().take(self.max_concurrent_searches.max(1)) {
            searches.push(self.search_provider_at(index, provider, request));
        }
        let mut completed = (0..provider_count).map(|_| Vec::new()).collect::<Vec<_>>();
        while let Some((index, results)) = searches.next().await {
            completed[index] = results;
            if let Some((index, provider)) = providers.next() {
                searches.push(self.search_provider_at(index, provider, request));
            }
        }
        completed.into_iter().flatten().collect()
    }

    fn selected_providers(&self, request: &LyricSearchRequest) -> Vec<Arc<dyn LyricProvider>> {
        let mut providers = self
            .providers
            .iter()
            .filter(|provider| {
                !request
                    .disabled_lyric_fetchers
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(provider.name()))
            })
            .cloned()
            .collect::<Vec<_>>();
        providers.sort_by_key(|provider| {
            request
                .lyric_fetcher_order
                .iter()
                .position(|name| name == provider.name())
                .unwrap_or(usize::MAX)
        });
        providers
    }

    async fn search_provider(
        &self,
        provider: Arc<dyn LyricProvider>,
        request: &LyricSearchRequest,
    ) -> Vec<RemoteLyricInfoDto> {
        let provider_name = provider.name().to_owned();
        let results = match tokio::time::timeout(self.provider_timeout, provider.search(request))
            .await
        {
            Ok(Ok(results)) => results,
            Ok(Err(error)) => {
                tracing::warn!(provider = %provider_name, %error, "lyric provider search failed");
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!(
                    provider = %provider_name,
                    timeout_seconds = self.provider_timeout.as_secs(),
                    "lyric provider search timed out"
                );
                return Vec::new();
            }
        };
        let provider_id = lyric_provider_id(&provider_name);
        results
            .into_iter()
            .filter_map(|result| {
                let format = lyric_format(&result.lyrics.name)?;
                let mut lyrics = Self::parse_lyrics(format, &result.lyrics.content)?;
                let object = lyrics.as_object_mut()?;
                object.insert(
                    "Metadata".to_owned(),
                    project_lyric_metadata(&result.metadata),
                );
                Some(RemoteLyricInfoDto {
                    id: format!("{provider_id}_{}", result.id),
                    provider_name: result.provider_name,
                    lyrics,
                })
            })
            .collect()
    }

    async fn search_provider_at(
        &self,
        index: usize,
        provider: Arc<dyn LyricProvider>,
        request: &LyricSearchRequest,
    ) -> (usize, Vec<RemoteLyricInfoDto>) {
        (index, self.search_provider(provider, request).await)
    }

    /// Resolves a provider-owned lyric id.
    #[must_use]
    pub fn get_lyrics(&self, id: &str) -> Option<LyricFile> {
        // `string.Split('_', 2)` in the official manager uses the only part as
        // both the provider id and provider-owned id when no separator exists.
        let (provider_id, lyric_id) = id.split_once('_').unwrap_or((id, id));
        let provider = self
            .providers
            .iter()
            .find(|provider| lyric_provider_id(provider.name()) == provider_id)?;
        provider.get_lyrics(lyric_id)
    }

    /// Parses a lyric file using Jellyfin's LRC parser with a TXT fallback.
    #[must_use]
    pub fn parse_lyrics(format: &str, content: &str) -> Option<Value> {
        if (format.eq_ignore_ascii_case("lrc") || format.eq_ignore_ascii_case("elrc"))
            && let Some(parsed) =
                LrcLyricParser.parse_lyrics(&LyricFile::new(format!("lyric.{format}"), content))
        {
            return Some(lyric_dto_to_json(&parsed));
        }
        if ["lrc", "elrc", "txt"]
            .iter()
            .any(|supported| format.eq_ignore_ascii_case(supported))
        {
            return Some(json!({
                "Metadata": {},
                "Lyrics": split_unsynced_lyric_lines(content)
                    .into_iter()
                    .map(str::trim)
                    .map(|text| json!({ "Text": text, "Start": null, "Cues": null }))
                    .collect::<Vec<_>>()
            }));
        }
        None
    }
}

fn lyric_provider_id(name: &str) -> String {
    // Official Jellyfin hashes the invariant-lowercase provider name as
    // `Encoding.Unicode` (UTF-16LE), then formats `new Guid(hash)` as `N`.
    let utf16le = name
        .to_lowercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let mut digest: [u8; 16] = Md5::digest(utf16le).into();

    // Guid(byte[]) treats its first three fields as little-endian, while the
    // remaining eight bytes retain their original order when formatted.
    digest[..4].reverse();
    digest[4..6].reverse();
    digest[6..8].reverse();

    let mut provider_id = String::with_capacity(32);
    for byte in digest {
        write!(&mut provider_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    provider_id
}

fn lyric_format(file_name: &str) -> Option<&str> {
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then_some(extension)
}

fn split_unsynced_lyric_lines(content: &str) -> Vec<&str> {
    let bytes = content.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                lines.push(&content[start..index]);
                index += usize::from(bytes.get(index + 1) == Some(&b'\n')) + 1;
                start = index;
            }
            b'\n' => {
                lines.push(&content[start..index]);
                index += 1;
                start = index;
            }
            _ => index += 1,
        }
    }
    lines.push(&content[start..]);
    lines
}

fn lyric_dto_to_json(parsed: &jellyfin_providers::lyrics::LyricDto) -> Value {
    json!({
        "Metadata": {},
        "Lyrics": parsed.lyrics.iter().map(|line| json!({
            "Text": line.text,
            "Start": line.start,
            "Cues": line.cues.iter().map(|cue| json!({
                "Position": cue.position,
                "EndPosition": cue.end_position,
                "Start": cue.start,
                "End": cue.end
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

fn project_lyric_metadata(metadata: &Value) -> Value {
    let Some(source) = metadata.as_object() else {
        return json!({});
    };
    let mut projected = serde_json::Map::new();
    for (wire_name, camel_name) in [
        ("Artist", "artist"),
        ("Album", "album"),
        ("Title", "title"),
        ("Author", "author"),
        ("By", "by"),
        ("Creator", "creator"),
        ("Version", "version"),
    ] {
        if let Some(value) = metadata_field(source, wire_name, camel_name).and_then(Value::as_str) {
            projected.insert(wire_name.to_owned(), Value::String(value.to_owned()));
        }
    }
    for (wire_name, camel_name) in [("Length", "length"), ("Offset", "offset")] {
        if let Some(value) = metadata_field(source, wire_name, camel_name).and_then(Value::as_i64) {
            projected.insert(wire_name.to_owned(), Value::from(value));
        }
    }
    if let Some(value) = metadata_field(source, "IsSynced", "isSynced").and_then(Value::as_bool) {
        projected.insert("IsSynced".to_owned(), Value::Bool(value));
    }
    Value::Object(projected)
}

fn metadata_field<'a>(
    metadata: &'a serde_json::Map<String, Value>,
    wire_name: &str,
    camel_name: &str,
) -> Option<&'a Value> {
    metadata.get(wire_name).or_else(|| metadata.get(camel_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use tokio::sync::{Semaphore, oneshot};

    #[derive(Debug)]
    struct TestProvider {
        name: &'static str,
        order: i32,
        search_results: Vec<RemoteLyricInfo>,
        search_calls: Arc<AtomicUsize>,
        search_error: bool,
        requested_ids: Arc<Mutex<Vec<String>>>,
    }

    impl LyricProvider for TestProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn order(&self) -> i32 {
            self.order
        }

        fn search<'a>(
            &'a self,
            _request: &'a LyricSearchRequest,
        ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
            self.search_calls.fetch_add(1, Ordering::AcqRel);
            let results = self.search_results.clone();
            let search_error = self.search_error;
            Box::pin(async move {
                if search_error {
                    Err(std::io::Error::other("provider failed").into())
                } else {
                    Ok(results)
                }
            })
        }

        fn get_lyrics(&self, id: &str) -> Option<LyricFile> {
            self.requested_ids
                .lock()
                .expect("requested ids")
                .push(id.to_owned());
            Some(LyricFile::new("download.lrc", "[00:01.00]Downloaded"))
        }
    }

    fn test_provider(name: &'static str, search_results: Vec<RemoteLyricInfo>) -> TestProvider {
        TestProvider {
            name,
            order: 0,
            search_results,
            search_calls: Arc::default(),
            search_error: false,
            requested_ids: Arc::default(),
        }
    }

    fn remote_result(provider_name: &str, id: &str, content: &str) -> RemoteLyricInfo {
        RemoteLyricInfo {
            id: id.to_owned(),
            provider_name: provider_name.to_owned(),
            metadata: json!({}),
            lyrics: LyricFile::new("result.txt", content),
        }
    }

    #[derive(Debug)]
    struct InFlightGuard(Arc<AtomicUsize>);

    impl Drop for InFlightGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }

    #[derive(Debug)]
    struct GatedProvider {
        name: &'static str,
        gate: Arc<Semaphore>,
        started: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl LyricProvider for GatedProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn search<'a>(
            &'a self,
            _request: &'a LyricSearchRequest,
        ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
            let gate = Arc::clone(&self.gate);
            let started = Arc::clone(&self.started);
            let in_flight = Arc::clone(&self.in_flight);
            let peak = Arc::clone(&self.peak);
            let result = remote_result(self.name, self.name, self.name);
            Box::pin(async move {
                started.fetch_add(1, Ordering::AcqRel);
                let current = in_flight.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(current, Ordering::AcqRel);
                let _guard = InFlightGuard(in_flight);
                gate.acquire_owned()
                    .await
                    .map_err(|_| std::io::Error::other("test gate closed"))?
                    .forget();
                Ok(vec![result])
            })
        }

        fn get_lyrics(&self, _id: &str) -> Option<LyricFile> {
            None
        }
    }

    #[derive(Debug)]
    struct CompletionProvider {
        name: &'static str,
        gate: Arc<Semaphore>,
        started: Arc<AtomicBool>,
        completed: Arc<AtomicBool>,
    }

    impl LyricProvider for CompletionProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn search<'a>(
            &'a self,
            _request: &'a LyricSearchRequest,
        ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
            let gate = Arc::clone(&self.gate);
            let started = Arc::clone(&self.started);
            let completed = Arc::clone(&self.completed);
            let result = remote_result(self.name, self.name, self.name);
            Box::pin(async move {
                started.store(true, Ordering::Release);
                gate.acquire_owned()
                    .await
                    .map_err(|_| std::io::Error::other("test gate closed"))?
                    .forget();
                completed.store(true, Ordering::Release);
                Ok(vec![result])
            })
        }

        fn get_lyrics(&self, _id: &str) -> Option<LyricFile> {
            None
        }
    }

    #[derive(Debug)]
    struct PendingProvider {
        calls: Arc<AtomicUsize>,
    }

    impl LyricProvider for PendingProvider {
        fn name(&self) -> &str {
            "Pending Provider"
        }

        fn search<'a>(
            &'a self,
            _request: &'a LyricSearchRequest,
        ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
            let calls = Arc::clone(&self.calls);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::AcqRel);
                std::future::pending::<()>().await;
                Ok(Vec::new())
            })
        }

        fn get_lyrics(&self, _id: &str) -> Option<LyricFile> {
            None
        }
    }

    #[derive(Debug)]
    struct DropSignal(Option<oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[derive(Debug)]
    struct CancelProvider {
        started: Mutex<Option<oneshot::Sender<()>>>,
        dropped: Mutex<Option<oneshot::Sender<()>>>,
    }

    impl LyricProvider for CancelProvider {
        fn name(&self) -> &str {
            "Cancel Provider"
        }

        fn search<'a>(
            &'a self,
            _request: &'a LyricSearchRequest,
        ) -> LyricProviderFuture<'a, Vec<RemoteLyricInfo>> {
            let started = self.started.lock().expect("started sender").take();
            let dropped = self.dropped.lock().expect("dropped sender").take();
            Box::pin(async move {
                let _drop_signal = DropSignal(dropped);
                if let Some(sender) = started {
                    let _ = sender.send(());
                }
                std::future::pending::<()>().await;
                Ok(Vec::new())
            })
        }

        fn get_lyrics(&self, _id: &str) -> Option<LyricFile> {
            None
        }
    }

    async fn wait_for_count(counter: &AtomicUsize, target: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while counter.load(Ordering::Acquire) < target {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("counter must reach target");
    }

    async fn wait_for_flag(flag: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !flag.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("flag must be set");
    }

    #[test]
    fn parses_lrc_into_official_lyric_dto_shape() {
        let parsed = LyricManager::parse_lyrics("lrc", "[00:01.00]Hello\n[00:02.50]World").unwrap();
        assert_eq!(parsed["Lyrics"][0]["Text"], "Hello");
        assert_eq!(parsed["Lyrics"][0]["Start"], 10_000_000);
        assert_eq!(parsed["Lyrics"][1]["Text"], "World");
        assert_eq!(parsed["Lyrics"][1]["Start"], 25_000_000);
    }

    #[test]
    fn rejects_unknown_formats() {
        assert!(LyricManager::parse_lyrics("srt", "1\n00:00:01,000 --> 00:00:02,000").is_none());
    }

    #[test]
    fn decodes_stream_reader_byte_order_marks() {
        assert_eq!(
            decode_lyric_bytes(b"\xEF\xBB\xBFUTF-8 lyrics"),
            "UTF-8 lyrics"
        );

        let text = "UTF-16 歌词";
        let utf16 = text.encode_utf16().collect::<Vec<_>>();
        let mut little_endian = vec![0xFF, 0xFE];
        little_endian.extend(utf16.iter().flat_map(|unit| unit.to_le_bytes()));
        assert_eq!(decode_lyric_bytes(&little_endian), text);

        let mut big_endian = vec![0xFE, 0xFF];
        big_endian.extend(utf16.iter().flat_map(|unit| unit.to_be_bytes()));
        assert_eq!(decode_lyric_bytes(&big_endian), text);

        let utf32 = text.chars().map(u32::from).collect::<Vec<_>>();
        let mut utf32_little_endian = vec![0xFF, 0xFE, 0x00, 0x00];
        utf32_little_endian.extend(utf32.iter().flat_map(|unit| unit.to_le_bytes()));
        assert_eq!(decode_lyric_bytes(&utf32_little_endian), text);

        let mut utf32_big_endian = vec![0x00, 0x00, 0xFE, 0xFF];
        utf32_big_endian.extend(utf32.iter().flat_map(|unit| unit.to_be_bytes()));
        assert_eq!(decode_lyric_bytes(&utf32_big_endian), text);
    }

    #[test]
    fn decoding_without_a_bom_uses_lossy_utf8() {
        assert_eq!(decode_lyric_bytes("无 BOM 歌词".as_bytes()), "无 BOM 歌词");
        assert_eq!(decode_lyric_bytes(b"invalid \xFF utf-8"), "invalid � utf-8");
    }

    #[test]
    fn malformed_utf16_uses_replacement_characters() {
        assert_eq!(decode_lyric_bytes(&[0xFF, 0xFE, b'A', 0, 0x00]), "A�");
        assert_eq!(decode_lyric_bytes(&[0xFE, 0xFF, 0xD8, 0x00]), "�");
    }

    #[test]
    fn malformed_utf32_uses_replacement_characters() {
        assert_eq!(
            decode_lyric_bytes(&[0xFF, 0xFE, 0x00, 0x00, b'A', 0, 0, 0, 0x00]),
            "A�"
        );
        assert_eq!(
            decode_lyric_bytes(&[0x00, 0x00, 0xFE, 0xFF, 0x00, 0x11, 0x00, 0x00]),
            "�"
        );
    }

    #[test]
    fn unsynced_lyrics_split_all_official_line_endings_without_leaking_cr() {
        for format in ["txt", "lrc"] {
            for content in [
                "  First  \nSecond\n",
                "  First  \r\nSecond\r\n",
                "  First  \rSecond\r",
            ] {
                let parsed = LyricManager::parse_lyrics(format, content).expect("plain lyrics");
                let lines = parsed["Lyrics"].as_array().expect("lyric lines");
                assert_eq!(lines.len(), 3, "{format}: {content:?}");
                assert_eq!(lines[0]["Text"], "First", "{format}: {content:?}");
                assert_eq!(lines[1]["Text"], "Second", "{format}: {content:?}");
                assert_eq!(lines[2]["Text"], "", "{format}: {content:?}");
                assert!(
                    lines
                        .iter()
                        .all(|line| !line["Text"].as_str().unwrap().contains('\r'))
                );
            }
        }
    }

    #[tokio::test]
    async fn remote_search_projects_sdk_shape_and_aggregates_providers() {
        let first = Arc::new(test_provider(
            "First Provider",
            vec![RemoteLyricInfo {
                id: "first-result".to_owned(),
                provider_name: "First Provider".to_owned(),
                metadata: json!({ "Artist": "Remote Artist", "IsSynced": true }),
                lyrics: LyricFile::new("first.lrc", "[00:01.00]First result"),
            }],
        ));
        let second = Arc::new(test_provider(
            "Second Provider",
            vec![
                RemoteLyricInfo {
                    id: "unparseable".to_owned(),
                    provider_name: "Second Provider".to_owned(),
                    metadata: json!({}),
                    lyrics: LyricFile::new("bad.srt", "unsupported"),
                },
                RemoteLyricInfo {
                    id: "second_result_with_underscores".to_owned(),
                    provider_name: "Second Provider".to_owned(),
                    metadata: json!({
                        "Album": "Remote Album",
                        "Artist": 42,
                        "Length": "invalid",
                        "IsSynced": "true",
                        "UnknownPluginField": { "nested": true }
                    }),
                    lyrics: LyricFile::new("second.txt", "Second result"),
                },
            ],
        ));
        let manager = LyricManager::new(vec![first, second]);

        let results = manager.search(&LyricSearchRequest::default()).await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider_name, "First Provider");
        assert_eq!(results[0].lyrics["Metadata"]["Artist"], "Remote Artist");
        assert_eq!(results[0].lyrics["Lyrics"][0]["Text"], "First result");
        assert_eq!(results[1].provider_name, "Second Provider");
        assert_eq!(results[1].lyrics["Metadata"]["Album"], "Remote Album");
        assert_eq!(
            results[1].lyrics["Metadata"],
            json!({ "Album": "Remote Album" })
        );
        assert_eq!(results[1].lyrics["Lyrics"][0]["Text"], "Second result");
        assert_eq!(
            results[0].id,
            "191c3b5627f3b041e3390e72eac7d213_first-result"
        );
        assert!(results[1].id.ends_with("_second_result_with_underscores"));

        let wire = serde_json::to_value(&results[0]).expect("remote lyric dto");
        assert_eq!(
            wire.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["Id", "Lyrics", "ProviderName"]
        );
        assert!(wire.get("Name").is_none());
    }

    #[test]
    fn lyric_search_defaults_to_all_providers() {
        let request = LyricSearchRequest::default();
        assert!(request.search_all_providers);
        assert!(!request.is_automated);
        assert!(request.disabled_lyric_fetchers.is_empty());
        assert!(request.lyric_fetcher_order.is_empty());
    }

    #[tokio::test]
    async fn all_provider_search_is_bounded_to_the_configured_window() {
        let gate = Arc::new(Semaphore::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let providers = ["First", "Second", "Third", "Fourth", "Fifth", "Sixth"]
            .into_iter()
            .map(|name| {
                Arc::new(GatedProvider {
                    name,
                    gate: Arc::clone(&gate),
                    started: Arc::clone(&started),
                    in_flight: Arc::clone(&in_flight),
                    peak: Arc::clone(&peak),
                }) as Arc<dyn LyricProvider>
            })
            .collect();
        let manager = LyricManager::with_search_limits(providers, 2, Duration::from_secs(5));
        let search =
            tokio::spawn(async move { manager.search(&LyricSearchRequest::default()).await });

        wait_for_count(started.as_ref(), 2).await;
        assert_eq!(started.load(Ordering::Acquire), 2);
        assert_eq!(in_flight.load(Ordering::Acquire), 2);
        assert_eq!(peak.load(Ordering::Acquire), 2);

        gate.add_permits(6);
        let results = tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .expect("bounded search must finish")
            .expect("bounded search task");
        assert_eq!(results.len(), 6);
        assert_eq!(started.load(Ordering::Acquire), 6);
        assert_eq!(peak.load(Ordering::Acquire), 2);
        assert_eq!(in_flight.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn bounded_search_refills_a_completed_slot_behind_a_slow_provider() {
        let first_gate = Arc::new(Semaphore::new(0));
        let second_gate = Arc::new(Semaphore::new(0));
        let third_gate = Arc::new(Semaphore::new(0));
        let first_started = Arc::new(AtomicBool::new(false));
        let second_started = Arc::new(AtomicBool::new(false));
        let third_started = Arc::new(AtomicBool::new(false));
        let first_completed = Arc::new(AtomicBool::new(false));
        let second_completed = Arc::new(AtomicBool::new(false));
        let third_completed = Arc::new(AtomicBool::new(false));
        let manager = LyricManager::with_search_limits(
            vec![
                Arc::new(CompletionProvider {
                    name: "First",
                    gate: Arc::clone(&first_gate),
                    started: Arc::clone(&first_started),
                    completed: Arc::clone(&first_completed),
                }),
                Arc::new(CompletionProvider {
                    name: "Second",
                    gate: Arc::clone(&second_gate),
                    started: Arc::clone(&second_started),
                    completed: Arc::clone(&second_completed),
                }),
                Arc::new(CompletionProvider {
                    name: "Third",
                    gate: Arc::clone(&third_gate),
                    started: Arc::clone(&third_started),
                    completed: Arc::clone(&third_completed),
                }),
            ],
            2,
            Duration::from_secs(5),
        );
        let search =
            tokio::spawn(async move { manager.search(&LyricSearchRequest::default()).await });

        wait_for_flag(first_started.as_ref()).await;
        wait_for_flag(second_started.as_ref()).await;
        assert!(!third_started.load(Ordering::Acquire));
        second_gate.add_permits(1);
        wait_for_flag(second_completed.as_ref()).await;
        wait_for_flag(third_started.as_ref()).await;
        assert!(!first_completed.load(Ordering::Acquire));

        first_gate.add_permits(1);
        third_gate.add_permits(1);
        let results = tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .expect("work-conserving search must finish")
            .expect("work-conserving search task");
        assert_eq!(
            results
                .iter()
                .map(|result| result.provider_name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second", "Third"]
        );
    }

    #[tokio::test]
    async fn concurrent_completion_keeps_official_provider_order() {
        let first_gate = Arc::new(Semaphore::new(0));
        let second_gate = Arc::new(Semaphore::new(0));
        let first_started = Arc::new(AtomicBool::new(false));
        let second_started = Arc::new(AtomicBool::new(false));
        let first_completed = Arc::new(AtomicBool::new(false));
        let second_completed = Arc::new(AtomicBool::new(false));
        let manager = LyricManager::with_search_limits(
            vec![
                Arc::new(CompletionProvider {
                    name: "First",
                    gate: Arc::clone(&first_gate),
                    started: Arc::clone(&first_started),
                    completed: Arc::clone(&first_completed),
                }),
                Arc::new(CompletionProvider {
                    name: "Second",
                    gate: Arc::clone(&second_gate),
                    started: Arc::clone(&second_started),
                    completed: Arc::clone(&second_completed),
                }),
            ],
            2,
            Duration::from_secs(5),
        );
        let search =
            tokio::spawn(async move { manager.search(&LyricSearchRequest::default()).await });

        wait_for_flag(first_started.as_ref()).await;
        wait_for_flag(second_started.as_ref()).await;
        second_gate.add_permits(1);
        wait_for_flag(second_completed.as_ref()).await;
        assert!(!first_completed.load(Ordering::Acquire));
        first_gate.add_permits(1);

        let results = tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .expect("ordered search must finish")
            .expect("ordered search task");
        assert_eq!(
            results
                .iter()
                .map(|result| result.provider_name.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
    }

    #[tokio::test]
    async fn provider_errors_and_timeouts_are_isolated_from_successes() {
        let mut failed = test_provider("Failed", Vec::new());
        failed.search_error = true;
        let failed_calls = Arc::clone(&failed.search_calls);
        let pending_calls = Arc::new(AtomicUsize::new(0));
        let successful = test_provider(
            "Successful",
            vec![remote_result("Successful", "success", "Found")],
        );
        let successful_calls = Arc::clone(&successful.search_calls);
        let manager = LyricManager::with_search_limits(
            vec![
                Arc::new(failed),
                Arc::new(PendingProvider {
                    calls: Arc::clone(&pending_calls),
                }),
                Arc::new(successful),
            ],
            4,
            Duration::from_millis(10),
        );

        let results = tokio::time::timeout(
            Duration::from_secs(1),
            manager.search(&LyricSearchRequest::default()),
        )
        .await
        .expect("provider timeout must be bounded");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider_name, "Successful");
        assert_eq!(failed_calls.load(Ordering::Acquire), 1);
        assert_eq!(pending_calls.load(Ordering::Acquire), 1);
        assert_eq!(successful_calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn dropping_search_cancels_in_flight_and_never_starts_queued_providers() {
        let (started_sender, started_receiver) = oneshot::channel();
        let (dropped_sender, dropped_receiver) = oneshot::channel();
        let queued = test_provider("Queued", vec![remote_result("Queued", "queued", "Queued")]);
        let queued_calls = Arc::clone(&queued.search_calls);
        let manager = LyricManager::with_search_limits(
            vec![
                Arc::new(CancelProvider {
                    started: Mutex::new(Some(started_sender)),
                    dropped: Mutex::new(Some(dropped_sender)),
                }),
                Arc::new(queued),
            ],
            1,
            Duration::from_secs(60),
        );
        let search =
            tokio::spawn(async move { manager.search(&LyricSearchRequest::default()).await });

        tokio::time::timeout(Duration::from_secs(1), started_receiver)
            .await
            .expect("first provider must start")
            .expect("start signal");
        search.abort();
        assert!(
            search
                .await
                .expect_err("search must be cancelled")
                .is_cancelled()
        );
        tokio::time::timeout(Duration::from_secs(1), dropped_receiver)
            .await
            .expect("in-flight provider future must be dropped")
            .expect("drop signal");
        assert_eq!(queued_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn sequential_search_continues_until_the_first_parseable_result() {
        let empty = test_provider("Empty", Vec::new());
        let empty_calls = Arc::clone(&empty.search_calls);
        let invalid = test_provider(
            "Invalid",
            vec![RemoteLyricInfo {
                id: "invalid".to_owned(),
                provider_name: "Invalid".to_owned(),
                metadata: json!({}),
                lyrics: LyricFile::new("invalid.srt", "unsupported"),
            }],
        );
        let invalid_calls = Arc::clone(&invalid.search_calls);
        let mut failed = test_provider("Failed", Vec::new());
        failed.search_error = true;
        let failed_calls = Arc::clone(&failed.search_calls);
        let successful = test_provider(
            "Successful",
            vec![remote_result("Successful", "success", "Found")],
        );
        let successful_calls = Arc::clone(&successful.search_calls);
        let skipped = test_provider(
            "Skipped",
            vec![remote_result("Skipped", "skipped", "Skipped")],
        );
        let skipped_calls = Arc::clone(&skipped.search_calls);
        let manager = LyricManager::new(vec![
            Arc::new(empty),
            Arc::new(invalid),
            Arc::new(failed),
            Arc::new(successful),
            Arc::new(skipped),
        ]);
        let request = LyricSearchRequest {
            search_all_providers: false,
            ..LyricSearchRequest::default()
        };

        let results = manager.search(&request).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].provider_name, "Successful");
        assert_eq!(empty_calls.load(Ordering::Acquire), 1);
        assert_eq!(invalid_calls.load(Ordering::Acquire), 1);
        assert_eq!(failed_calls.load(Ordering::Acquire), 1);
        assert_eq!(successful_calls.load(Ordering::Acquire), 1);
        assert_eq!(skipped_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn request_filtering_and_order_follow_official_string_comparisons() {
        let implicit = test_provider(
            "Implicit First",
            vec![remote_result("Implicit First", "implicit", "Implicit")],
        );
        let preferred = test_provider(
            "Preferred",
            vec![remote_result("Preferred", "preferred", "Preferred")],
        );
        let disabled = test_provider(
            "Disabled",
            vec![remote_result("Disabled", "disabled", "Disabled")],
        );
        let disabled_calls = Arc::clone(&disabled.search_calls);
        let trailing = test_provider(
            "Trailing",
            vec![remote_result("Trailing", "trailing", "Trailing")],
        );
        let manager = LyricManager::new(vec![
            Arc::new(implicit),
            Arc::new(preferred),
            Arc::new(disabled),
            Arc::new(trailing),
        ]);
        let request = LyricSearchRequest {
            disabled_lyric_fetchers: vec!["dIsAbLeD".to_owned()],
            lyric_fetcher_order: vec!["Preferred".to_owned(), "implicit first".to_owned()],
            ..LyricSearchRequest::default()
        };

        let results = manager.search(&request).await;

        assert_eq!(
            results
                .iter()
                .map(|result| result.provider_name.as_str())
                .collect::<Vec<_>>(),
            ["Preferred", "Implicit First", "Trailing"]
        );
        assert_eq!(disabled_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn provider_ids_match_utf16le_dotnet_guid_n_format() {
        assert_eq!(
            lyric_provider_id("First Provider"),
            "191c3b5627f3b041e3390e72eac7d213"
        );
        assert_eq!(
            lyric_provider_id("FIRST PROVIDER"),
            lyric_provider_id("First Provider")
        );

        let provider_id = lyric_provider_id("First Provider");
        assert_eq!(provider_id.len(), 32);
        assert!(
            provider_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }

    #[test]
    fn provider_names_follow_stable_intrinsic_order() {
        let mut first = test_provider("First Provider", Vec::new());
        first.order = 20;
        let mut second = test_provider("Second Provider", Vec::new());
        second.order = 10;
        let mut same_order = test_provider("Same Order Provider", Vec::new());
        same_order.order = 10;
        let manager = LyricManager::new(vec![
            Arc::new(first),
            Arc::new(second),
            Arc::new(same_order),
        ]);

        assert_eq!(
            manager.provider_names().collect::<Vec<_>>(),
            ["Second Provider", "Same Order Provider", "First Provider"]
        );
    }

    #[test]
    fn lyric_metadata_matches_sdk_types_and_omits_invalid_fields() {
        assert_eq!(
            project_lyric_metadata(&json!({
                "artist": "Artist",
                "Album": "Album",
                "title": "Title",
                "Author": "Author",
                "length": 1_234_567,
                "By": "Editor",
                "offset": -10_000,
                "Creator": "Creator",
                "version": "1.0",
                "isSynced": false,
                "Unknown": "discarded"
            })),
            json!({
                "Artist": "Artist",
                "Album": "Album",
                "Title": "Title",
                "Author": "Author",
                "Length": 1_234_567,
                "By": "Editor",
                "Offset": -10_000,
                "Creator": "Creator",
                "Version": "1.0",
                "IsSynced": false
            })
        );
        assert_eq!(
            project_lyric_metadata(&json!({
                "Artist": 123,
                "Length": 1.5,
                "IsSynced": "false"
            })),
            json!({})
        );
        assert_eq!(project_lyric_metadata(&Value::Null), json!({}));
    }

    #[test]
    fn remote_download_routes_by_hashed_provider_and_strips_prefix() {
        let requested_ids = Arc::new(Mutex::new(Vec::new()));
        let mut provider = test_provider("Download Provider", Vec::new());
        provider.requested_ids = Arc::clone(&requested_ids);
        let provider = Arc::new(provider);
        let manager = LyricManager::new(vec![provider]);
        let provider_id = lyric_provider_id("Download Provider");
        assert_eq!(provider_id, "a14e566951f4669abf58f6555ec9d3d1");

        let downloaded = manager
            .get_lyrics(&format!("{provider_id}_remote_id_with_underscores"))
            .expect("remote lyric");

        assert_eq!(downloaded.name, "download.lrc");
        assert_eq!(
            requested_ids.lock().expect("requested ids").as_slice(),
            ["remote_id_with_underscores"]
        );
        assert!(manager.get_lyrics("unknown_remote-id").is_none());
        assert!(manager.get_lyrics(&provider_id).is_some());
        assert_eq!(
            requested_ids.lock().expect("requested ids").as_slice(),
            ["remote_id_with_underscores", provider_id.as_str()]
        );
        assert!(
            manager
                .get_lyrics(&format!("{}_remote-id", provider_id.to_uppercase()))
                .is_none()
        );
    }
}
