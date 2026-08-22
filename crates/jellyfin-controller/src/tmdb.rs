use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ItemValueError, ItemValueRepository, NewBaseItem, NewPerson, PersonError,
    PersonRepository,
    entities::{base_item, item_value::ItemValueType},
};
use jellyfin_model::{ImageType, RatingType, RemoteImageInfo, RemoteSearchResult};
use jellyfin_providers::tmdb::TmdbUtils;
use serde::Deserialize;
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
}

impl TmdbClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, TMDB_API_BASE_URL.to_owned())
    }

    #[must_use]
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: http_client(),
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    pub(crate) async fn search_movie(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Vec<RemoteSearchResult>, MetadataProviderError> {
        let mut query = vec![
            ("query", name),
            ("language", "en-US"),
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
            ("language", "en-US"),
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
                &[("query", name), ("include_adult", "false")],
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
                &[("query", name), ("language", "en-US")],
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
                ("language", "en-US"),
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
                ("language", "en-US"),
                (
                    "append_to_response",
                    "credits,content_ratings,external_ids,videos,images,keywords",
                ),
            ],
        )
        .await
    }

    pub(crate) async fn movie_images(&self, id: i64) -> Result<TmdbImages, MetadataProviderError> {
        self.get_json(
            &format!("/movie/{id}/images"),
            &[("include_image_language", "en,null")],
        )
        .await
    }

    pub(crate) async fn tv_images(&self, id: i64) -> Result<TmdbImages, MetadataProviderError> {
        self.get_json(
            &format!("/tv/{id}/images"),
            &[("include_image_language", "en,null")],
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
        query.push(("api_key", key.as_str()));
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

    fn api_key(&self) -> Result<String, MetadataProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(MetadataProviderError::NoApiKey);
        }
        Ok(self.api_key.clone())
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

fn movie_search_to_remote_result(result: TmdbSearchMovie) -> RemoteSearchResult {
    RemoteSearchResult {
        name: result
            .title
            .clone()
            .or_else(|| result.original_title.clone()),
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

fn tv_search_to_remote_result(result: TmdbSearchTv) -> RemoteSearchResult {
    RemoteSearchResult {
        name: result.name.clone().or_else(|| result.original_name.clone()),
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
    items: BaseItemRepository,
    values: ItemValueRepository,
    people: PersonRepository,
    updates: ItemUpdateRepository,
    images: Option<ItemImageService>,
}

impl TmdbMetadataProvider {
    #[must_use]
    pub fn new(
        api_key: impl Into<String>,
        items: BaseItemRepository,
        values: ItemValueRepository,
        people: PersonRepository,
        updates: ItemUpdateRepository,
        images: Option<ItemImageService>,
    ) -> Self {
        Self {
            client: TmdbClient::new(api_key),
            items,
            values,
            people,
            updates,
            images,
        }
    }

    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, MetadataProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };

        match item.item_type.as_str() {
            "Movie" => self.refresh_movie(&item).await,
            "Series" => self.refresh_series(&item).await,
            _ => Ok(false),
        }
    }

    async fn refresh_movie(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_movie_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.movie_details(tmdb_id).await?;
        self.apply_movie_metadata(item.id, &details).await?;
        self.save_remote_images(
            item.id,
            details.poster_path.as_deref(),
            details.backdrop_path.as_deref(),
        )
        .await;
        Ok(true)
    }

    async fn refresh_series(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let Some(tmdb_id) = self.resolve_series_id(item).await? else {
            return Ok(false);
        };
        let details = self.client.tv_details(tmdb_id).await?;
        self.apply_tv_metadata(item.id, &details).await?;
        self.save_remote_images(
            item.id,
            details.poster_path.as_deref(),
            details.backdrop_path.as_deref(),
        )
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
            .and_then(|result| result.provider_ids.get("Tmdb").cloned())
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
            .and_then(|result| result.provider_ids.get("Tmdb").cloned())
            .and_then(|id| id.parse::<i64>().ok()))
    }

    async fn apply_movie_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbMovieDetails,
    ) -> Result<(), MetadataProviderError> {
        let provider_ids = movie_provider_ids(details);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(keyword_names(&details.keywords)),
                    genres: Some(names(&details.genres)),
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;
        for studio in names(&details.production_companies) {
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
            .map(|minutes| minutes as i64 * 60 * 10_000_000);
        if let Some(premiere_date) = parse_tmdb_date(details.release_date.as_deref()) {
            item.premiere_date = Some(premiere_date);
            item.production_year = Some(premiere_date.year());
        }
        item.data = Some(movie_extra_data(&item.data, details));
        self.items.update(item).await?;

        self.replace_people(item_id, &details.credits.cast, &details.credits.crew)
            .await?;
        Ok(())
    }

    async fn apply_tv_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbTvDetails,
    ) -> Result<(), MetadataProviderError> {
        let provider_ids = tv_provider_ids(details);
        let mut studios = names(&details.networks);
        for studio in names(&details.production_companies) {
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
                    tags: Some(keyword_names(&details.keywords)),
                    genres: Some(names(&details.genres)),
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
        item.data = Some(tv_extra_data(&item.data, details, &studios));
        self.items.update(item).await?;

        self.replace_people(item_id, &details.credits.cast, &details.credits.crew)
            .await?;
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
            self.ensure_person_image(&person.name, actor.profile_path.as_deref())
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
            self.ensure_person_image(&person.name, member.profile_path.as_deref())
                .await?;
            order += 1;
        }
        Ok(())
    }

    async fn ensure_person_image(
        &self,
        name: &str,
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
        let person_item = if let Some(item) = self.items.get_by_type_and_name("Person", name).await? {
            item
        } else {
            let mut item = NewBaseItem::new(Uuid::new_v4(), "Person");
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
            item.is_virtual_item = true;
            item.data = Some(json!({ "SourceType": "Library" }));
            self.items.create(item).await?
        };
        if let Err(error) = images
            .download_remote_image(person_item.id, ImageType::Profile, &image_url)
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
    ) {
        let Some(images) = &self.images else {
            return;
        };
        let Some(item) = self.items.get(item_id).await.ok().flatten() else {
            return;
        };
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
        if !has_primary && let Some(url) = TmdbUtils::image_url(Some("original"), poster_path) {
            if let Err(error) = images
                .download_remote_image(item_id, ImageType::Primary, &url)
                .await
            {
                tracing::warn!(%error, "TMDB primary image download failed");
            }
        }
        if !has_backdrop && let Some(url) = TmdbUtils::image_url(Some("original"), backdrop_path) {
            if let Err(error) = images
                .download_remote_image(item_id, ImageType::Backdrop, &url)
                .await
            {
                tracing::warn!(%error, "TMDB backdrop download failed");
            }
        }
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
    ids
}

fn movie_extra_data(data: &Option<Value>, details: &TmdbMovieDetails) -> Value {
    let mut object = metadata_object(data);
    set_string(
        &mut object,
        "OriginalTitle",
        details.original_title.as_deref(),
    );
    set_string(
        &mut object,
        "OriginalLanguage",
        details.original_language.as_deref(),
    );
    set_string(&mut object, "Tagline", details.tagline.as_deref());
    set_string(&mut object, "Status", details.status.as_deref());
    object.insert("CommunityRating".to_owned(), json!(details.vote_average));
    object.insert(
        "ProductionLocations".to_owned(),
        json!(
            details
                .production_countries
                .iter()
                .filter_map(|country| country.name.clone())
                .collect::<Vec<_>>()
        ),
    );
    object.insert(
        "RemoteTrailers".to_owned(),
        json!(trailers(&details.videos)),
    );
    Value::Object(object)
}

fn tv_extra_data(data: &Option<Value>, details: &TmdbTvDetails, studios: &[String]) -> Value {
    let mut object = metadata_object(data);
    set_string(
        &mut object,
        "OriginalTitle",
        details.original_name.as_deref(),
    );
    set_string(
        &mut object,
        "OriginalLanguage",
        details.original_language.as_deref(),
    );
    set_string(&mut object, "Tagline", details.tagline.as_deref());
    set_string(&mut object, "Status", details.status.as_deref());
    object.insert("CommunityRating".to_owned(), json!(details.vote_average));
    object.insert(
        "ProductionLocations".to_owned(),
        json!(
            details
                .production_countries
                .iter()
                .filter_map(|country| country.name.clone())
                .collect::<Vec<_>>()
        ),
    );
    object.insert("Studios".to_owned(), json!(studios));
    object.insert(
        "RemoteTrailers".to_owned(),
        json!(trailers(&details.videos)),
    );
    if let Some(seasons) = details.number_of_seasons {
        object.insert("NumberOfSeasons".to_owned(), json!(seasons));
    }
    if let Some(episodes) = details.number_of_episodes {
        object.insert("NumberOfEpisodes".to_owned(), json!(episodes));
    }
    Value::Object(object)
}

fn metadata_object(data: &Option<Value>) -> serde_json::Map<String, Value> {
    data.as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
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

fn names<T: Named>(values: &[T]) -> Vec<String> {
    values
        .iter()
        .filter_map(Named::name)
        .filter(|name| !name.trim().is_empty())
        .collect()
}

trait Named {
    fn name(&self) -> Option<String>;
}

impl Named for TmdbGenre {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }
}

impl Named for TmdbCompany {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }
}

