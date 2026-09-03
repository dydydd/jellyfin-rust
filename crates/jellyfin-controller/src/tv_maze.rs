use std::{collections::BTreeMap, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ItemValueError, ItemValueRepository,
    entities::{base_item, item_value::ItemValueType},
};
use jellyfin_model::RemoteSearchResult;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const TV_MAZE_API_BASE_URL: &str = "https://api.tvmaze.com";
const TV_MAZE_PROVIDER_NAME: &str = "TVMaze";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum TvMazeProviderError {
    #[error("TVMaze provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("TVMaze provider returned an invalid response")]
    Json,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Clone)]
pub(crate) struct TvMazeClient {
    http: reqwest::Client,
    base_url: String,
}

impl TvMazeClient {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_base_url(TV_MAZE_API_BASE_URL)
    }

    #[must_use]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: builder.build().unwrap_or_else(|error| {
                tracing::error!(%error, "could not configure the TVMaze HTTP client");
                reqwest::Client::new()
            }),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    pub(crate) async fn search(
        &self,
        name: &str,
    ) -> Result<Vec<RemoteSearchResult>, TvMazeProviderError> {
        let response = self
            .get_json::<Vec<TvMazeSearchResult>>("/search/shows", &[("q", name)])
            .await?;
        Ok(response
            .into_iter()
            .map(|result| show_to_remote_result(result.show))
            .collect())
    }

    pub(crate) async fn show(&self, id: i64) -> Result<TvMazeShow, TvMazeProviderError> {
        self.get_json(&format!("/shows/{id}"), &[]).await
    }

    pub(crate) async fn lookup(
        &self,
        key: &str,
        value: &str,
    ) -> Result<Option<TvMazeShow>, TvMazeProviderError> {
        let response = self
            .http
            .get(format!("{}/lookup/shows", self.base_url))
            .query(&[(key, value)])
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        response
            .error_for_status()?
            .json()
            .await
            .map_err(|_| TvMazeProviderError::Json)
    }

    async fn get_json<T>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, TvMazeProviderError>
    where
        T: serde::de::DeserializeOwned,
    {
        self.http
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .map_err(|_| TvMazeProviderError::Json)
    }
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_TVMAZE_PROXY",
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

/// Fetches series metadata from `TVMaze` and merges it into library items.
pub struct TvMazeMetadataProvider {
    client: TvMazeClient,
    items: std::sync::Arc<BaseItemRepository>,
    values: std::sync::Arc<ItemValueRepository>,
    updates: std::sync::Arc<ItemUpdateRepository>,
}

impl TvMazeMetadataProvider {
    #[must_use]
    pub fn new(
        items: std::sync::Arc<BaseItemRepository>,
        values: std::sync::Arc<ItemValueRepository>,
        updates: std::sync::Arc<ItemUpdateRepository>,
    ) -> Self {
        Self {
            client: TvMazeClient::new(),
            items,
            values,
            updates,
        }
    }

