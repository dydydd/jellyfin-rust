#![allow(clippy::cast_possible_truncation)]

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ItemValueError, ItemValueRepository, NewBaseItem, NewPerson, PersonError,
    PersonRepository,
    entities::{base_item, item_value::ItemValueType},
};
use jellyfin_model::{ImageType, ProviderIdMap, RatingType, RemoteImageInfo, RemoteSearchResult};
use jellyfin_providers::tmdb::TmdbUtils;
use jellyfin_providers::tv::{
    EpisodeLookupInfo, EpisodeMetadata, EpisodeMetadataCapability, EpisodeMetadataResult,
    EpisodeMetadataService, EpisodeParentContext, EpisodeRefreshOptions, SeasonContext,
    SeriesContext,
};
use serde::{Deserialize, Deserializer, de::Error as _};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::item_images::ItemImageService;

const TMDB_API_BASE_URL: &str = "https://api.themoviedb.org/3";
const TMDB_PROVIDER_NAME: &str = "TheMovieDb";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum MetadataProviderError {
    #[error("metadata provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("TMDB API returned error: {0}")]
    TmdbApi(String),
    #[error("no TMDB API key configured")]
    NoApiKey,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    Person(#[from] PersonError),
}

/// Minimal TMDB v3 client shared by metadata refresh, remote search, and
/// remote image discovery.
#[derive(Clone)]
pub(crate) struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    language: String,
    image_languages: String,
}

impl TmdbClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, TMDB_API_BASE_URL.to_owned())
    }

    #[must_use]
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::with_base_url_and_locale(api_key, base_url, "en", "US")
    }

    #[must_use]
    pub fn with_locale(
        api_key: impl Into<String>,
        language: impl AsRef<str>,
        country: impl AsRef<str>,
    ) -> Self {
        Self::with_base_url_and_locale(api_key, TMDB_API_BASE_URL.to_owned(), language, country)
    }

    fn with_base_url_and_locale(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        language: impl AsRef<str>,
        country: impl AsRef<str>,
    ) -> Self {
        let language = tmdb_language(language.as_ref(), country.as_ref());
        let image_language = language.split('-').next().unwrap_or("en");
        let image_languages = if image_language.eq_ignore_ascii_case("en") {
            "en,null".to_owned()
        } else {
            format!("{image_language},en,null")
        };
        Self {
            http: http_client(),
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            language,
            image_languages,
        }
    }

    pub(crate) async fn search_movie(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Vec<RemoteSearchResult>, MetadataProviderError> {
        let mut query = vec![
            ("query", name),
            ("language", self.language.as_str()),
            ("include_adult", "false"),
        ];
        let year_param = year.map(|year| year.to_string());
        if let Some(year) = year_param.as_deref() {
            query.push(("year", year));
        }
        let response = self
            .get_json::<TmdbSearchResponse<TmdbSearchMovie>>("/search/movie", &query)
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(movie_search_to_remote_result)
            .collect())
    }

    pub(crate) async fn search_tv(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Vec<RemoteSearchResult>, MetadataProviderError> {
        let mut query = vec![
            ("query", name),
            ("language", self.language.as_str()),
            ("include_adult", "false"),
        ];
        let year_param = year.map(|year| year.to_string());
        if let Some(year) = year_param.as_deref() {
            query.push(("first_air_date_year", year));
        }
        let response = self
            .get_json::<TmdbSearchResponse<TmdbSearchTv>>("/search/tv", &query)
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(tv_search_to_remote_result)
            .collect())
    }

    pub(crate) async fn search_person(
        &self,
        name: &str,
    ) -> Result<Vec<RemoteSearchResult>, MetadataProviderError> {
        let response = self
            .get_json::<TmdbSearchResponse<TmdbSearchPerson>>(
                "/search/person",
                &[
                    ("query", name),
                    ("language", self.language.as_str()),
                    ("include_adult", "false"),
                ],
            )
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(person_search_to_remote_result)
            .collect())
    }

    pub(crate) async fn search_collection(
        &self,
        name: &str,
    ) -> Result<Vec<RemoteSearchResult>, MetadataProviderError> {
        let response = self
            .get_json::<TmdbSearchResponse<TmdbSearchCollection>>(
                "/search/collection",
                &[("query", name), ("language", self.language.as_str())],
            )
            .await?;
        Ok(response
            .results
            .into_iter()
            .map(collection_search_to_remote_result)
            .collect())
    }

    pub(crate) async fn movie_details(
        &self,
        id: i64,
    ) -> Result<TmdbMovieDetails, MetadataProviderError> {
        self.get_json(
            &format!("/movie/{id}"),
            &[
                ("language", self.language.as_str()),
                (
                    "append_to_response",
                    "credits,release_dates,external_ids,videos,images,keywords",
                ),
            ],
        )
        .await
    }

    pub(crate) async fn tv_details(&self, id: i64) -> Result<TmdbTvDetails, MetadataProviderError> {
        self.get_json(
            &format!("/tv/{id}"),
            &[
                ("language", self.language.as_str()),
                (
                    "append_to_response",
                    "credits,content_ratings,external_ids,videos,images,keywords",
                ),
            ],
        )
        .await
    }

    pub(crate) async fn tv_season_details(
        &self,
        id: i64,
        season_number: i32,
    ) -> Result<TmdbTvSeasonDetails, MetadataProviderError> {
        self.get_json(
            &format!("/tv/{id}/season/{season_number}"),
            &[
                ("language", self.language.as_str()),
                ("append_to_response", "credits,external_ids,videos"),
            ],
        )
        .await
    }

    pub(crate) async fn episode_details(
        &self,
        series_id: i64,
        season_number: i32,
        episode_number: i32,
    ) -> Result<TmdbEpisodeDetails, MetadataProviderError> {
        self.get_json(
            &format!("/tv/{series_id}/season/{season_number}/episode/{episode_number}"),
            &[
                ("language", self.language.as_str()),
                ("append_to_response", "credits,external_ids,videos"),
            ],
        )
        .await
    }

    pub(crate) async fn collection_details(
        &self,
        id: i64,
    ) -> Result<TmdbCollectionDetails, MetadataProviderError> {
        self.get_json(
            &format!("/collection/{id}"),
            &[("language", self.language.as_str())],
        )
        .await
    }

    pub(crate) async fn person_details(
        &self,
        id: i64,
    ) -> Result<TmdbPersonDetails, MetadataProviderError> {
        self.get_json(
            &format!("/person/{id}"),
            &[
                ("language", self.language.as_str()),
                ("append_to_response", "external_ids,images"),
            ],
        )
        .await
    }

    pub(crate) async fn movie_images(&self, id: i64) -> Result<TmdbImages, MetadataProviderError> {
        self.get_json(
            &format!("/movie/{id}/images"),
            &[("include_image_language", self.image_languages.as_str())],
        )
        .await
    }

    pub(crate) async fn tv_images(&self, id: i64) -> Result<TmdbImages, MetadataProviderError> {
        self.get_json(
            &format!("/tv/{id}/images"),
            &[("include_image_language", self.image_languages.as_str())],
        )
        .await
    }

    pub(crate) async fn person_images(&self, id: i64) -> Result<TmdbImages, MetadataProviderError> {
        self.get_json(&format!("/person/{id}/images"), &[]).await
    }

    async fn get_json<T>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, MetadataProviderError>
    where
        T: serde::de::DeserializeOwned,
    {
        let key = self.api_key()?;
        let mut query = query.to_vec();
        query.push(("api_key", key));
        let url = format!("{}{path}", self.base_url);
        self.http
            .get(&url)
            .query(&query)
            .send()
            .await
            .map_err(MetadataProviderError::Http)?
            .error_for_status()
            .map_err(MetadataProviderError::Http)?
            .json()
            .await
            .map_err(MetadataProviderError::Http)
    }

    fn api_key(&self) -> Result<&str, MetadataProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(MetadataProviderError::NoApiKey);
        }
        Ok(&self.api_key)
    }
}

fn http_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
    if let Some(proxy) = proxy_from_environment() {
        builder = builder.proxy(proxy);
    }
    builder.build().unwrap_or_else(|error| {
        tracing::error!(%error, "could not configure the TMDB HTTP client");
        reqwest::Client::new()
    })
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_TMDB_PROXY",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok())
    .find(|value| !value.trim().is_empty())
    .and_then(|value| reqwest::Proxy::all(value.trim()).ok())
}

fn movie_search_to_remote_result(mut result: TmdbSearchMovie) -> RemoteSearchResult {
    RemoteSearchResult {
        name: result.title.take().or_else(|| result.original_title.take()),
        r#type: Some("Movie".to_owned()),
        provider_ids: HashMap::from([("Tmdb".to_owned(), result.id.to_string())]),
        production_year: parse_year(result.release_date.as_deref()),
        premiere_date: parse_tmdb_date(result.release_date.as_deref()),
        image_url: TmdbUtils::image_url(Some("w500"), result.poster_path.as_deref()),
        search_provider_name: Some(TMDB_PROVIDER_NAME.to_owned()),
        overview: result.overview,
        ..RemoteSearchResult::default()
    }
}

