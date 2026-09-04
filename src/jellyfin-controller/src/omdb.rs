use std::{collections::BTreeMap, sync::Arc, time::Duration};

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
    http: Arc<reqwest::Client>,
    api_key: String,
    base_url: Arc<str>,
}

/// Builds lightweight OMDb clients over one service-level HTTP connection pool.
#[derive(Clone)]
pub(crate) struct OmdbClientFactory {
    http: Arc<reqwest::Client>,
    base_url: Arc<str>,
}

impl OmdbClientFactory {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_base_url(OMDB_API_BASE_URL)
    }

    #[must_use]
    fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: Arc::new(builder.build().unwrap_or_else(|error| {
                tracing::error!(%error, "could not configure the OMDb HTTP client");
                reqwest::Client::new()
            })),
            base_url: Arc::from(base_url.into().trim_end_matches('/')),
        }
    }

    #[must_use]
    fn client(&self, api_key: impl Into<String>) -> OmdbClient {
        let api_key = api_key.into();
        let api_key = if api_key.trim().is_empty() {
            DEFAULT_OMDB_API_KEY.to_owned()
        } else {
            api_key
        };
        OmdbClient {
            http: Arc::clone(&self.http),
            api_key,
            base_url: Arc::clone(&self.base_url),
        }
    }
}

impl OmdbClient {
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
            .get(self.base_url.as_ref())
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
        let clients = OmdbClientFactory::new();
        Self::with_client_factory(&clients, api_key, items, values, updates)
    }

    #[must_use]
    pub(crate) fn with_client_factory(
        clients: &OmdbClientFactory,
        api_key: impl Into<String>,
        items: std::sync::Arc<BaseItemRepository>,
        values: std::sync::Arc<ItemValueRepository>,
        updates: std::sync::Arc<ItemUpdateRepository>,
    ) -> Self {
        Self {
            client: clients.client(api_key),
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
    pub async fn refresh_item(
        &self,
        item_id: Uuid,
        replace_data: bool,
    ) -> Result<bool, OmdbMetadataProviderError> {
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
        self.apply(item.id, omdb, replace_data).await?;
        Ok(true)
    }

    async fn apply(
        &self,
        item_id: Uuid,
        mut omdb: OmdbItem,
        replace_data: bool,
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

        let mut provider_ids = self
            .items
            .get(item_id)
            .await?
            .and_then(|item| provider_ids(item.data.as_ref()))
            .unwrap_or_default();
        provider_ids.extend(std::mem::take(&mut result.item.core.provider_ids));
        let genres = std::mem::take(&mut result.item.genres);
        let studios = std::mem::take(&mut result.item.studios);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: None,
                    // A lower-priority provider must not replace genres that
                    // the preferred provider already supplied.
                    genres: replace_data.then_some(genres),
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
        if (replace_data || item.name.as_deref().is_none_or(str::is_empty))
            && let Some(name) = result
                .item
                .core
                .name
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        if replace_data
            || item
                .overview
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            item.overview = std::mem::take(&mut result.item.core.overview)
                .filter(|value| !value.trim().is_empty());
        }
        if replace_data || item.official_rating.is_none() {
            item.official_rating = std::mem::take(&mut result.item.official_rating);
        }
        if replace_data || item.production_year.is_none() {
            item.production_year = result.item.production_year;
        }
        if let Some(date) = release_date
            && let Some(premiere_date) = omdb_date_to_utc(date)
            && (replace_data || item.premiere_date.is_none())
        {
            item.premiere_date = Some(premiere_date);
        }
        item.data = Some(omdb_extra_data(
            item.data.as_ref(),
            language,
            website,
            &result,
            replace_data,
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
    replace_data: bool,
) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if (replace_data || !object.contains_key("OriginalTitle"))
        && let Some(original_title) = result
            .item
            .core
            .name
            .as_deref()
            .filter(|value| !value.is_empty())
    {
        object.insert("OriginalTitle".to_owned(), json!(original_title));
    }
    if (replace_data || !object.contains_key("OriginalLanguage"))
        && let Some(language) = language.and_then(first_list_value)
    {
        object.insert("OriginalLanguage".to_owned(), Value::String(language));
    }
    if (replace_data || !object.contains_key("HomePageUrl"))
        && let Some(website) = website.filter(|value| !value.is_empty())
    {
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

fn provider_ids(data: Option<&Value>) -> Option<BTreeMap<String, String>> {
    Some(
        data?
            .get("ProviderIds")?
            .as_object()?
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.to_owned(), value.to_owned()))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_factory_reuses_transport_and_keeps_keys_per_client() {
        let factory = OmdbClientFactory::with_base_url("https://omdb.invalid/");
        let default_key = factory.client("");
        let configured_key = factory.client("configured-key");

        assert!(Arc::ptr_eq(&default_key.http, &configured_key.http));
        assert!(Arc::ptr_eq(&default_key.base_url, &configured_key.base_url));
        assert_eq!(default_key.api_key, DEFAULT_OMDB_API_KEY);
        assert_eq!(configured_key.api_key, "configured-key");
    }

    #[test]
    fn provider_ids_preserve_identifiers_from_earlier_providers() {
        let data = json!({
            "ProviderIds": {
                "Tmdb": "152044",
                "Imdb": "tt2194724"
            }
        });

        let mut merged = provider_ids(Some(&data)).expect("provider ids");
        merged.extend(BTreeMap::from([
            ("Imdb".to_owned(), "tt2194724".to_owned()),
            ("Tvdb".to_owned(), "1234".to_owned()),
        ]));

        assert_eq!(merged.get("Tmdb").map(String::as_str), Some("152044"));
        assert_eq!(merged.get("Imdb").map(String::as_str), Some("tt2194724"));
        assert_eq!(merged.get("Tvdb").map(String::as_str), Some("1234"));
    }

    #[test]
    fn lower_priority_result_preserves_localized_extra_fields() {
        let existing = json!({
            "OriginalTitle": "首选标题",
            "OriginalLanguage": "zh",
            "HomePageUrl": "https://preferred.invalid/"
        });
        let mut result = MetadataResult::default();
        result.item.core.name = Some("English title".to_owned());

        let merged = omdb_extra_data(
            Some(&existing),
            Some("English, French".to_owned()),
            Some("https://omdb.invalid/".to_owned()),
            &result,
            false,
        );

        assert_eq!(merged["OriginalTitle"], "首选标题");
        assert_eq!(merged["OriginalLanguage"], "zh");
        assert_eq!(merged["HomePageUrl"], "https://preferred.invalid/");

        let replaced = omdb_extra_data(
            Some(&existing),
            Some("English, French".to_owned()),
            Some("https://omdb.invalid/".to_owned()),
            &result,
            true,
        );
        assert_eq!(replaced["OriginalTitle"], "English title");
        assert_eq!(replaced["OriginalLanguage"], "English");
        assert_eq!(replaced["HomePageUrl"], "https://omdb.invalid/");
    }
}