    /// Refreshes one series item.
    ///
    /// # Errors
    ///
    /// Returns a provider or persistence error when the lookup fails.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, TvMazeProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        if item.item_type != "Series" {
            return Ok(false);
        }
        let Some(show) = self.fetch_show(&item).await? else {
            return Ok(false);
        };
        self.apply(item.id, show).await?;
        Ok(true)
    }

    async fn fetch_show(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<TvMazeShow>, TvMazeProviderError> {
        if let Some(id) =
            provider_id(item.data.as_ref(), "TvMaze").and_then(|id| id.parse::<i64>().ok())
        {
            return self.client.show(id).await.map(Some);
        }
        for (key, provider) in [("thetvdb", "Tvdb"), ("imdb", "Imdb")] {
            if let Some(id) = provider_id(item.data.as_ref(), provider)
                && let Some(show) = self.client.lookup(key, &id).await?
            {
                return Ok(Some(show));
            }
        }
        let search = self
            .client
            .search(item.name.as_deref().unwrap_or_default())
            .await?;
        if let Some(id) = search
            .into_iter()
            .next()
            .and_then(|mut result| result.provider_ids.remove("TvMaze"))
            .and_then(|id| id.parse::<i64>().ok())
        {
            return self.client.show(id).await.map(Some);
        }
        Ok(None)
    }

    async fn apply(&self, item_id: Uuid, mut show: TvMazeShow) -> Result<(), TvMazeProviderError> {
        let provider_ids = show_provider_ids(&show);
        let genres = std::mem::take(&mut show.genres);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(genres.clone()),
                    genres: Some(genres),
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;
        if let Some(network) = show
            .network
            .as_ref()
            .and_then(|network| network.name.as_deref())
            .or_else(|| {
                show.web_channel
                    .as_ref()
                    .and_then(|network| network.name.as_deref())
            })
            .filter(|name| !name.is_empty())
        {
            self.values
                .link(item_id, ItemValueType::Studios, network)
                .await?;
        }

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(name) = show.name.as_deref().filter(|value| !value.is_empty()) {
            item.name = Some(name.to_owned());
            item.sort_name = Some(name.to_owned());
        }
        item.overview = show
            .summary
            .as_deref()
            .map(strip_html)
            .filter(|value| !value.is_empty());
        item.production_year = parse_year(show.premiered.as_deref());
        item.premiere_date = parse_date(show.premiered.as_deref());
        item.data = Some(show_extra_data(item.data.as_ref(), &show));
        self.items.update(item).await?;
        Ok(())
    }
}

fn show_to_remote_result(show: TvMazeShow) -> RemoteSearchResult {
    let provider_ids = show_provider_ids(&show).into_iter().collect();
    let image_url = show.image.and_then(|image| image.original.or(image.medium));
    let overview = show.summary.as_deref().map(strip_html);
    RemoteSearchResult {
        name: show.name,
        r#type: Some("Series".to_owned()),
        provider_ids,
        production_year: parse_year(show.premiered.as_deref()),
        premiere_date: parse_date(show.premiered.as_deref()),
        image_url,
        search_provider_name: Some(TV_MAZE_PROVIDER_NAME.to_owned()),
        overview,
        ..RemoteSearchResult::default()
    }
}

fn show_provider_ids(show: &TvMazeShow) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::from([("TvMaze".to_owned(), show.id.to_string())]);
    if let Some(tvdb_id) = show.externals.thetvdb {
        ids.insert("Tvdb".to_owned(), tvdb_id.to_string());
    }
    if let Some(imdb_id) = show
        .externals
        .imdb
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        ids.insert("Imdb".to_owned(), imdb_id.to_owned());
    }
    if let Some(tvrage_id) = show.externals.tvrage {
        ids.insert("TvRage".to_owned(), tvrage_id.to_string());
    }
    ids
}

fn parse_year(value: Option<&str>) -> Option<i32> {
    value?
        .get(..4)
        .and_then(|year| year.parse::<i32>().ok())
        .filter(|year| *year > 1850)
}

fn parse_date(value: Option<&str>) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(value?, "%Y-%m-%d").ok()?;
    Some(DateTime::from_naive_utc_and_offset(
        date.and_hms_opt(0, 0, 0)?,
        Utc,
    ))
}