fn tv_search_to_remote_result(mut result: TmdbSearchTv) -> RemoteSearchResult {
    RemoteSearchResult {
        name: result.name.take().or_else(|| result.original_name.take()),
        r#type: Some("Series".to_owned()),
        provider_ids: HashMap::from([("Tmdb".to_owned(), result.id.to_string())]),
        production_year: parse_year(result.first_air_date.as_deref()),
        premiere_date: parse_tmdb_date(result.first_air_date.as_deref()),
        image_url: TmdbUtils::image_url(Some("w500"), result.poster_path.as_deref()),
        search_provider_name: Some(TMDB_PROVIDER_NAME.to_owned()),
        overview: result.overview,
        ..RemoteSearchResult::default()
    }
}

fn person_search_to_remote_result(result: TmdbSearchPerson) -> RemoteSearchResult {
    RemoteSearchResult {
        name: Some(result.name),
        provider_ids: HashMap::from([("Tmdb".to_owned(), result.id.to_string())]),
        image_url: TmdbUtils::image_url(Some("w185"), result.profile_path.as_deref()),
        search_provider_name: Some(TMDB_PROVIDER_NAME.to_owned()),
        ..RemoteSearchResult::default()
    }
}

fn collection_search_to_remote_result(result: TmdbSearchCollection) -> RemoteSearchResult {
    RemoteSearchResult {
        name: Some(result.name),
        provider_ids: HashMap::from([("Tmdb".to_owned(), result.id.to_string())]),
        image_url: TmdbUtils::image_url(Some("w500"), result.poster_path.as_deref()),
        search_provider_name: Some(TMDB_PROVIDER_NAME.to_owned()),
        ..RemoteSearchResult::default()
    }
}

/// Fetches metadata from TMDB and updates the item.
pub struct TmdbMetadataProvider {
    client: TmdbClient,
    items: std::sync::Arc<BaseItemRepository>,
    values: std::sync::Arc<ItemValueRepository>,
    people: std::sync::Arc<PersonRepository>,
    updates: std::sync::Arc<ItemUpdateRepository>,
    images: Option<std::sync::Arc<ItemImageService>>,
}

impl TmdbMetadataProvider {
    #[must_use]
    pub fn new(
        api_key: impl Into<String>,
        items: std::sync::Arc<BaseItemRepository>,
        values: std::sync::Arc<ItemValueRepository>,
        people: std::sync::Arc<PersonRepository>,
        updates: std::sync::Arc<ItemUpdateRepository>,
        images: Option<std::sync::Arc<ItemImageService>>,
    ) -> Self {
        Self::with_locale(api_key, "en", "US", items, values, people, updates, images)
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn with_locale(
        api_key: impl Into<String>,
        language: impl AsRef<str>,
        country: impl AsRef<str>,
        items: std::sync::Arc<BaseItemRepository>,
        values: std::sync::Arc<ItemValueRepository>,
        people: std::sync::Arc<PersonRepository>,
        updates: std::sync::Arc<ItemUpdateRepository>,
        images: Option<std::sync::Arc<ItemImageService>>,
    ) -> Self {
        Self {
            client: TmdbClient::with_locale(api_key, language, country),
            items,
            values,
            people,
            updates,
            images,
        }
    }

    /// Refreshes metadata for one persisted item.
    ///
    /// # Errors
    ///
    /// Returns a provider error when TMDB cannot resolve or persist metadata.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, MetadataProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };

        if item.item_type == "Episode" {
            return self.refresh_episode(item).await;
        }
        match item.item_type.as_str() {
            "Movie" => self.refresh_movie(&item).await,
            "Series" => self.refresh_series(&item).await,
            "Season" => self.refresh_season_item(&item).await,
            "BoxSet" => self.refresh_box_set(&item).await,
            "Person" => self.refresh_person(&item).await,
            _ => Ok(false),
        }
    }