fn keyword_names(keywords: &TmdbKeywordResults) -> Vec<String> {
    keywords
        .all()
        .into_iter()
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
#[serde(rename_all = "camelCase")]
struct TmdbSearchResponse<T> {
    results: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchMovie {
    id: i64,
    title: Option<String>,
    original_title: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    release_date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchTv {
    id: i64,
    name: Option<String>,
    original_name: Option<String>,
    overview: Option<String>,
    poster_path: Option<String>,
    first_air_date: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchPerson {
    id: i64,
    name: String,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchCollection {
    id: i64,
    name: String,
    poster_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
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
#[serde(rename_all = "camelCase", default)]
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TmdbImages {
    pub(crate) backdrops: Vec<TmdbImage>,
    pub(crate) posters: Vec<TmdbImage>,
    pub(crate) logos: Vec<TmdbImage>,
    pub(crate) profiles: Vec<TmdbImage>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TmdbImage {
    pub(crate) file_path: Option<String>,
    pub(crate) width: Option<i32>,
    pub(crate) height: Option<i32>,
    pub(crate) iso_639_1: Option<String>,
    pub(crate) vote_average: f64,
    pub(crate) vote_count: i32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbGenre {
    id: i32,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCompany {
    id: i32,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCountry {
    iso_3166_1: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCollection {
    id: i64,
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCredits {
    cast: Vec<TmdbCast>,
    crew: Vec<TmdbCrew>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCast {
    id: i64,
    name: String,
    character: Option<String>,
    order: i32,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbCrew {
    id: i64,
    name: String,
    job: Option<String>,
    department: Option<String>,
    profile_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbReleaseDates {
    results: Vec<TmdbReleaseDateGroup>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbReleaseDateGroup {
    iso_3166_1: String,
    release_dates: Vec<TmdbReleaseDate>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbReleaseDate {
    certification: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbContentRatings {
    results: Vec<TmdbContentRating>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbContentRating {
    iso_3166_1: String,
    rating: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbVideos {
    results: Vec<TmdbVideo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbVideo {
    key: Option<String>,
    name: Option<String>,
    site: Option<String>,
    video_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbKeywordResults {
    keywords: Vec<TmdbKeyword>,
    results: Vec<TmdbKeyword>,
}

impl TmdbKeywordResults {
    fn all(&self) -> Vec<TmdbKeyword> {
        let mut keywords = self.keywords.clone();
        keywords.extend(self.results.clone());
        keywords
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TmdbKeyword {
    id: i32,
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn tmdb_dates_parse_to_utc_midnight() {
        let date = parse_tmdb_date(Some("2026-08-21")).expect("date");
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 8);
        assert_eq!(date.day(), 21);
        assert_eq!(parse_tmdb_date(Some("")), None);
        assert_eq!(parse_tmdb_date(None), None);
    }
}
