use std::sync::Arc;

use jellyfin_providers::lyrics::{LrcLyricParser, LyricFile};
use serde::Serialize;
use serde_json::{Value, json};

/// Remote lyric search result matching Jellyfin's `RemoteLyricInfoDto`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteLyricInfo {
    pub id: String,
    pub provider_name: String,
    pub name: String,
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

    /// Searches providers until the first one returns results.
    #[must_use]
    pub fn search(&self, request: &LyricSearchRequest) -> Vec<RemoteLyricInfo> {
        for provider in self.providers.iter() {
            let results = provider.search(request);
            if !results.is_empty() {
                return results;
            }
        }
        Vec::new()
    }

    /// Resolves a provider-owned lyric id.
    #[must_use]
    pub fn get_lyrics(&self, id: &str) -> Option<LyricFile> {
        let provider_name = id.split('_').next()?;
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.name() == provider_name)?;
        provider.get_lyrics(id)
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
}