    /// Refreshes only remote poster/backdrop images for an item.
    ///
    /// # Errors
    ///
    /// Returns a provider error when TMDB cannot resolve or fetch image data.
    pub async fn refresh_images(
        &self,
        item_id: Uuid,
        replace_all: bool,
    ) -> Result<bool, MetadataProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        let (poster, backdrop) = match item.item_type.as_str() {
            "Movie" => {
                let Some(id) = self.resolve_movie_id(&item).await? else {
                    return Ok(false);
                };
                let details = self.client.movie_details(id).await?;
                (
                    details.poster_path.as_deref().map(str::to_owned),
                    details.backdrop_path.as_deref().map(str::to_owned),
                )
            }
            "Series" => {
                let Some(id) = self.resolve_series_id(&item).await? else {
                    return Ok(false);
                };
                let details = self.client.tv_details(id).await?;
                (
                    details.poster_path.as_deref().map(str::to_owned),
                    details.backdrop_path.as_deref().map(str::to_owned),
                )
            }
            "BoxSet" => {
                let Some(id) = self.resolve_box_set_id(&item).await? else {
                    return Ok(false);
                };
                let details = self.client.collection_details(id).await?;
                (
                    details.poster_path.as_deref().map(str::to_owned),
                    details.backdrop_path.as_deref().map(str::to_owned),
                )
            }
            _ => return Ok(false),
        };
        self.save_remote_images(item.id, poster.as_deref(), backdrop.as_deref(), replace_all)
            .await;
        Ok(true)
    }

    async fn refresh_movie(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_movie_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.movie_details(tmdb_id).await?;
        let (poster_path, backdrop_path) = self.apply_movie_metadata(item.id, details).await?;
        self.save_remote_images(
            item.id,
            poster_path.as_deref(),
            backdrop_path.as_deref(),
            false,
        )
        .await;
        Ok(true)
    }

    async fn refresh_series(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_series_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.tv_details(tmdb_id).await?;
        let season_count = details.number_of_seasons;
        let (poster_path, backdrop_path) = self.apply_tv_metadata(item.id, details).await?;
        self.refresh_season_metadata(item.id, tmdb_id, season_count)
            .await?;
        self.save_remote_images(
            item.id,
            poster_path.as_deref(),
            backdrop_path.as_deref(),
            false,
        )
        .await;
        Ok(true)
    }

    async fn refresh_season_item(
        &self,
        item: &base_item::Model,
    ) -> Result<bool, MetadataProviderError> {
        let Some(series) = self.items.parent(item.id).await? else {
            return Ok(false);
        };
        let Some(series_tmdb_id) = self.resolve_series_id(&series).await? else {
            return Ok(false);
        };
        let Some(season_number) = item.index_number else {
            return Ok(false);
        };
        let season = self
            .client
            .tv_season_details(series_tmdb_id, season_number)
            .await?;
        self.apply_season_metadata(series.id, season_number, season)
            .await?;
        Ok(true)
    }

    async fn refresh_box_set(
        &self,
        item: &base_item::Model,
    ) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_box_set_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.collection_details(tmdb_id).await?;
        self.apply_box_set_metadata(item.id, &details).await?;
        self.save_remote_images(
            item.id,
            details.poster_path.as_deref(),
            details.backdrop_path.as_deref(),
            false,
        )
        .await;
        Ok(true)
    }

    async fn refresh_person(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_person_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.person_details(tmdb_id).await?;
        self.apply_person_metadata(item.id, &details).await?;
        self.save_profile_image(item.id, details.profile_path.as_deref())
            .await;
        Ok(true)
    }

    async fn resolve_movie_id(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<i64>, MetadataProviderError> {
        if let Some(id) =
            provider_id(item.data.as_ref(), "Tmdb").and_then(|id| id.parse::<i64>().ok())
        {
            return Ok(Some(id));
        }
        let results = self
            .client
            .search_movie(
                item.name.as_deref().unwrap_or_default(),
                item.production_year,
            )
            .await?;
        Ok(results
            .into_iter()
            .next()
            .and_then(|mut result| result.provider_ids.remove("Tmdb"))
            .and_then(|id| id.parse::<i64>().ok()))
    }

    async fn resolve_series_id(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<i64>, MetadataProviderError> {
        if let Some(id) =
            provider_id(item.data.as_ref(), "Tmdb").and_then(|id| id.parse::<i64>().ok())
        {
            return Ok(Some(id));
        }
        let results = self
            .client
            .search_tv(
                item.name.as_deref().unwrap_or_default(),
                item.production_year,
            )
            .await?;
        Ok(results
            .into_iter()
            .next()
            .and_then(|mut result| result.provider_ids.remove("Tmdb"))
            .and_then(|id| id.parse::<i64>().ok()))
    }

    async fn resolve_box_set_id(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<i64>, MetadataProviderError> {
        for key in ["TmdbCollection", "Tmdb"] {
            if let Some(id) =
                provider_id(item.data.as_ref(), key).and_then(|id| id.parse::<i64>().ok())
            {
                return Ok(Some(id));
            }
        }
        let results = self
            .client
            .search_collection(item.name.as_deref().unwrap_or_default())
            .await?;
        Ok(results
            .into_iter()
            .next()
            .and_then(|mut result| result.provider_ids.remove("Tmdb"))
            .and_then(|id| id.parse::<i64>().ok()))
    }

    async fn resolve_person_id(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<i64>, MetadataProviderError> {
        if let Some(id) =
            provider_id(item.data.as_ref(), "Tmdb").and_then(|id| id.parse::<i64>().ok())
        {
            return Ok(Some(id));
        }
        let results = self
            .client
            .search_person(item.name.as_deref().unwrap_or_default())
            .await?;
        Ok(results
            .into_iter()
            .next()
            .and_then(|mut result| result.provider_ids.remove("Tmdb"))
            .and_then(|id| id.parse::<i64>().ok()))
    }

    async fn apply_movie_metadata(
        &self,
        item_id: Uuid,
        details: TmdbMovieDetails,
    ) -> Result<(Option<String>, Option<String>), MetadataProviderError> {
        let provider_ids = movie_provider_ids(&details);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(into_keyword_names(details.keywords)),
                    genres: Some(into_names(details.genres)),
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;
        for studio in into_names(details.production_companies) {
            self.values
                .link(item_id, ItemValueType::Studios, &studio)
                .await?;
        }

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(title) = details.title.as_deref().filter(|value| !value.is_empty()) {
            item.name = Some(title.to_owned());
            item.sort_name = Some(title.to_owned());
        }
        item.overview = details
            .overview
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        item.official_rating = us_rating(&details.release_dates);
        item.runtime_ticks = details
            .runtime
            .map(|minutes| i64::from(minutes) * 60 * 10_000_000);
        if let Some(premiere_date) = parse_tmdb_date(details.release_date.as_deref()) {
            item.premiere_date = Some(premiere_date);
            item.production_year = Some(premiere_date.year());
        }
        item.data = Some(movie_extra_data(
            item.data.take(),
            details.original_title.as_deref(),
            details.original_language.as_deref(),
            details.tagline.as_deref(),
            details.status.as_deref(),
            details.vote_average,
            &details.production_countries,
            &details.videos,
        ));
        self.items.update(item).await?;

        self.replace_owned_people(item_id, details.credits.cast, details.credits.crew)
            .await?;
        Ok((details.poster_path, details.backdrop_path))
    }

    async fn apply_tv_metadata(
        &self,
        item_id: Uuid,
        details: TmdbTvDetails,
    ) -> Result<(Option<String>, Option<String>), MetadataProviderError> {
        let provider_ids = tv_provider_ids(&details);
        let mut studios = into_names(details.networks);
        for studio in into_names(details.production_companies) {
            if !studios
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&studio))
            {
                studios.push(studio);
            }
        }
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(into_keyword_names(details.keywords)),
                    genres: Some(into_names(details.genres)),
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;
        for studio in &studios {
            self.values
                .link(item_id, ItemValueType::Studios, studio)
                .await?;
        }

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(name) = details.name.as_deref().filter(|value| !value.is_empty()) {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        item.overview = details
            .overview
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        item.official_rating = tv_rating(&details.content_ratings);
        if let Some(premiere_date) = parse_tmdb_date(details.first_air_date.as_deref()) {
            item.premiere_date = Some(premiere_date);
            item.production_year = Some(premiere_date.year());
        }
        item.data = Some(tv_extra_data(
            item.data.take(),
            details.original_name.as_deref(),
            details.original_language.as_deref(),
            details.tagline.as_deref(),
            details.status.as_deref(),
            details.vote_average,
            &details.production_countries,
            &details.videos,
            details.number_of_seasons,
            details.number_of_episodes,
            &studios,
        ));
        self.items.update(item).await?;

        self.replace_owned_people(item_id, details.credits.cast, details.credits.crew)
            .await?;
        Ok((details.poster_path, details.backdrop_path))
    }

    async fn apply_box_set_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbCollectionDetails,
    ) -> Result<(), MetadataProviderError> {
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: None,
                    genres: None,
                    provider_ids: Some(BTreeMap::from([
                        ("Tmdb".to_owned(), details.id.to_string()),
                        ("TmdbCollection".to_owned(), details.id.to_string()),
                    ])),
                },
            )
            .await?;

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(name) = details.name.as_deref().filter(|value| !value.is_empty()) {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        item.overview = details
            .overview
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        self.items.update(item).await?;
        Ok(())
    }

    async fn apply_person_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbPersonDetails,
    ) -> Result<(), MetadataProviderError> {
        let mut provider_ids = BTreeMap::from([("Tmdb".to_owned(), details.id.to_string())]);
        if let Some(imdb_id) = details
            .external_ids
            .imdb_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            provider_ids.insert("Imdb".to_owned(), imdb_id.to_owned());
        }
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: None,
                    genres: None,
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        item.overview = details
            .biography
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        if let Some(birthday) = parse_tmdb_date(details.birthday.as_deref()) {
            item.premiere_date = Some(birthday);
        }
        item.data = Some(person_extra_data(item.data.take(), details));
        self.items.update(item).await?;
        Ok(())
    }

    async fn save_profile_image(&self, item_id: Uuid, profile_path: Option<&str>) {
        let Some(images) = self.images.as_ref() else {
            return;
        };
        let Some(profile_path) = profile_path.filter(|path| !path.is_empty()) else {
            return;
        };
        let Some(image_url) = TmdbUtils::image_url(Some("original"), Some(profile_path)) else {
            return;
        };
        let Some(item) = self.items.get(item_id).await.ok().flatten() else {
            return;
        };
        let existing = images.list(&item).await.ok();
        let has_profile = existing.as_ref().is_some_and(|images| {
            images
                .iter()
                .any(|image| image.image_type == ImageType::Primary)
        });
        if !has_profile
            && let Err(error) = images
                .download_remote_image(item_id, ImageType::Primary, &image_url)
                .await
        {
            tracing::warn!(%error, "TMDB person profile image download failed");
        }
    }

    async fn refresh_episode(&self, item: base_item::Model) -> Result<bool, MetadataProviderError> {
        let parents = self.episode_parents(&item).await?;
        let item_id = item.id;
        let mut episode = episode_metadata_from_item(item);
        let outcome = EpisodeMetadataService::refresh(
            &mut episode,
            EpisodeParentContext {
                series: parents.series.as_ref(),
                season: parents.season.as_ref(),
            },
            EpisodeRefreshOptions {
                replace_data: false,
                metadata_language: Some(self.client.language.as_str()),
                metadata_country_code: None,
            },
            &TmdbEpisodeCapability {
                client: &self.client,
            },
        )
        .await?;
        if outcome.metadata_changed || outcome.provider_returned_metadata {
            self.apply_episode_metadata(item_id, &episode).await?;
        }
        Ok(outcome.provider_returned_metadata)
    }

    async fn episode_parents(
        &self,
        item: &base_item::Model,
    ) -> Result<EpisodeParents, MetadataProviderError> {
        let season = self.items.parent(item.id).await?;
        let series = if let Some(season_item) = &season {
            self.items.parent(season_item.id).await?
        } else {
            None
        };
        let mut series_context = None;
        if let Some(series_item) = series {
            let seasons = self
                .items
                .children(series_item.id)
                .await?
                .into_iter()
                .filter(|item| item.item_type == "Season")
                .map(season_context_from_item)
                .collect::<Vec<_>>();
            series_context = Some(series_context_from_item(series_item, seasons));
        }
        let season_context = season.map(season_context_from_item);
        Ok(EpisodeParents {
            series: series_context,
            season: season_context,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn apply_episode_metadata(
        &self,
        item_id: Uuid,
        episode: &EpisodeMetadata,
    ) -> Result<(), MetadataProviderError> {
        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(name) = episode.name.as_deref().filter(|value| !value.is_empty()) {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        item.overview = episode
            .overview
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        if let Some(index_number) = episode.index_number {
            item.index_number = Some(index_number);
        }
        if let Some(parent_index_number) = episode.parent_index_number {
            item.parent_index_number = Some(parent_index_number);
        }
        if let Some(ticks) = episode.premiere_date
            && let Some(date) = ticks_to_datetime(ticks)
        {
            item.premiere_date = Some(date);
        }
        if let Some(production_year) = episode.production_year {
            item.production_year = Some(production_year);
        }
        if let Some(runtime_ticks) = episode.runtime_ticks {
            item.runtime_ticks = Some(runtime_ticks);
        }
        if let Some(series_id) = episode
            .series_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            item.series_id = Some(series_id);
        }
        if let Some(season_id) = episode
            .season_id
            .as_deref()
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            item.season_id = Some(season_id);
        }
        if let Some(series_presentation_unique_key) = episode
            .series_presentation_unique_key
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            item.series_presentation_unique_key = Some(series_presentation_unique_key.to_owned());
        }

        let mut data = metadata_object(item.data.take());
        upsert_i32(&mut data, "IndexNumberEnd", episode.index_number_end);
        upsert_i32(
            &mut data,
            "AirsAfterSeasonNumber",
            episode.airs_after_season_number,
        );
        upsert_i32(
            &mut data,
            "AirsBeforeSeasonNumber",
            episode.airs_before_season_number,
        );
        upsert_i32(
            &mut data,
            "AirsBeforeEpisodeNumber",
            episode.airs_before_episode_number,
        );
        if let Some(rating) = episode.community_rating {
            data.insert("CommunityRating".to_owned(), json!(rating));
        }
        if !episode.provider_ids.is_empty() {
            data.insert(
                "ProviderIds".to_owned(),
                serde_json::to_value(&episode.provider_ids).unwrap_or_default(),
            );
        }
        if !episode.remote_trailers.is_empty() {
            data.insert(
                "RemoteTrailers".to_owned(),
                Value::Array(
                    episode
                        .remote_trailers
                        .iter()
                        .map(|url| json!({ "Name": "", "Url": url }))
                        .collect(),
                ),
            );
        }
        if let Some(series_name) = episode.series_name.as_deref() {
            data.insert("SeriesName".to_owned(), json!(series_name));
        }
        if let Some(season_name) = episode.season_name.as_deref() {
            data.insert("SeasonName".to_owned(), json!(season_name));
        }
        item.data = Some(Value::Object(data));
        self.items.update(item).await?;
        Ok(())
    }

    async fn refresh_season_metadata(
        &self,
        series_id: Uuid,
        tmdb_series_id: i64,
        season_count: Option<i32>,
    ) -> Result<(), MetadataProviderError> {
        let Some(season_count) = season_count else {
            return Ok(());
        };
        for season_number in 1..=season_count {
            let Ok(season) = self
                .client
                .tv_season_details(tmdb_series_id, season_number)
                .await
            else {
                continue;
            };
            self.apply_season_metadata(series_id, season_number, season)
                .await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn apply_season_metadata(
        &self,
        series_id: Uuid,
        season_number: i32,
        mut season: TmdbTvSeasonDetails,
    ) -> Result<(), MetadataProviderError> {
        let season_items = self
            .items
            .children(series_id)
            .await?
            .into_iter()
            .filter(|item| item.item_type == "Season" && item.index_number == Some(season_number))
            .collect::<Vec<_>>();
        let season_item_count = season_items.len();
        for (season_item_index, mut season_item) in season_items.into_iter().enumerate() {
            let mut season_changed = false;
            if let Some(name) = season.name.as_deref().filter(|name| !name.is_empty())
                && season_item.name.as_deref() != Some(name)
            {
                season_item.name = Some(name.to_owned());
                season_item.sort_name = Some(name.to_owned());
                season_changed = true;
            }
            if let Some(overview) = season.overview.as_deref().filter(|value| !value.is_empty())
                && season_item.overview.as_deref() != Some(overview)
            {
                season_item.overview = Some(overview.to_owned());
                season_changed = true;
            }
            if let Some(premiere_date) = parse_tmdb_date(season.air_date.as_deref())
                && season_item.premiere_date != Some(premiere_date)
            {
                season_item.premiere_date = Some(premiere_date);
                season_item.production_year = Some(premiere_date.year());
                season_changed = true;
            }
            if let Some(tvdb_id) = season
                .external_ids
                .tvdb_id
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                let mut data = metadata_object(season_item.data.take());
                let mut provider_ids = take_provider_ids(&mut data);
                provider_ids.insert("Tvdb".to_owned(), tvdb_id.to_owned());
                data.insert(
                    "ProviderIds".to_owned(),
                    serde_json::to_value(&provider_ids).unwrap_or_default(),
                );
                season_item.data = Some(Value::Object(data));
                season_changed = true;
            }
            if season_changed {
                season_item = self.items.update(season_item).await?;
            }
            if let Some(images) = &self.images
                && let Some(url) =
                    TmdbUtils::image_url(Some("original"), season.poster_path.as_deref())
            {
                let existing = images.list(&season_item).await.ok();
                let has_primary = existing.as_ref().is_some_and(|images| {
                    images
                        .iter()
                        .any(|image| image.image_type == ImageType::Primary)
                });
                if !has_primary
                    && let Err(error) = images
                        .download_remote_image(season_item.id, ImageType::Primary, &url)
                        .await
                {
                    tracing::warn!(%error, "TMDB season primary image download failed");
                }
            }
            let mut episodes = self
                .items
                .children(season_item.id)
                .await?
                .into_iter()
                .map(Some)
                .collect::<Vec<_>>();
            for remote in &season.episodes {
                let Some(episode_slot) = episodes.iter_mut().find(|episode| {
                    episode
                        .as_ref()
                        .is_some_and(|item| item.index_number == Some(remote.episode_number))
                }) else {
                    continue;
                };
                let mut episode = episode_slot
                    .take()
                    .expect("matching episode slot contains a model");
                if let Some(name) = remote.name.as_deref().filter(|name| !name.is_empty()) {
                    episode.name = Some(name.to_owned());
                    episode.sort_name = Some(name.to_owned());
                }
                if let Some(overview) = remote.overview.as_deref().filter(|value| !value.is_empty())
                {
                    episode.overview = Some(overview.to_owned());
                }
                if let Some(premiere_date) = parse_tmdb_date(remote.air_date.as_deref()) {
                    episode.premiere_date = Some(premiere_date);
                }
                if let Some(runtime) = remote.runtime {
                    episode.runtime_ticks = Some(i64::from(runtime) * 60 * 10_000_000);
                }
                episode.data = Some(episode_data_with_rating(
                    episode.data.take(),
                    &remote.id.to_string(),
                    remote.vote_average,
                    remote.vote_count,
                ));
                if let Some(images) = &self.images
                    && let Some(url) =
                        TmdbUtils::image_url(Some("original"), remote.still_path.as_deref())
                {
                    let existing = images.list(&episode).await.ok();
                    let has_primary = existing.as_ref().is_some_and(|images| {
                        images
                            .iter()
                            .any(|image| image.image_type == ImageType::Primary)
                    });
                    if !has_primary
                        && let Err(error) = images
                            .download_remote_image(episode.id, ImageType::Primary, &url)
                            .await
                    {
                        tracing::warn!(%error, "TMDB episode primary image download failed");
                    }
                }
                *episode_slot = Some(self.items.update(episode).await?);
            }
            if season_item_index + 1 == season_item_count {
                self.replace_owned_people(
                    season_item.id,
                    std::mem::take(&mut season.credits.cast),
                    std::mem::take(&mut season.credits.crew),
                )
                .await?;
            } else {
                self.replace_people(season_item.id, &season.credits.cast, &season.credits.crew)
                    .await?;
            }
        }
        Ok(())
    }

    async fn replace_people(
        &self,
        item_id: Uuid,
        cast: &[TmdbCast],
        crew: &[TmdbCrew],
    ) -> Result<(), MetadataProviderError> {
        self.people.clear_credits(item_id).await?;
        let mut order = 0;
        for actor in cast {
            let person = self
                .people
                .link(
                    item_id,
                    NewPerson {
                        name: actor.name.clone(),
                        provider_ids: json!({ "Tmdb": actor.id.to_string() }),
                    },
                    "Actor",
                    actor.character.as_deref(),
                    Some(actor.order),
                    order,
                )
                .await?;
            self.ensure_person_image(
                person.id,
                &person.name,
                actor.id,
                actor.profile_path.as_deref(),
            )
            .await?;
            order += 1;
        }
        for member in crew {
            let Some(person_type) =
                crew_person_type(member.department.as_deref(), member.job.as_deref())
            else {
                continue;
            };
            let person = self
                .people
                .link(
                    item_id,
                    NewPerson {
                        name: member.name.clone(),
                        provider_ids: json!({ "Tmdb": member.id.to_string() }),
                    },
                    person_type,
                    member.job.as_deref(),
                    Some(order),
                    order,
                )
                .await?;
            self.ensure_person_image(
                person.id,
                &person.name,
                member.id,
                member.profile_path.as_deref(),
            )
            .await?;
            order += 1;
        }
        Ok(())
    }

    async fn replace_owned_people(
        &self,
        item_id: Uuid,
        cast: Vec<TmdbCast>,
        crew: Vec<TmdbCrew>,
    ) -> Result<(), MetadataProviderError> {
        self.people.clear_credits(item_id).await?;
        let mut order = 0;
        for actor in cast {
            let person = self
                .people
                .link(
                    item_id,
                    NewPerson {
                        name: actor.name,
                        provider_ids: json!({ "Tmdb": actor.id.to_string() }),
                    },
                    "Actor",
                    actor.character.as_deref(),
                    Some(actor.order),
                    order,
                )
                .await?;
            self.ensure_person_image(
                person.id,
                &person.name,
                actor.id,
                actor.profile_path.as_deref(),
            )
            .await?;
            order += 1;
        }
        for member in crew {
            let Some(person_type) =
                crew_person_type(member.department.as_deref(), member.job.as_deref())
            else {
                continue;
            };
            let person = self
                .people
                .link(
                    item_id,
                    NewPerson {
                        name: member.name,
                        provider_ids: json!({ "Tmdb": member.id.to_string() }),
                    },
                    person_type,
                    member.job.as_deref(),
                    Some(order),
                    order,
                )
                .await?;
            self.ensure_person_image(
                person.id,
                &person.name,
                member.id,
                member.profile_path.as_deref(),
            )
            .await?;
            order += 1;
        }
        Ok(())
    }

    async fn ensure_person_image(
        &self,
        person_id: Uuid,
        name: &str,
        tmdb_person_id: i64,
        profile_path: Option<&str>,
    ) -> Result<(), MetadataProviderError> {
        let Some(images) = self.images.as_ref() else {
            return Ok(());
        };
        let Some(profile_path) = profile_path.filter(|path| !path.is_empty()) else {
            return Ok(());
        };
        let Some(image_url) = TmdbUtils::image_url(Some("original"), Some(profile_path)) else {
            return Ok(());
        };
        let person_item = if let Some(item) = self.items.get(person_id).await? {
            item
        } else {
            let mut item = NewBaseItem::new(person_id, "Person");
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
            item.is_virtual_item = true;
            item.data = Some(json!({
                "SourceType": "Library",
                "ProviderIds": { "Tmdb": tmdb_person_id.to_string() }
            }));
            self.items.create(item).await?
        };
        if images.list(&person_item).await.is_ok_and(|images| {
            images
                .iter()
                .any(|image| image.image_type == ImageType::Primary)
        }) {
            return Ok(());
        }
        if let Err(error) = images
            .download_remote_image(person_item.id, ImageType::Primary, &image_url)
            .await
        {
            tracing::warn!(name, %error, "person profile image download failed");
        }
        Ok(())
    }

    async fn save_remote_images(
        &self,
        item_id: Uuid,
        poster_path: Option<&str>,
        backdrop_path: Option<&str>,
        replace_all: bool,
    ) {
        let Some(images) = &self.images else {
            return;
        };
        let Some(item) = self.items.get(item_id).await.ok().flatten() else {
            return;
        };
        if replace_all {
            let _ = images.delete(item_id, ImageType::Primary, 0).await;
            let _ = images.delete(item_id, ImageType::Backdrop, 0).await;
        }
        let existing = images.list(&item).await.ok();
        let has_primary = existing.as_ref().is_some_and(|images| {
            images
                .iter()
                .any(|image| image.image_type == ImageType::Primary)
        });
        let has_backdrop = existing.as_ref().is_some_and(|images| {
            images
                .iter()
                .any(|image| image.image_type == ImageType::Backdrop)
        });
        if !has_primary
            && let Some(url) = TmdbUtils::image_url(Some("original"), poster_path)
            && let Err(error) = images
                .download_remote_image(item_id, ImageType::Primary, &url)
                .await
        {
            tracing::warn!(%error, "TMDB primary image download failed");
        }
        if !has_backdrop
            && let Some(url) = TmdbUtils::image_url(Some("original"), backdrop_path)
            && let Err(error) = images
                .download_remote_image(item_id, ImageType::Backdrop, &url)
                .await
        {
            tracing::warn!(%error, "TMDB backdrop download failed");
        }
    }
}

fn tmdb_language(language: &str, country: &str) -> String {
    let language = language.trim().replace('_', "-");
    if language.is_empty() {
        return "en-US".to_owned();
    }
    if language.contains('-') || country.trim().is_empty() {
        return language;
    }
    format!("{}-{}", language, country.trim().to_ascii_uppercase())
}

struct EpisodeParents {
    series: Option<SeriesContext>,
    season: Option<SeasonContext>,
}

struct TmdbEpisodeCapability<'a> {
    client: &'a TmdbClient,
}

