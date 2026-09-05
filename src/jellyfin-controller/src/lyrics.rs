use std::sync::Arc;

use jellyfin_providers::lyrics::{LrcLyricParser, LyricFile};
use md5::{Digest, Md5};
use serde::Serialize;
use serde_json::{Value, json};

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
                "Lyrics": content.split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line).trim())
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
