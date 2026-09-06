use std::sync::Arc;

use jellyfin_providers::lyrics::{LrcLyricParser, LyricFile};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::{Value, json};

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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LyricSearchRequest {
    pub song_name: Option<String>,
    pub album_name: Option<String>,
    pub artist_names: Vec<String>,
    pub album_artist_names: Vec<String>,
    pub duration_ticks: Option<i64>,
}

/// Remote lyric provider boundary matching Jellyfin's plugin contract.
pub trait LyricProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, request: &LyricSearchRequest) -> Vec<RemoteLyricInfo>;
    fn get_lyrics(&self, id: &str) -> Option<LyricFile>;
}

/// Aggregates remote lyric providers and parses their responses.
#[derive(Clone, Default)]
pub struct LyricManager {
    providers: Arc<Vec<Arc<dyn LyricProvider>>>,
}

impl LyricManager {
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn LyricProvider>>) -> Self {
        Self {
            providers: Arc::new(providers),
        }
    }

    /// Searches every provider and projects parseable responses to the public DTO.
    #[must_use]
    pub fn search(&self, request: &LyricSearchRequest) -> Vec<RemoteLyricInfoDto> {
        let mut projected = Vec::new();
        for provider in self.providers.iter() {
            let provider_id = lyric_provider_id(provider.name());
            for result in provider.search(request) {
                let Some(format) = lyric_format(&result.lyrics.name) else {
                    continue;
                };
                let Some(mut lyrics) = Self::parse_lyrics(format, &result.lyrics.content) else {
                    continue;
                };
                let Some(object) = lyrics.as_object_mut() else {
                    continue;
                };
                object.insert("Metadata".to_owned(), result.metadata);
                projected.push(RemoteLyricInfoDto {
                    id: format!("{provider_id}_{}", result.id),
                    provider_name: result.provider_name,
                    lyrics,
                });
            }
        }
        projected
    }

    /// Resolves a provider-owned lyric id.
    #[must_use]
    pub fn get_lyrics(&self, id: &str) -> Option<LyricFile> {
        let (provider_id, lyric_id) = id.split_once('_')?;
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
    format!("{:x}", Md5::digest(name.to_lowercase().as_bytes()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct TestProvider {
        name: &'static str,
        search_results: Vec<RemoteLyricInfo>,
        requested_ids: Arc<Mutex<Vec<String>>>,
    }

    impl LyricProvider for TestProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn search(&self, _request: &LyricSearchRequest) -> Vec<RemoteLyricInfo> {
            self.search_results.clone()
        }

        fn get_lyrics(&self, id: &str) -> Option<LyricFile> {
            self.requested_ids
                .lock()
                .expect("requested ids")
                .push(id.to_owned());
            Some(LyricFile::new("download.lrc", "[00:01.00]Downloaded"))
        }
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

    #[test]
    fn remote_search_projects_sdk_shape_and_aggregates_providers() {
        let first = Arc::new(TestProvider {
            name: "First Provider",
            search_results: vec![RemoteLyricInfo {
                id: "first-result".to_owned(),
                provider_name: "First Provider".to_owned(),
                metadata: json!({ "Artist": "Remote Artist", "IsSynced": true }),
                lyrics: LyricFile::new("first.lrc", "[00:01.00]First result"),
            }],
            requested_ids: Arc::default(),
        });
        let second = Arc::new(TestProvider {
            name: "Second Provider",
            search_results: vec![
                RemoteLyricInfo {
                    id: "unparseable".to_owned(),
                    provider_name: "Second Provider".to_owned(),
                    metadata: json!({}),
                    lyrics: LyricFile::new("bad.srt", "unsupported"),
                },
                RemoteLyricInfo {
                    id: "second_result_with_underscores".to_owned(),
                    provider_name: "Second Provider".to_owned(),
                    metadata: json!({ "Album": "Remote Album" }),
                    lyrics: LyricFile::new("second.txt", "Second result"),
                },
            ],
            requested_ids: Arc::default(),
        });
        let manager = LyricManager::new(vec![first, second]);

        let results = manager.search(&LyricSearchRequest::default());

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].provider_name, "First Provider");
        assert_eq!(results[0].lyrics["Metadata"]["Artist"], "Remote Artist");
        assert_eq!(results[0].lyrics["Lyrics"][0]["Text"], "First result");
        assert_eq!(results[1].provider_name, "Second Provider");
        assert_eq!(results[1].lyrics["Metadata"]["Album"], "Remote Album");
        assert_eq!(results[1].lyrics["Lyrics"][0]["Text"], "Second result");
        assert_eq!(
            results[0].id,
            "86e6efa43156061e1f9d7b7154349726_first-result"
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
    fn remote_download_routes_by_hashed_provider_and_strips_prefix() {
        let requested_ids = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(TestProvider {
            name: "Download Provider",
            search_results: Vec::new(),
            requested_ids: Arc::clone(&requested_ids),
        });
        let manager = LyricManager::new(vec![provider]);
        let provider_id = lyric_provider_id("Download Provider");

        let downloaded = manager
            .get_lyrics(&format!("{provider_id}_remote_id_with_underscores"))
            .expect("remote lyric");

        assert_eq!(downloaded.name, "download.lrc");
        assert_eq!(
            requested_ids.lock().expect("requested ids").as_slice(),
            ["remote_id_with_underscores"]
        );
        assert!(manager.get_lyrics("unknown_remote-id").is_none());
        assert!(manager.get_lyrics(&provider_id).is_none());
    }
}