impl EpisodeMetadataCapability for TmdbEpisodeCapability<'_> {
    type Error = MetadataProviderError;

    async fn get_metadata(
        &self,
        lookup: &EpisodeLookupInfo,
    ) -> Result<Option<EpisodeMetadataResult>, Self::Error> {
        if lookup.is_missing_episode || lookup.index_number.is_none() {
            return Ok(None);
        }
        let Some(first) =
            fetch_episode_details(self.client, lookup, lookup.index_number.unwrap()).await?
        else {
            return Ok(None);
        };
        let details = if let Some(end) = lookup.index_number_end {
            let mut combined = first;
            let mut number = combined.episode_number + 1;
            while number <= end {
                if let Some(next) = fetch_episode_details(self.client, lookup, number).await? {
                    combine_episode_details(&mut combined, &next);
                }
                number += 1;
            }
            combined
        } else {
            first
        };
        Ok(Some(EpisodeMetadataResult {
            item: episode_metadata_from_details(details, lookup),
            has_metadata: true,
        }))
    }
}

async fn fetch_episode_details(
    client: &TmdbClient,
    lookup: &EpisodeLookupInfo,
    episode_number: i32,
) -> Result<Option<TmdbEpisodeDetails>, MetadataProviderError> {
    let Some(series_id) = lookup
        .tmdb_series_id
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return Ok(None);
    };
    let season_number = lookup.parent_index_number.unwrap_or(1);
    Ok(Some(
        client
            .episode_details(series_id, season_number, episode_number)
            .await?,
    ))
}