fn strip_html(value: &str) -> String {
    let mut stripped = String::new();
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !stripped.chars().last().is_some_and(char::is_whitespace) {
                    stripped.push(' ');
                }
            }
            _ if !in_tag => stripped.push(character),
            _ => {}
        }
    }
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn show_extra_data(existing: Option<&Value>, show: &TvMazeShow) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(name) = show.name.as_deref().filter(|value| !value.is_empty()) {
        object.insert("OriginalTitle".to_owned(), json!(name));
    }
    if let Some(language) = show.language.as_deref().filter(|value| !value.is_empty()) {
        object.insert("OriginalLanguage".to_owned(), json!(language));
    }
    if let Some(ended) = show.ended.as_deref().filter(|value| !value.is_empty()) {
        object.insert("EndDate".to_owned(), json!(ended));
    }
    if let Some(status) = show.status.as_deref().filter(|value| !value.is_empty()) {
        object.insert("Status".to_owned(), json!(normalize_status(status)));
    }
    if let Some(network) = show
        .network
        .as_ref()
        .and_then(|network| network.name.as_deref())
        .or_else(|| {
            show.web_channel
                .as_ref()
                .and_then(|network| network.name.as_deref())
        })
        .filter(|value| !value.is_empty())
    {
        object.insert("Network".to_owned(), json!(network));
    }
    if let Some(site) = show
        .official_site
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("OfficialSite".to_owned(), json!(site));
    }
    if let Some(rating) = show.rating.average {
        object.insert("CommunityRating".to_owned(), json!(rating));
    }
    Value::Object(object)
}

fn normalize_status(value: &str) -> &str {
    match value {
        "Running" => "Continuing",
        other => other,
    }
}

fn provider_id(data: Option<&Value>, key: &str) -> Option<String> {
    data?
        .get("ProviderIds")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TvMazeSearchResult {
    show: TvMazeShow,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TvMazeShow {
    pub(crate) id: i64,
    pub(crate) name: Option<String>,
    pub(crate) genres: Vec<String>,
    pub(crate) summary: Option<String>,
    pub(crate) premiered: Option<String>,
    pub(crate) ended: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) language: Option<String>,
    pub(crate) image: Option<TvMazeImage>,
    pub(crate) network: Option<TvMazeNetwork>,
    pub(crate) web_channel: Option<TvMazeNetwork>,
    pub(crate) rating: TvMazeRating,
    pub(crate) externals: TvMazeExternals,
    pub(crate) official_site: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TvMazeImage {
    pub(crate) medium: Option<String>,
    pub(crate) original: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TvMazeNetwork {
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TvMazeRating {
    pub(crate) average: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct TvMazeExternals {
    pub(crate) thetvdb: Option<i64>,
    pub(crate) imdb: Option<String>,
    pub(crate) tvrage: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_map_tv_maze_fields() {
        let show = TvMazeShow {
            id: 82,
            name: Some("Game of Thrones".to_owned()),
            genres: vec!["Drama".to_owned(), "Adventure".to_owned()],
            summary: Some("<p>A great story.</p>".to_owned()),
            premiered: Some("2011-04-17".to_owned()),
            image: Some(TvMazeImage {
                medium: Some("https://example.test/medium.jpg".to_owned()),
                original: Some("https://example.test/original.jpg".to_owned()),
            }),
            externals: TvMazeExternals {
                thetvdb: Some(121_361),
                imdb: Some("tt0944947".to_owned()),
                tvrage: Some(24493),
            },
            rating: TvMazeRating { average: Some(8.9) },
            ..TvMazeShow::default()
        };

        let result = show_to_remote_result(show);
        assert_eq!(result.name.as_deref(), Some("Game of Thrones"));
        assert_eq!(result.provider_ids["TvMaze"], "82");
        assert_eq!(result.provider_ids["Tvdb"], "121361");
        assert_eq!(result.provider_ids["Imdb"], "tt0944947");
        assert_eq!(result.provider_ids["TvRage"], "24493");
        assert_eq!(result.production_year, Some(2011));
        assert_eq!(result.overview.as_deref(), Some("A great story."));
    }

    #[test]
    fn html_summary_is_collapsed_to_plain_text() {
        assert_eq!(
            strip_html("<p>Hello <b>brave</b> new world.</p>"),
            "Hello brave new world."
        );
    }

    #[test]
    fn status_is_normalized_for_jellyfin_writers() {
        assert_eq!(normalize_status("Running"), "Continuing");
        assert_eq!(normalize_status("Ended"), "Ended");
    }
}
