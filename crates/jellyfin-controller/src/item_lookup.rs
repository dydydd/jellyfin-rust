use std::collections::HashMap;

use jellyfin_data::{BaseItemError, BaseItemRepository};
use jellyfin_model::{
    ExternalIdInfo, ImageProviderInfo, ImageType, RemoteImageResult, RemoteSearchResult,
    order_by_language_descending,
};
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::google_books::GoogleBooksClient;
use crate::music_brainz::MusicBrainzClient;
use crate::tmdb::{MetadataProviderError, TmdbClient, images_to_remote_images, provider_id};
use crate::tv_maze::{TvMazeClient, TvMazeProviderError};

const TMDB_PROVIDER_NAME: &str = "TheMovieDb";
const TV_MAZE_PROVIDER_NAME: &str = "TVMaze";

#[derive(Debug, Error)]
pub enum ItemLookupError {
    #[error("item was not found")]
    NotFound,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Metadata(#[from] MetadataProviderError),
    #[error(transparent)]
    GoogleBooks(#[from] crate::google_books::GoogleBooksProviderError),
    #[error(transparent)]
    TvMaze(#[from] TvMazeProviderError),
    #[error(transparent)]
    MusicBrainz(#[from] crate::music_brainz::MusicBrainzProviderError),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSearchInfo {
    pub name: Option<String>,
    pub year: Option<i32>,
    pub production_year: Option<i32>,
    pub provider_ids: HashMap<String, String>,
    pub metadata_language: Option<String>,
    pub metadata_country_code: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct RemoteSearchRequest {
    pub search_info: RemoteSearchInfo,
    pub item_id: Option<Uuid>,
    pub search_provider_name: Option<String>,
    pub include_disabled_providers: bool,
}

/// Resolves persisted items against the registered metadata providers.
#[derive(Clone)]
pub struct ItemLookupService {
    items: BaseItemRepository,
}

impl ItemLookupService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            items: BaseItemRepository::new(database),
        }
    }

    /// Returns every registered external identifier supported by an item.
    ///
    /// # Errors
    ///
    /// Returns [`ItemLookupError::NotFound`] for an unknown item or the
    /// corresponding `PostgreSQL` persistence error.
    pub async fn external_id_infos(
        &self,
        item_id: Uuid,
    ) -> Result<Vec<ExternalIdInfo>, ItemLookupError> {
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        Ok(jellyfin_providers::external_id::external_id_infos(
            &item.item_type,
        ))
    }

    /// Searches TMDB for remote matches matching the requested item kind.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the TMDB request fails or no key is
    /// configured.
    #[allow(clippy::too_many_lines)]
    pub async fn remote_search(
        &self,
        kind: &str,
        request: RemoteSearchRequest,
        api_key: &str,
        metadata_options: &jellyfin_model::MetadataOptions,
    ) -> Result<Vec<RemoteSearchResult>, ItemLookupError> {
        let name = request.search_info.name.as_deref().unwrap_or_default();
        if name.trim().is_empty() {
            return Ok(Vec::new());
        }
        let year = request
            .search_info
            .year
            .or(request.search_info.production_year);
        let selected_provider = request.search_provider_name.as_deref().map(str::to_owned);
        let results: Result<Vec<RemoteSearchResult>, ItemLookupError> =
            match kind.to_ascii_lowercase().as_str() {
                "movie" | "trailer" | "musicvideo" => {
                    if api_key.trim().is_empty()
                        || provider_disabled(
                            metadata_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        return Ok(Vec::new());
                    }
                    Ok(TmdbClient::new(api_key.to_owned())
                        .search_movie(name, year)
                        .await?)
                }
                "series" => {
                    if api_key.trim().is_empty()
                        || provider_disabled(
                            metadata_options,
                            TMDB_PROVIDER_NAME,
                            request.include_disabled_providers,
                            selected_provider.as_deref(),
                        )
                    {
                        return Ok(Vec::new());
                    }
                    let mut results = Vec::new();
                    if !provider_disabled(
                        metadata_options,
                        TV_MAZE_PROVIDER_NAME,
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        results.extend(TvMazeClient::new().search(name).await?);
                    }
                    results.extend(
                        TmdbClient::new(api_key.to_owned())
                            .search_tv(name, year)
                            .await?,
                    );
                    Ok(results)
                }
                "person" => {
                    if api_key.trim().is_empty() {
                        return Ok(Vec::new());
                    }
                    Ok(TmdbClient::new(api_key.to_owned())
                        .search_person(name)
                        .await?)
                }
                "boxset" => {
                    if api_key.trim().is_empty() {
                        return Ok(Vec::new());
                    }
                    Ok(TmdbClient::new(api_key.to_owned())
                        .search_collection(name)
                        .await?)
                }
                "book" => {
                    if api_key.trim().is_empty() {
                        return Ok(Vec::new());
                    }
                    Ok(GoogleBooksClient::new().search(name, year).await?)
                }
                "musicartist" => {
                    if provider_disabled(
                        metadata_options,
                        "MusicBrainz",
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        return Ok(Vec::new());
                    }
                    Ok(MusicBrainzClient::new().search_artists(name).await?)
                }
                "musicalbum" => {
                    if provider_disabled(
                        metadata_options,
                        "MusicBrainz",
                        request.include_disabled_providers,
                        selected_provider.as_deref(),
                    ) {
                        return Ok(Vec::new());
                    }
                    Ok(MusicBrainzClient::new().search_release_groups(name).await?)
                }
                _ => Ok(Vec::new()),
            };
        Ok(sort_remote_search_results(results?, metadata_options))
    }

    /// Lists remote images offered by the item's configured TMDB provider.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item or a provider error when TMDB
    /// cannot be reached.
    #[allow(clippy::too_many_arguments)]
    pub async fn remote_images(
        &self,
        item_id: Uuid,
        image_type: Option<ImageType>,
        provider_name: Option<&str>,
        include_all_languages: bool,
        start_index: usize,
        limit: Option<usize>,
        api_key: &str,
    ) -> Result<RemoteImageResult, ItemLookupError> {
        if api_key.trim().is_empty() {
            return Ok(empty_remote_images());
        }
        if provider_name.is_some_and(|name| !name.eq_ignore_ascii_case(TMDB_PROVIDER_NAME)) {
            return Ok(empty_remote_images());
        }
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        let Some(tmdb_id) =
            provider_id(item.data.as_ref(), "Tmdb").and_then(|id| id.parse::<i64>().ok())
        else {
            return Ok(empty_remote_images());
        };

        let client = TmdbClient::new(api_key.to_owned());
        let images = match item.item_type.as_str() {
            "Movie" | "MusicVideo" | "Trailer" => client.movie_images(tmdb_id).await?,
            "Series" => client.tv_images(tmdb_id).await?,
            "Person" => client.person_images(tmdb_id).await?,
            _ => return Ok(empty_remote_images()),
        };
        let mut images = images_to_remote_images(images, include_all_languages);
        if let Some(image_type) = image_type {
            images.retain(|image| image.image_type == image_type);
        }
        let total_record_count = i32::try_from(images.len()).unwrap_or(i32::MAX);
        let mut images = order_by_language_descending(images, Some("en"));
        if start_index > 0 {
            images = images.into_iter().skip(start_index).collect();
        }
        if let Some(limit) = limit {
            images.truncate(limit);
        }
        Ok(RemoteImageResult {
            images,
            total_record_count,
            providers: vec![TMDB_PROVIDER_NAME.to_owned()],
        })
    }

    /// Returns image providers available for the item when a TMDB key exists.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item.
    pub async fn remote_image_providers(
        &self,
        item_id: Uuid,
        api_key: &str,
        metadata_options: &jellyfin_model::MetadataOptions,
    ) -> Result<Vec<ImageProviderInfo>, ItemLookupError> {
        if api_key.trim().is_empty() {
            return Ok(Vec::new());
        }
        let item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        let supported_images = match item.item_type.as_str() {
            "Movie" | "MusicVideo" | "Trailer" | "Series" => {
                vec![ImageType::Primary, ImageType::Backdrop, ImageType::Logo]
            }
            "Person" => vec![ImageType::Profile],
            _ => return Ok(Vec::new()),
        };
        Ok([TMDB_PROVIDER_NAME, "TV Maze", "TheAudioDB"]
            .into_iter()
            .filter(|name| {
                !metadata_options
                    .disabled_image_fetchers
                    .iter()
                    .any(|disabled| disabled.eq_ignore_ascii_case(name))
            })
            .filter(|name| {
                metadata_options.image_fetcher_order.is_empty()
                    || metadata_options
                        .image_fetcher_order
                        .iter()
                        .any(|ordered| ordered.eq_ignore_ascii_case(name))
            })
            .map(|name| ImageProviderInfo {
                name: name.to_owned(),
                supported_images: supported_images.clone(),
            })
            .collect())
    }

    /// Applies remote-search provider identifiers to a persisted item.
    ///
    /// # Errors
    ///
    /// Returns not-found for a missing item or persistence errors from
    /// `PostgreSQL`.
    pub async fn apply_remote_search(
        &self,
        item_id: Uuid,
        result: RemoteSearchResult,
    ) -> Result<(), ItemLookupError> {
        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(ItemLookupError::NotFound)?;
        if let Some(item_type) = identified_item_type(&item.item_type, result.r#type.as_deref()) {
            item.item_type = item_type.to_owned();
        }
        if let Some(name) = result
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        if let Some(production_year) = result.production_year {
            item.production_year = Some(production_year);
        }
        if let Some(premiere_date) = result.premiere_date {
            item.premiere_date = Some(premiere_date);
        }
        if let Some(overview) = result
            .overview
            .as_deref()
            .filter(|overview| !overview.trim().is_empty())
        {
            item.overview = Some(overview.to_owned());
        }
        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(serde_json::Map::default()));
        }
        if let Some(Value::Object(metadata)) = item.data.as_mut() {
            metadata.insert(
                "ProviderIds".to_owned(),
                serde_json::to_value(result.provider_ids)
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::default())),
            );
            metadata.remove("provider_ids");
        }
        self.items.update(item).await?;
        Ok(())
    }
}