#[allow(clippy::format_push_string)]
fn combine_episode_details(target: &mut TmdbEpisodeDetails, next: &TmdbEpisodeDetails) {
    if let Some(name) = next.name.as_deref().filter(|value| !value.is_empty()) {
        target
            .name
            .get_or_insert_with(String::new)
            .push_str(&format!(" / {name}"));
    }
    if let Some(overview) = next.overview.as_deref().filter(|value| !value.is_empty()) {
        target
            .overview
            .get_or_insert_with(String::new)
            .push_str(&format!(" / {overview}"));
    }
}

fn episode_metadata_from_item(item: base_item::Model) -> EpisodeMetadata {
    let mut data = metadata_object(item.data);
    let provider_ids = take_provider_ids(&mut data);
    let series_name = take_data_string(&mut data, "SeriesName");
    let season_name = take_data_string(&mut data, "SeasonName");
    let series_id = take_data_string(&mut data, "SeriesId");
    let season_id = take_data_string(&mut data, "SeasonId");
    EpisodeMetadata {
        name: item.name,
        overview: item.overview,
        index_number: item.index_number,
        parent_index_number: item.parent_index_number,
        index_number_end: data_i32(&data, "IndexNumberEnd"),
        airs_after_season_number: data_i32(&data, "AirsAfterSeasonNumber"),
        airs_before_season_number: data_i32(&data, "AirsBeforeSeasonNumber"),
        airs_before_episode_number: data_i32(&data, "AirsBeforeEpisodeNumber"),
        provider_ids,
        series_name,
        season_name,
        series_id: series_id.or_else(|| item.series_id.map(|id| id.simple().to_string())),
        season_id: season_id.or_else(|| item.season_id.map(|id| id.simple().to_string())),
        series_presentation_unique_key: item.series_presentation_unique_key,
        is_missing_episode: item.is_virtual_item,
        ..EpisodeMetadata::default()
    }
}

fn episode_metadata_from_details(
    details: TmdbEpisodeDetails,
    lookup: &EpisodeLookupInfo,
) -> EpisodeMetadata {
    let mut provider_ids = ProviderIdMap::new();
    provider_ids.insert("Tmdb".to_owned(), details.id.to_string());
    if let Some(tvdb_id) = details
        .external_ids
        .tvdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        provider_ids.insert("Tvdb".to_owned(), tvdb_id.to_owned());
    }
    if let Some(imdb_id) = details
        .external_ids
        .imdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        provider_ids.insert("Imdb".to_owned(), imdb_id.to_owned());
    }
    if let Some(tvrage_id) = details
        .external_ids
        .tvrage_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        provider_ids.insert("TvRage".to_owned(), tvrage_id.to_owned());
    }
    let premiere_date = parse_tmdb_date(details.air_date.as_deref());
    let remote_trailers = trailer_urls(&details.videos);
    EpisodeMetadata {
        name: details.name,
        overview: details.overview,
        index_number: Some(details.episode_number),
        index_number_end: lookup.index_number_end,
        parent_index_number: Some(details.season_number),
        provider_ids,
        premiere_date: premiere_date.map(datetime_to_ticks),
        production_year: premiere_date.map(|date| date.year()),
        community_rating: (details.vote_average > 0.0).then_some(details.vote_average as f32),
        runtime_ticks: details
            .runtime
            .map(|minutes| i64::from(minutes) * 60 * 10_000_000),
        remote_trailers,
        ..EpisodeMetadata::default()
    }
}

fn season_context_from_item(item: base_item::Model) -> SeasonContext {
    let mut data = metadata_object(item.data);
    SeasonContext {
        id: item.id.simple().to_string(),
        name: item.name.unwrap_or_default(),
        index_number: item.index_number,
        provider_ids: take_provider_ids(&mut data),
    }
}

fn series_context_from_item(item: base_item::Model, seasons: Vec<SeasonContext>) -> SeriesContext {
    let mut data = metadata_object(item.data);
    let display_order = take_data_string(&mut data, "DisplayOrder");
    let provider_ids = take_provider_ids(&mut data);
    SeriesContext {
        id: item.id.simple().to_string(),
        name: item.name.unwrap_or_default(),
        presentation_unique_key: item
            .series_presentation_unique_key
            .or(item.presentation_unique_key),
        display_order,
        provider_ids,
        seasons,
    }
}

