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

use crate::tmdb::{MetadataProviderError, TmdbClient, images_to_remote_images, provider_id};

const TMDB_PROVIDER_NAME: &str = "TheMovieDb";

#[derive(Debug, Error)]
pub enum ItemLookupError {
    #[error("item was not found")]
    NotFound,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Metadata(#[from] MetadataProviderError),
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
    pub async fn remote_search(
        &self,
        kind: &str,
        request: RemoteSearchRequest,
        api_key: &str,
    ) -> Result<Vec<RemoteSearchResult>, ItemLookupError> {
        if api_key.trim().is_empty() {
            return Ok(Vec::new());
        }
        let client = TmdbClient::new(api_key.to_owned());
        let name = request.search_info.name.as_deref().unwrap_or_default();
        if name.trim().is_empty() {
            return Ok(Vec::new());
        }
        let year = request
            .search_info
            .year
            .or(request.search_info.production_year);
        match kind.to_ascii_lowercase().as_str() {
            "movie" | "trailer" | "musicvideo" => Ok(client.search_movie(name, year).await?),
            "series" => Ok(client.search_tv(name, year).await?),
            "person" => Ok(client.search_person(name).await?),
            "boxset" => Ok(client.search_collection(name).await?),
            _ => Ok(Vec::new()),
        }
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
        Ok(vec![ImageProviderInfo {
            name: TMDB_PROVIDER_NAME.to_owned(),
            supported_images,
        }])
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

fn empty_remote_images() -> RemoteImageResult {
    RemoteImageResult {
        images: Vec::new(),
        total_record_count: 0,
        providers: Vec::new(),
    }
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
            .remote_search("Movie", RemoteSearchRequest::default(), "")
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