fn sort_remote_search_results(
    mut results: Vec<RemoteSearchResult>,
    options: &jellyfin_model::MetadataOptions,
) -> Vec<RemoteSearchResult> {
    results.sort_by_key(|result| {
        configured_provider_order(
            &options.metadata_fetcher_order,
            result.search_provider_name.as_deref(),
        )
    });
    results
}

fn empty_remote_images() -> RemoteImageResult {
    RemoteImageResult {
        images: Vec::new(),
        total_record_count: 0,
        providers: Vec::new(),
    }
}

fn provider_disabled(
    options: &jellyfin_model::MetadataOptions,
    provider_name: &str,
    include_disabled: bool,
    selected_provider: Option<&str>,
) -> bool {
    if let Some(selected_provider) = selected_provider {
        return !selected_provider.eq_ignore_ascii_case(provider_name);
    }
    !include_disabled
        && options
            .disabled_metadata_fetchers
            .iter()
            .any(|name| name.eq_ignore_ascii_case(provider_name))
}

fn configured_provider_order(order: &[String], provider_name: Option<&str>) -> usize {
    provider_name
        .and_then(|name| {
            order
                .iter()
                .position(|configured| configured.eq_ignore_ascii_case(name))
        })
        .unwrap_or(usize::MAX)
}