fn take_provider_ids(data: &mut serde_json::Map<String, Value>) -> ProviderIdMap {
    let Some(Value::Object(provider_ids)) = data.remove("ProviderIds") else {
        return ProviderIdMap::new();
    };
    provider_ids
        .into_iter()
        .filter_map(|(key, value)| match value {
            Value::String(value) if !value.is_empty() => Some((key, value)),
            _ => None,
        })
        .collect()
}

fn person_extra_data(existing: Option<Value>, details: &TmdbPersonDetails) -> Value {
    let mut object = metadata_object(existing);
    set_string(&mut object, "HomePageUrl", details.homepage.as_deref());
    set_string(&mut object, "EndDate", details.deathday.as_deref());
    if let Some(place) = details
        .place_of_birth
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("ProductionLocations".to_owned(), json!([place]));
    }
    let mut provider_ids = serde_json::Map::new();
    provider_ids.insert("Tmdb".to_owned(), json!(details.id));
    if let Some(imdb_id) = details
        .external_ids
        .imdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        provider_ids.insert("Imdb".to_owned(), json!(imdb_id));
    }
    object.insert("ProviderIds".to_owned(), Value::Object(provider_ids));
    Value::Object(object)
}

fn data_i32(data: &serde_json::Map<String, Value>, key: &str) -> Option<i32> {
    data.get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

fn take_data_string(data: &mut serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match data.remove(key) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

fn trailer_urls(videos: &TmdbVideos) -> Vec<String> {
    videos
        .results
        .iter()
        .filter(|video| {
            video
                .site
                .as_deref()
                .is_some_and(|site| site.eq_ignore_ascii_case("youtube"))
                && video
                    .video_type
                    .as_deref()
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("trailer"))
        })
        .filter_map(|video| video.key.as_deref().filter(|key| !key.is_empty()))
        .map(|key| format!("https://www.youtube.com/watch?v={key}"))
        .collect()
}

fn datetime_to_ticks(date: DateTime<Utc>) -> i64 {
    date.timestamp() * 10_000_000 + i64::from(date.timestamp_subsec_nanos()) / 100
}

fn ticks_to_datetime(ticks: i64) -> Option<DateTime<Utc>> {
    let seconds = ticks.div_euclid(10_000_000);
    let subsec_nanos =
        u32::try_from(ticks.rem_euclid(10_000_000)).expect("remainder always fits in u32") * 100;
    DateTime::<Utc>::from_timestamp(seconds, subsec_nanos)
}

fn upsert_i32(data: &mut serde_json::Map<String, Value>, key: &str, value: Option<i32>) {
    if let Some(value) = value {
        data.insert(key.to_owned(), json!(value));
    }
}

/// Maps TMDB image endpoints to Jellyfin's remote image DTOs.
pub(crate) fn images_to_remote_images(
    images: TmdbImages,
    include_all_languages: bool,
) -> Vec<RemoteImageInfo> {
    let mut result = Vec::new();
    append_images(&mut result, images.posters, ImageType::Primary, "w342");
    append_images(&mut result, images.backdrops, ImageType::Backdrop, "w780");
    append_images(&mut result, images.logos, ImageType::Logo, "w500");
    append_images(&mut result, images.profiles, ImageType::Profile, "w185");
    if !include_all_languages {
        result.retain(|image| {
            image
                .language
                .as_deref()
                .is_none_or(|language| language.is_empty() || language.eq_ignore_ascii_case("en"))
        });
    }
    result
}

fn append_images(
    result: &mut Vec<RemoteImageInfo>,
    images: Vec<TmdbImage>,
    image_type: ImageType,
    thumbnail_size: &str,
) {
    for image in images {
        let Some(file_path) = image.file_path.as_deref().filter(|path| !path.is_empty()) else {
            continue;
        };
        result.push(RemoteImageInfo {
            provider_name: Some(TMDB_PROVIDER_NAME.to_owned()),
            url: TmdbUtils::image_url(Some("original"), Some(file_path)),
            thumbnail_url: TmdbUtils::image_url(Some(thumbnail_size), Some(file_path)),
            height: image.height,
            width: image.width,
            community_rating: Some(image.vote_average),
            vote_count: Some(image.vote_count),
            language: image.iso_639_1.filter(|language| !language.is_empty()),
            image_type,
            rating_type: RatingType::Score,
        });
    }
}

fn movie_provider_ids(details: &TmdbMovieDetails) -> std::collections::BTreeMap<String, String> {
    let mut ids = std::collections::BTreeMap::from([("Tmdb".to_owned(), details.id.to_string())]);
    if let Some(imdb_id) = details
        .external_ids
        .imdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        ids.insert("Imdb".to_owned(), imdb_id.to_owned());
    }
    if let Some(collection) = details.belongs_to_collection.as_ref() {
        ids.insert("TmdbCollection".to_owned(), collection.id.to_string());
    }
    ids
}

fn tv_provider_ids(details: &TmdbTvDetails) -> std::collections::BTreeMap<String, String> {
    let mut ids = std::collections::BTreeMap::from([("Tmdb".to_owned(), details.id.to_string())]);
    if let Some(imdb_id) = details
        .external_ids
        .imdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        ids.insert("Imdb".to_owned(), imdb_id.to_owned());
    }
    if let Some(tvdb_id) = details
        .external_ids
        .tvdb_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        ids.insert("Tvdb".to_owned(), tvdb_id.to_owned());
    }
    if let Some(tvrage_id) = details
        .external_ids
        .tvrage_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        ids.insert("TvRage".to_owned(), tvrage_id.to_owned());
    }
    ids
}

#[allow(clippy::too_many_arguments)]
fn movie_extra_data(
    data: Option<Value>,
    original_title: Option<&str>,
    original_language: Option<&str>,
    tagline: Option<&str>,
    status: Option<&str>,
    vote_average: f64,
    production_countries: &[TmdbCountry],
    videos: &TmdbVideos,
) -> Value {
    let mut object = metadata_object(data);
    set_string(&mut object, "OriginalTitle", original_title);
    set_string(&mut object, "OriginalLanguage", original_language);
    set_string(&mut object, "Tagline", tagline);
    set_string(&mut object, "Status", status);
    object.insert("CommunityRating".to_owned(), json!(vote_average));
    object.insert(
        "ProductionLocations".to_owned(),
        json!(
            production_countries
                .iter()
                .filter_map(|country| country.name.as_deref())
                .collect::<Vec<_>>()
        ),
    );
    object.insert("RemoteTrailers".to_owned(), json!(trailers(videos)));
    Value::Object(object)
}

fn episode_data_with_rating(
    existing: Option<Value>,
    tmdb_id: &str,
    community_rating: f64,
    vote_count: i32,
) -> Value {
    let mut object = metadata_object(existing);
    if let Some(provider_ids) = object.get_mut("ProviderIds").and_then(Value::as_object_mut) {
        provider_ids.insert("Tmdb".to_owned(), Value::String(tmdb_id.to_owned()));
    } else {
        object.insert(
            "ProviderIds".to_owned(),
            json!({ "Tmdb": tmdb_id.to_owned() }),
        );
    }
    if community_rating > 0.0 {
        object.insert("CommunityRating".to_owned(), json!(community_rating));
    }
    object.insert("VoteCount".to_owned(), json!(vote_count));
    Value::Object(object)
}

#[allow(clippy::too_many_arguments)]
fn tv_extra_data(
    data: Option<Value>,
    original_name: Option<&str>,
    original_language: Option<&str>,
    tagline: Option<&str>,
    status: Option<&str>,
    vote_average: f64,
    production_countries: &[TmdbCountry],
    videos: &TmdbVideos,
    number_of_seasons: Option<i32>,
    number_of_episodes: Option<i32>,
    studios: &[String],
) -> Value {
    let mut object = metadata_object(data);
    set_string(&mut object, "OriginalTitle", original_name);
    set_string(&mut object, "OriginalLanguage", original_language);
    set_string(&mut object, "Tagline", tagline);
    set_string(&mut object, "Status", status);
    object.insert("CommunityRating".to_owned(), json!(vote_average));
    object.insert(
        "ProductionLocations".to_owned(),
        json!(
            production_countries
                .iter()
                .filter_map(|country| country.name.as_deref())
                .collect::<Vec<_>>()
        ),
    );
    object.insert("Studios".to_owned(), json!(studios));
    object.insert("RemoteTrailers".to_owned(), json!(trailers(videos)));
    if let Some(seasons) = number_of_seasons {
        object.insert("NumberOfSeasons".to_owned(), json!(seasons));
    }
    if let Some(episodes) = number_of_episodes {
        object.insert("NumberOfEpisodes".to_owned(), json!(episodes));
    }
    Value::Object(object)
}

