use std::collections::BTreeMap;

use jellyfin_data::{BaseItemError, BaseItemRepository, entities::base_item};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

/// Errors that can occur during metadata provider operations.
#[derive(Debug, thiserror::Error)]
pub enum MetadataProviderError {
    #[error("metadata provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("TMDB API returned error: {0}")]
    TmdbApi(String),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error("no TMDB API key configured")]
    NoApiKey,
}

/// Fetches metadata from The Movie Database (TMDB) and updates the item.
pub struct TmdbMetadataProvider {
    api_key: String,
    http: reqwest::Client,
    items: BaseItemRepository,
}

impl TmdbMetadataProvider {
    #[must_use]
    pub fn new(api_key: String, items: BaseItemRepository) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
            items,
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
        let name = item.name.as_deref().unwrap_or("");
        let year = item.production_year;

        let movie_id = self.search_movie(name, year).await?;
        let Some(movie_id) = movie_id else {
            return Ok(false);
        };

        let details = self.movie_details(movie_id).await?;
        self.apply_movie_metadata(item.id, &details).await?;
        Ok(true)
    }

    async fn refresh_series(&self, item: &base_item::Model) -> Result<bool, MetadataProviderError> {
        let name = item.name.as_deref().unwrap_or("");
        let year = item.production_year;

        let tv_id = self.search_tv(name, year).await?;
        let Some(tv_id) = tv_id else {
            return Ok(false);
        };

        let details = self.tv_details(tv_id).await?;
        self.apply_tv_metadata(item.id, &details).await?;
        Ok(true)
    }

    async fn search_movie(&self, name: &str, year: Option<i32>) -> Result<Option<i64>, MetadataProviderError> {
        let mut query = vec![("query", name.to_owned()), ("language", "en-US".to_owned())];
        if let Some(y) = year {
            query.push(("year", y.to_string()));
        }
        let resp: TmdbSearchResponse = self
            .http
            .get("https://api.themoviedb.org/3/search/movie")
            .query(&query)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.results.into_iter().next().map(|r| r.id))
    }

    async fn search_tv(&self, name: &str, year: Option<i32>) -> Result<Option<i64>, MetadataProviderError> {
        let mut query = vec![("query", name.to_owned()), ("language", "en-US".to_owned())];
        if let Some(y) = year {
            query.push(("first_air_date_year", y.to_string()));
        }
        let resp: TmdbSearchResponse = self
            .http
            .get("https://api.themoviedb.org/3/search/tv")
            .query(&query)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.results.into_iter().next().map(|r| r.id))
    }

    async fn movie_details(&self, movie_id: i64) -> Result<TmdbMovieDetails, MetadataProviderError> {
        let append = "credits,release_dates,external_ids";
        let resp: TmdbMovieDetails = self
            .http
            .get(&format!("https://api.themoviedb.org/3/movie/{movie_id}"))
            .query(&[("append_to_response", append), ("language", "en-US")])
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    async fn tv_details(&self, tv_id: i64) -> Result<TmdbTvDetails, MetadataProviderError> {
        let append = "credits,content_ratings,external_ids";
        let resp: TmdbTvDetails = self
            .http
            .get(&format!("https://api.themoviedb.org/3/tv/{tv_id}"))
            .query(&[("append_to_response", append), ("language", "en-US")])
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    async fn apply_movie_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbMovieDetails,
    ) -> Result<(), MetadataProviderError> {
        let mut item = self.items.get(item_id).await?.ok_or(BaseItemError::NotFound)?;

        if let Some(overview) = &details.overview {
            if overview.len() > 10 {
                item.overview = Some(overview.clone());
            }
        }

        let mut provider_ids = extract_provider_ids(&item.data);
        provider_ids.insert("Tmdb".to_owned(), details.id.to_string());
        if let Some(imdb_id) = &details.external_ids.imdb_id {
            provider_ids.insert("Imdb".to_owned(), imdb_id.clone());
        }
        set_provider_ids(&mut item, provider_ids);

        if let Some(runtime) = details.runtime {
            item.runtime_ticks = Some((runtime as i64) * 60 * 10_000_000);
        }

        self.items.update(item).await?;
        Ok(())
    }

    async fn apply_tv_metadata(
        &self,
        item_id: Uuid,
        details: &TmdbTvDetails,
    ) -> Result<(), MetadataProviderError> {
        let mut item = self.items.get(item_id).await?.ok_or(BaseItemError::NotFound)?;

        if let Some(overview) = &details.overview {
            if overview.len() > 10 {
                item.overview = Some(overview.clone());
            }
        }

        let mut provider_ids = extract_provider_ids(&item.data);
        provider_ids.insert("Tmdb".to_owned(), details.id.to_string());
        if let Some(imdb_id) = &details.external_ids.imdb_id {
            provider_ids.insert("Imdb".to_owned(), imdb_id.clone());
        }
        set_provider_ids(&mut item, provider_ids);

        self.items.update(item).await?;
        Ok(())
    }
}

fn extract_provider_ids(data: &Option<Value>) -> BTreeMap<String, String> {
    data.as_ref()
        .and_then(|d| d.get("ProviderIds"))
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn set_provider_ids(item: &mut base_item::Model, ids: BTreeMap<String, String>) {
    let mut data = item.data.clone().unwrap_or(Value::Object(serde_json::Map::new()));
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "ProviderIds".to_owned(),
            Value::Object(ids.into_iter().map(|(k, v)| (k, Value::String(v))).collect()),
        );
    }
    item.data = Some(data);
}

// ---- TMDB API response types ----

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchResponse {
    results: Vec<TmdbSearchResult>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TmdbSearchResult {
    id: i64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TmdbMovieDetails {
    id: i64,
    overview: Option<String>,
    runtime: Option<i32>,
    external_ids: TmdbExternalIds,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TmdbTvDetails {
    id: i64,
    overview: Option<String>,
    external_ids: TmdbExternalIds,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TmdbExternalIds {
    imdb_id: Option<String>,
}