fn identified_item_type<'a>(current: &str, identified: Option<&'a str>) -> Option<&'a str> {
    match identified {
        Some(identified @ ("Movie" | "Series")) if current != identified => {
            matches!(current, "Video" | "Movie" | "Series").then_some(identified)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn remote_search_without_api_key_returns_empty() {
        let service = ItemLookupService::new(DatabaseConnection::Disconnected);
        let results = service
            .remote_search(
                "Movie",
                RemoteSearchRequest::default(),
                "",
                &jellyfin_model::MetadataOptions::default(),
            )
            .await
            .expect("empty result");

        assert!(results.is_empty());
    }

    #[test]
    fn remote_search_request_parses_official_body_shape() {
        let request: RemoteSearchRequest = serde_json::from_value(json!({
            "SearchInfo": {
                "Name": "Fallen",
                "ProviderIds": { "Imdb": "tt0119094" },
                "Year": 1998
            },
            "ItemId": "00000000-0000-0000-0000-000000000000",
            "SearchProviderName": "TheMovieDb",
            "IncludeDisabledProviders": true
        }))
        .expect("remote search request");

        assert_eq!(request.search_info.name.as_deref(), Some("Fallen"));
        assert_eq!(request.search_info.year, Some(1998));
        assert_eq!(request.search_info.provider_ids["Imdb"], "tt0119094");
        assert_eq!(request.search_provider_name.as_deref(), Some("TheMovieDb"));
    }

    #[test]
    fn provider_disabled_honors_config_and_search_provider_selection() {
        let options = jellyfin_model::MetadataOptions {
            disabled_metadata_fetchers: vec!["TVMaze".to_owned()],
            ..Default::default()
        };
        assert!(provider_disabled(&options, "TVMaze", false, None));
        assert!(!provider_disabled(&options, "TVMaze", true, None));
        assert!(provider_disabled(
            &options,
            "TheMovieDb",
            false,
            Some("TVMaze")
        ));
        assert!(!provider_disabled(
            &options,
            "TVMaze",
            false,
            Some("TVMaze")
        ));
    }

    #[test]
    fn search_results_follow_configured_provider_order() {
        let options = jellyfin_model::MetadataOptions {
            metadata_fetcher_order: vec!["TVMaze".to_owned(), "TheMovieDb".to_owned()],
            ..Default::default()
        };
        let tmdb = RemoteSearchResult {
            search_provider_name: Some("TheMovieDb".to_owned()),
            ..RemoteSearchResult::default()
        };
        let tv_maze = RemoteSearchResult {
            search_provider_name: Some("TVMaze".to_owned()),
            ..RemoteSearchResult::default()
        };
        let results = sort_remote_search_results(vec![tmdb, tv_maze], &options);
        assert_eq!(
            results
                .iter()
                .map(|result| result.search_provider_name.as_deref())
                .collect::<Vec<_>>(),
            [Some("TVMaze"), Some("TheMovieDb")]
        );
    }

    #[test]
    fn identified_item_type_only_upgrades_video_items_to_movie_or_series() {
        assert_eq!(identified_item_type("Video", Some("Movie")), Some("Movie"));
        assert_eq!(
            identified_item_type("Video", Some("Series")),
            Some("Series")
        );
        assert_eq!(identified_item_type("Movie", Some("Movie")), None);
        assert_eq!(identified_item_type("Audio", Some("Movie")), None);
        assert_eq!(identified_item_type("Video", None), None);
        assert_eq!(identified_item_type("Video", Some("BoxSet")), None);
    }
}