fn metadata_object(data: Option<Value>) -> serde_json::Map<String, Value> {
    match data {
        Some(Value::Object(object)) => object,
        _ => serde_json::Map::new(),
    }
}

fn set_string(object: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        object.insert(key.to_owned(), json!(value));
    }
}

fn trailers(videos: &TmdbVideos) -> Vec<Value> {
    videos
        .results
        .iter()
        .filter(|video| {
            video.site.as_deref().is_some_and(|site| site.eq_ignore_ascii_case("youtube"))
                && video.video_type.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("trailer"))
        })
        .map(|video| {
            json!({
                "Name": video.name,
                "Url": format!("https://www.youtube.com/watch?v={}", video.key.as_deref().unwrap_or_default())
            })
        })
        .collect()
}

fn us_rating(release_dates: &TmdbReleaseDates) -> Option<String> {
    let releases = release_dates
        .results
        .iter()
        .find(|group| group.iso_3166_1.eq_ignore_ascii_case("US"))?;
    releases.release_dates.iter().find_map(|release| {
        release
            .certification
            .as_deref()
            .filter(|certification| !certification.trim().is_empty())
            .map(str::to_owned)
    })
}

fn tv_rating(content_ratings: &TmdbContentRatings) -> Option<String> {
    content_ratings
        .results
        .iter()
        .find(|result| result.iso_3166_1.eq_ignore_ascii_case("US"))
        .and_then(|result| result.rating.as_deref())
        .filter(|rating| !rating.is_empty())
        .map(str::to_owned)
}

fn into_names<T: Named>(values: Vec<T>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(Named::into_name)
        .filter(|name| !name.trim().is_empty())
        .collect()
}

trait Named {
    fn into_name(self) -> Option<String>;
}

impl Named for TmdbGenre {
    fn into_name(self) -> Option<String> {
        self.name
    }
}

impl Named for TmdbCompany {
    fn into_name(self) -> Option<String> {
        self.name
    }
}

fn into_keyword_names(keywords: TmdbKeywordResults) -> Vec<String> {
    keywords
        .keywords
        .into_iter()
        .chain(keywords.results)
        .filter_map(|keyword| keyword.name)
        .filter(|name| !name.trim().is_empty())
        .collect()
}

fn crew_person_type(department: Option<&str>, job: Option<&str>) -> Option<&'static str> {
    match TmdbUtils::map_crew_to_person_kind(department, job) {
        jellyfin_providers::tmdb::TmdbPersonKind::Director => Some("Director"),
        jellyfin_providers::tmdb::TmdbPersonKind::Writer => Some("Writer"),
        jellyfin_providers::tmdb::TmdbPersonKind::Producer => Some("Producer"),
        jellyfin_providers::tmdb::TmdbPersonKind::Unknown => None,
    }
}

