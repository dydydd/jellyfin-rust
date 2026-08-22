use std::sync::Arc;

use jellyfin_model::RemoteSubtitleInfo;

/// Provider lookup values for remote subtitle search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubtitleSearchRequest {
    pub language: String,
    pub is_perfect_match: bool,
    pub name: Option<String>,
    pub series_name: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub media_type: String,
}

/// A subtitle payload returned by a remote provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleResponse {
    pub format: String,
    pub language: String,
    pub content: Vec<u8>,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
}

/// Remote subtitle provider boundary matching Jellyfin's plugin contract.
pub trait SubtitleProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_media_types(&self) -> &[&str];
    fn search(&self, request: &SubtitleSearchRequest) -> Vec<RemoteSubtitleInfo>;
    fn get_subtitles(&self, id: &str) -> Option<SubtitleResponse>;
}

/// Aggregates remote subtitle providers in Jellyfin's official order.
#[derive(Clone, Default)]
pub struct SubtitleManager {
    providers: Arc<Vec<Arc<dyn SubtitleProvider>>>,
}

impl SubtitleManager {
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn SubtitleProvider>>) -> Self {
        Self {
            providers: Arc::new(providers),
        }
    }

    /// Searches providers until the first one returns results.
    #[must_use]
    pub fn search(&self, request: &SubtitleSearchRequest) -> Vec<RemoteSubtitleInfo> {
        for provider in self.providers.iter() {
            if !provider.supported_media_types().contains(&request.media_type.as_str()) {
                continue;
            }
            let results = provider.search(request);
            if !results.is_empty() {
                return results;
            }
        }
        Vec::new()
    }

    /// Resolves a provider-owned subtitle id.
    #[must_use]
    pub fn get_subtitles(&self, id: &str) -> Option<SubtitleResponse> {
        let provider_name = id.split('_').next()?;
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.name() == provider_name)?;
        provider.get_subtitles(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MovieProvider;

    impl SubtitleProvider for MovieProvider {
        fn name(&self) -> &str {
            "MovieSubtitles"
        }

        fn supported_media_types(&self) -> &[&str] {
            &["Movie"]
        }

        fn search(&self, request: &SubtitleSearchRequest) -> Vec<RemoteSubtitleInfo> {
            vec![RemoteSubtitleInfo {
                id: Some(format!("MovieSubtitles_{}", request.language)),
                provider_name: Some("MovieSubtitles".to_owned()),
                ..RemoteSubtitleInfo::default()
            }]
        }

        fn get_subtitles(&self, id: &str) -> Option<SubtitleResponse> {
            (id == "MovieSubtitles_eng").then(|| SubtitleResponse {
                format: "srt".to_owned(),
                language: "eng".to_owned(),
                content: b"1\n00:00:01,000 --> 00:00:02,000\nHello".to_vec(),
                is_forced: false,
                is_hearing_impaired: false,
            })
        }
    }

    #[test]
    fn searches_only_supported_media_types() {
        let manager = SubtitleManager::new(vec![Arc::new(MovieProvider)]);
        let movie = SubtitleSearchRequest {
            language: "eng".to_owned(),
            media_type: "Movie".to_owned(),
            ..SubtitleSearchRequest::default()
        };
        assert_eq!(manager.search(&movie).len(), 1);

        let episode = SubtitleSearchRequest {
            language: "eng".to_owned(),
            media_type: "Episode".to_owned(),
            ..SubtitleSearchRequest::default()
        };
        assert!(manager.search(&episode).is_empty());
    }

    #[test]
    fn resolves_provider_owned_subtitle_ids() {
        let manager = SubtitleManager::new(vec![Arc::new(MovieProvider)]);
        assert!(manager.get_subtitles("MovieSubtitles_eng").is_some());
        assert!(manager.get_subtitles("Unknown_id").is_none());
    }
}
