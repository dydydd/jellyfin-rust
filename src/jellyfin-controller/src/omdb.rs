use std::{collections::BTreeMap, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ItemValueError, ItemValueRepository, entities::item_value::ItemValueType,
};
use jellyfin_providers::{
    manager::metadata_service::{
        DefaultMetadataServiceCapability, MetadataResult, MetadataService,
    },
    omdb::{JsonOmdbConverter, OmdbItem},
};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const OMDB_API_BASE_URL: &str = "https://www.omdbapi.com";
const DEFAULT_OMDB_API_KEY: &str = "2c9d9507";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum OmdbMetadataProviderError {
    #[error("OMDb provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("OMDb provider response error: {0}")]
    Omdb(String),
    #[error("OMDb metadata response is invalid")]
    Json,
    #[error("no IMDb provider id is available")]
    NoImdbId,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

/// `OMDb` v3 client used by the built-in movie and series metadata provider.
#[derive(Clone)]
struct OmdbClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OmdbClient {
    #[must_use]
    fn new(api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        let api_key = if api_key.trim().is_empty() {
            DEFAULT_OMDB_API_KEY.to_owned()
        } else {
            api_key
        };
        Self::with_base_url(api_key, OMDB_API_BASE_URL)
    }

    #[must_use]
    fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: builder.build().unwrap_or_else(|error| {
                tracing::error!(%error, "could not configure the OMDb HTTP client");
                reqwest::Client::new()
            }),
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    async fn fetch(&self, imdb_id: &str) -> Result<OmdbItem, OmdbMetadataProviderError> {
        let api_key = self.api_key.trim();
        if api_key.is_empty() {
            return Err(OmdbMetadataProviderError::Omdb(
                "no OMDb API key configured".to_owned(),
            ));
        }
        let imdb_id = if imdb_id.starts_with("tt") {
            imdb_id.to_owned()
        } else {
            format!("tt{imdb_id}")
        };
        let response = self
            .http
            .get(&self.base_url)
            .query(&[
                ("apikey", api_key),
                ("i", &imdb_id),
                ("plot", "short"),
                ("tomatoes", "true"),
                ("r", "json"),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let item = JsonOmdbConverter::deserialize_item(&response)
            .map_err(|_| OmdbMetadataProviderError::Json)?;
        if item
            .response
            .as_deref()
            .is_some_and(|response| response.eq_ignore_ascii_case("False"))
        {
            return Err(OmdbMetadataProviderError::Omdb(
                item.error
                    .unwrap_or_else(|| "OMDb returned no metadata".to_owned()),
            ));
        }
        Ok(item)
    }
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_OMDB_PROXY",
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

/// Fetches `OMDb` metadata and merges it into movie and series items.
pub struct OmdbMetadataProvider {
    client: OmdbClient,
    items: std::sync::Arc<BaseItemRepository>,
    values: std::sync::Arc<ItemValueRepository>,
    updates: std::sync::Arc<ItemUpdateRepository>,
}

impl OmdbMetadataProvider {
    #[must_use]
    pub fn new(
        api_key: impl Into<String>,
        items: std::sync::Arc<BaseItemRepository>,
        values: std::sync::Arc<ItemValueRepository>,
        updates: std::sync::Arc<ItemUpdateRepository>,
    ) -> Self {
        Self {
            client: OmdbClient::new(api_key),
            items,
            values,
            updates,
        }
    }

    /// Refreshes one movie or series item from `OMDb` when an `IMDb` id exists.
    ///
    /// # Errors
    ///
    /// Returns a provider or persistence error when the lookup fails.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, OmdbMetadataProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        if !matches!(item.item_type.as_str(), "Movie" | "Series") {
            return Ok(false);
        }
        let Some(imdb_id) = provider_id(item.data.as_ref(), "Imdb") else {
            return Ok(false);
        };
        let omdb = self.client.fetch(&imdb_id).await?;
        self.apply(item.id, omdb).await?;
        Ok(true)
    }

    async fn apply(
        &self,
        item_id: Uuid,
        mut omdb: OmdbItem,
    ) -> Result<(), OmdbMetadataProviderError> {
        let release_date = omdb.release_date();
        let language = omdb.language.take();
        let website = omdb.website.take();
        let mut result = MetadataResult::default();
        MetadataService::merge_omdb_item(
            omdb,
            &mut result,
            &[],
            false,
            &DefaultMetadataServiceCapability,
        );

        let provider_ids = std::mem::take(&mut result.item.core.provider_ids)
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let genres = std::mem::take(&mut result.item.genres);
        let studios = std::mem::take(&mut result.item.studios);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: None,
                    genres: Some(genres),
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
        if let Some(name) = result
            .item
            .core
            .name
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        item.overview =
            std::mem::take(&mut result.item.core.overview).filter(|value| !value.trim().is_empty());
        item.official_rating = std::mem::take(&mut result.item.official_rating);
        item.production_year = result.item.production_year;
        if let Some(date) = release_date
            && let Some(premiere_date) = omdb_date_to_utc(date)
        {
            item.premiere_date = Some(premiere_date);
        }
        item.data = Some(omdb_extra_data(
            item.data.as_ref(),
            language,
            website,
            &result,
        ));
        self.items.update(item).await?;
        Ok(())
    }
}

fn omdb_extra_data(
    existing: Option<&Value>,
    language: Option<String>,
    website: Option<String>,
    result: &MetadataResult,
) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(original_title) = result
        .item
        .core
        .name
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("OriginalTitle".to_owned(), json!(original_title));
    }
    if let Some(language) = language.and_then(first_list_value) {
        object.insert("OriginalLanguage".to_owned(), Value::String(language));
    }
    if let Some(website) = website.filter(|value| !value.is_empty()) {
        object.insert("HomePageUrl".to_owned(), Value::String(website));
    }
    Value::Object(object)
}

fn first_list_value(mut value: String) -> Option<String> {
    if let Some(comma) = value.find(',') {
        value.truncate(comma);
    }
    let leading_whitespace = value.len() - value.trim_start().len();
    value.drain(..leading_whitespace);
    value.truncate(value.trim_end().len());
    (!value.is_empty()).then_some(value)
}

fn omdb_date_to_utc(date: jellyfin_providers::omdb::OmdbDate) -> Option<DateTime<Utc>> {
    let naive = NaiveDate::from_ymd_opt(date.year, date.month.into(), date.day.into())?;
    Some(DateTime::from_naive_utc_and_offset(
        naive.and_hms_opt(0, 0, 0)?,
        Utc,
    ))
}

fn provider_id(data: Option<&Value>, key: &str) -> Option<String> {
    data?
        .get("ProviderIds")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}