pub(crate) fn provider_id(data: Option<&Value>, key: &str) -> Option<String> {
    data?
        .get("ProviderIds")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

fn parse_year(value: Option<&str>) -> Option<i32> {
    parse_tmdb_date(value).map(|date| date.year())
}

fn parse_tmdb_date(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let naive = NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok()?;
    Some(DateTime::from_naive_utc_and_offset(
        naive.and_hms_opt(0, 0, 0)?,
        Utc,
    ))
}

// ---- TMDB response types ----

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
struct TmdbSearchResponse<T> {
    results: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbSearchMovie {
    id: i64,
    title: Option<String>,
    original_title: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    release_date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbSearchTv {
    id: i64,
    name: Option<String>,
    original_name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    first_air_date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbSearchPerson {
    id: i64,
    name: String,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbSearchCollection {
    id: i64,
    name: String,
    poster_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbMovieDetails {
    id: i64,
    imdb_id: Option<String>,
    title: Option<String>,
    original_title: Option<String>,
    tagline: Option<String>,
    overview: Option<String>,
    runtime: Option<i32>,
    release_date: Option<String>,
    vote_average: f64,
    vote_count: i32,
    status: Option<String>,
    original_language: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Vec<TmdbGenre>,
    production_companies: Vec<TmdbCompany>,
    production_countries: Vec<TmdbCountry>,
    belongs_to_collection: Option<TmdbCollection>,
    credits: TmdbCredits,
    release_dates: TmdbReleaseDates,
    external_ids: TmdbExternalIds,
    videos: TmdbVideos,
    images: TmdbImages,
    keywords: TmdbKeywordResults,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbTvDetails {
    id: i64,
    name: Option<String>,
    original_name: Option<String>,
    tagline: Option<String>,
    overview: Option<String>,
    first_air_date: Option<String>,
    last_air_date: Option<String>,
    number_of_seasons: Option<i32>,
    number_of_episodes: Option<i32>,
    vote_average: f64,
    vote_count: i32,
    status: Option<String>,
    original_language: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genres: Vec<TmdbGenre>,
    networks: Vec<TmdbCompany>,
    production_companies: Vec<TmdbCompany>,
    production_countries: Vec<TmdbCountry>,
    credits: TmdbCredits,
    content_ratings: TmdbContentRatings,
    external_ids: TmdbExternalIds,
    videos: TmdbVideos,
    images: TmdbImages,
    keywords: TmdbKeywordResults,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbTvSeasonDetails {
    pub(crate) name: Option<String>,
    pub(crate) overview: Option<String>,
    pub(crate) air_date: Option<String>,
    pub(crate) poster_path: Option<String>,
    external_ids: TmdbExternalIds,
    credits: TmdbCredits,
    videos: TmdbVideos,
    pub(crate) episodes: Vec<TmdbEpisode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbEpisode {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    episode_number: i32,
    season_number: i32,
    runtime: Option<i32>,
    still_path: Option<String>,
    vote_average: f64,
    vote_count: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbEpisodeDetails {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    season_number: i32,
    episode_number: i32,
    runtime: Option<i32>,
    still_path: Option<String>,
    vote_average: f64,
    vote_count: i32,
    external_ids: TmdbExternalIds,
    videos: TmdbVideos,
    credits: TmdbCredits,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbCollectionDetails {
    id: i64,
    name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbPersonDetails {
    id: i64,
    name: Option<String>,
    homepage: Option<String>,
    biography: Option<String>,
    birthday: Option<String>,
    deathday: Option<String>,
    place_of_birth: Option<String>,
    profile_path: Option<String>,
    external_ids: TmdbExternalIds,
    images: TmdbImages,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbImages {
    pub(crate) backdrops: Vec<TmdbImage>,
    pub(crate) posters: Vec<TmdbImage>,
    pub(crate) logos: Vec<TmdbImage>,
    pub(crate) profiles: Vec<TmdbImage>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct TmdbImage {
    pub(crate) file_path: Option<String>,
    pub(crate) width: Option<i32>,
    pub(crate) height: Option<i32>,
    pub(crate) iso_639_1: Option<String>,
    pub(crate) vote_average: f64,
    pub(crate) vote_count: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbGenre {
    id: i32,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCompany {
    id: i32,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCountry {
    iso_3166_1: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCollection {
    id: i64,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCredits {
    cast: Vec<TmdbCast>,
    crew: Vec<TmdbCrew>,
    guest_stars: Vec<TmdbCast>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCast {
    id: i64,
    name: String,
    character: Option<String>,
    order: i32,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbCrew {
    id: i64,
    name: String,
    job: Option<String>,
    department: Option<String>,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbReleaseDates {
    results: Vec<TmdbReleaseDateGroup>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbReleaseDateGroup {
    iso_3166_1: String,
    release_dates: Vec<TmdbReleaseDate>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbReleaseDate {
    certification: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbContentRatings {
    results: Vec<TmdbContentRating>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbContentRating {
    iso_3166_1: String,
    rating: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
#[allow(clippy::struct_field_names)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_number")]
    tvdb_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_or_number")]
    tvrage_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbVideos {
    results: Vec<TmdbVideo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbVideo {
    key: Option<String>,
    name: Option<String>,
    site: Option<String>,
    #[serde(rename = "type")]
    video_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbKeywordResults {
    keywords: Vec<TmdbKeyword>,
    results: Vec<TmdbKeyword>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct TmdbKeyword {
    id: i32,
    name: Option<String>,
}

fn deserialize_optional_string_or_number<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        Value::Number(value) => Ok(Some(value.to_string())),
        value => Err(D::Error::custom(format!(
            "expected a string, number, or null, got {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmdb_language_combines_language_and_country() {
        assert_eq!(tmdb_language("zh", "CN"), "zh-CN");
    }

    #[test]
    fn tmdb_language_normalizes_locale_separator() {
        assert_eq!(tmdb_language("zh_CN", "US"), "zh-CN");
    }

    #[test]
    fn tmdb_language_defaults_when_language_is_empty() {
        assert_eq!(tmdb_language("  ", "CN"), "en-US");
    }

    #[test]
    fn movie_search_result_maps_tmdb_id_year_and_poster() {
        let result = movie_search_to_remote_result(TmdbSearchMovie {
            id: 30287,
            title: Some("Fallen".to_owned()),
            original_title: None,
            overview: Some("Detective story".to_owned()),
            poster_path: Some("/falling.jpg".to_owned()),
            release_date: Some("1998-01-16".to_owned()),
        });

        assert_eq!(result.provider_ids["Tmdb"], "30287");
        assert_eq!(result.production_year, Some(1998));
        assert_eq!(result.name.as_deref(), Some("Fallen"));
        assert!(
            result
                .image_url
                .as_deref()
                .is_some_and(|url| url.ends_with("/falling.jpg"))
        );
    }

    #[test]
    fn images_are_mapped_to_remote_image_types() {
        let images = TmdbImages {
            posters: vec![TmdbImage {
                file_path: Some("/poster.jpg".to_owned()),
                width: Some(1000),
                height: Some(1500),
                iso_639_1: Some("en".to_owned()),
                vote_average: 7.5,
                vote_count: 10,
            }],
            backdrops: vec![TmdbImage {
                file_path: Some("/backdrop.jpg".to_owned()),
                width: Some(1920),
                height: Some(1080),
                iso_639_1: Some("zh".to_owned()),
                vote_average: 6.0,
                vote_count: 4,
            }],
            logos: Vec::new(),
            profiles: Vec::new(),
        };

        let all = images_to_remote_images(images.clone(), true);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].image_type, ImageType::Primary);
        assert_eq!(all[1].image_type, ImageType::Backdrop);

        let english_only = images_to_remote_images(images, false);
        assert_eq!(english_only.len(), 1);
        assert_eq!(english_only[0].image_type, ImageType::Primary);
    }

    #[test]
    fn movie_details_deserialize_tmdb_snake_case_fields() {
        let details: TmdbMovieDetails = serde_json::from_str(
            r#"{
                "id": 152044,
                "imdb_id": "tt2735226",
                "original_title": "劇場版 魔法少女まどか☆マギカ 永遠の物語",
                "release_date": "2012-10-13",
                "vote_average": 7.6,
                "vote_count": 217,
                "original_language": "ja",
                "poster_path": "/poster.jpg",
                "backdrop_path": "/backdrop.jpg",
                "production_companies": [{"id": 1, "name": "Shaft"}],
                "production_countries": [{"iso_3166_1": "JP", "name": "Japan"}],
                "credits": {
                    "cast": [{"id": 2, "name": "Actor", "profile_path": "/actor.jpg"}],
                    "guest_stars": []
                },
                "release_dates": {
                    "results": [{"iso_3166_1": "US", "release_dates": [{"certification": "PG-13"}]}]
                },
                "external_ids": {"imdb_id": "tt2735226", "tvdb_id": 1234},
                "videos": {
                    "results": [{"key": "trailer", "site": "YouTube", "type": "Trailer"}]
                },
                "images": {
                    "posters": [{
                        "file_path": "/poster.jpg",
                        "iso_639_1": "en",
                        "vote_average": 7.2,
                        "vote_count": 4,
                        "width": 1000,
                        "height": 1500
                    }]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(details.imdb_id.as_deref(), Some("tt2735226"));
        assert_eq!(
            details.original_title.as_deref(),
            Some("劇場版 魔法少女まどか☆マギカ 永遠の物語")
        );
        assert_eq!(details.poster_path.as_deref(), Some("/poster.jpg"));
        assert_eq!(details.backdrop_path.as_deref(), Some("/backdrop.jpg"));
        assert_eq!(details.production_companies.len(), 1);
        assert_eq!(
            details.production_countries[0].iso_3166_1.as_deref(),
            Some("JP")
        );
        assert_eq!(
            details.credits.cast[0].profile_path.as_deref(),
            Some("/actor.jpg")
        );
        assert_eq!(details.release_dates.results[0].iso_3166_1, "US");
        assert_eq!(details.external_ids.tvdb_id.as_deref(), Some("1234"));
        assert_eq!(
            details.videos.results[0].video_type.as_deref(),
            Some("Trailer")
        );
        assert_eq!(
            details.images.posters[0].file_path.as_deref(),
            Some("/poster.jpg")
        );
        assert_eq!(details.images.posters[0].iso_639_1.as_deref(), Some("en"));
    }

    #[test]
    fn tmdb_dates_parse_to_utc_midnight() {
        let date = parse_tmdb_date(Some("2026-08-21")).expect("date");
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 8);
        assert_eq!(date.day(), 21);
        assert_eq!(parse_tmdb_date(Some("")), None);
        assert_eq!(parse_tmdb_date(None), None);
    }

    #[test]
    fn episode_data_keeps_existing_fields_and_adds_tmdb_rating() {
        let existing = json!({ "Container": "mkv" });
        let data = episode_data_with_rating(Some(existing), "12345", 8.4, 42);

        assert_eq!(data["Container"], "mkv");
        assert_eq!(data["ProviderIds"]["Tmdb"], "12345");
        assert_eq!(data["CommunityRating"], 8.4);
        assert_eq!(data["VoteCount"], 42);
    }

    #[test]
    fn tv_season_details_deserialize_episode_fields() {
        let season: TmdbTvSeasonDetails = serde_json::from_str(
            r#"{
                "name": "Season 1",
                "overview": "The first season.",
                "poster_path": "/season1.jpg",
                "episodes": [{
                    "id": 1,
                    "name": "Pilot",
                    "overview": "The beginning.",
                    "air_date": "2026-01-01",
                    "episode_number": 1,
                    "season_number": 1,
                    "runtime": 45,
                    "still_path": "/pilot.jpg",
                    "vote_average": 7.5,
                    "vote_count": 10
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(season.name.as_deref(), Some("Season 1"));
        assert_eq!(season.overview.as_deref(), Some("The first season."));
        assert_eq!(season.poster_path.as_deref(), Some("/season1.jpg"));
        assert_eq!(season.episodes.len(), 1);
        assert_eq!(season.episodes[0].name.as_deref(), Some("Pilot"));
        assert_eq!(season.episodes[0].episode_number, 1);
        assert_eq!(season.episodes[0].season_number, 1);
        assert_eq!(season.episodes[0].still_path.as_deref(), Some("/pilot.jpg"));
    }

    #[test]
    fn episode_metadata_from_details_maps_official_fields() {
        let details = TmdbEpisodeDetails {
            id: 42,
            name: Some("Episode".to_owned()),
            overview: Some("Overview".to_owned()),
            air_date: Some("2020-01-02".to_owned()),
            season_number: 2,
            episode_number: 3,
            runtime: Some(45),
            still_path: None,
            vote_average: 7.5,
            vote_count: 10,
            external_ids: TmdbExternalIds {
                imdb_id: Some("tt123".to_owned()),
                tvdb_id: Some("456".to_owned()),
                tvrage_id: Some("789".to_owned()),
            },
            videos: TmdbVideos {
                results: vec![TmdbVideo {
                    key: Some("abc".to_owned()),
                    name: None,
                    site: Some("YouTube".to_owned()),
                    video_type: Some("Trailer".to_owned()),
                }],
            },
            credits: TmdbCredits::default(),
        };
        let lookup = EpisodeLookupInfo {
            index_number: Some(3),
            index_number_end: Some(4),
            ..EpisodeLookupInfo::default()
        };

        let metadata = episode_metadata_from_details(details, &lookup);
        assert_eq!(metadata.name.as_deref(), Some("Episode"));
        assert_eq!(metadata.parent_index_number, Some(2));
        assert_eq!(metadata.index_number_end, Some(4));
        assert_eq!(metadata.runtime_ticks, Some(45 * 60 * 10_000_000));
        assert_eq!(metadata.provider_ids["Tmdb"], "42");
        assert_eq!(metadata.provider_ids["Tvdb"], "456");
        assert_eq!(
            metadata.remote_trailers,
            ["https://www.youtube.com/watch?v=abc"]
        );
    }

    #[test]
    fn person_extra_data_keeps_provider_and_place_of_birth() {
        let details = TmdbPersonDetails {
            id: 7,
            place_of_birth: Some("Paris".to_owned()),
            external_ids: TmdbExternalIds {
                imdb_id: Some("nm123".to_owned()),
                ..TmdbExternalIds::default()
            },
            ..TmdbPersonDetails::default()
        };
        let data = person_extra_data(None, &details);
        assert_eq!(data["ProviderIds"]["Tmdb"], json!(7));
        assert_eq!(data["ProviderIds"]["Imdb"], json!("nm123"));
        assert_eq!(data["ProductionLocations"], json!(["Paris"]));
    }

    #[test]
    fn tv_provider_ids_include_tmdb_external_ids() {
        let details = TmdbTvDetails {
            id: 1399,
            external_ids: TmdbExternalIds {
                imdb_id: Some("tt0944947".to_owned()),
                tvdb_id: Some("121361".to_owned()),
                tvrage_id: Some("24493".to_owned()),
            },
            ..TmdbTvDetails::default()
        };

        let ids = tv_provider_ids(&details);
        assert_eq!(ids["Tmdb"], "1399");
        assert_eq!(ids["Imdb"], "tt0944947");
        assert_eq!(ids["Tvdb"], "121361");
        assert_eq!(ids["TvRage"], "24493");
    }
}
