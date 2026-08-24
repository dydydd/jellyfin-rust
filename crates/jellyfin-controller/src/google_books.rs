use std::{collections::HashMap, time::Duration};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, entities::base_item,
};
use jellyfin_model::RemoteSearchResult;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const GOOGLE_BOOKS_API_BASE_URL: &str = "https://www.googleapis.com/books/v1";
const GOOGLE_BOOKS_PROVIDER_NAME: &str = "Google Books";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum GoogleBooksProviderError {
    #[error("Google Books provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Google Books provider returned an invalid response")]
    Json,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
}

#[derive(Clone)]
pub(crate) struct GoogleBooksClient {
    http: reqwest::Client,
    base_url: String,
}

impl GoogleBooksClient {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_base_url(GOOGLE_BOOKS_API_BASE_URL)
    }

    #[must_use]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: builder.build().unwrap_or_else(|error| {
                tracing::error!(%error, "could not configure the Google Books HTTP client");
                reqwest::Client::new()
            }),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    pub(crate) async fn search(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Vec<RemoteSearchResult>, GoogleBooksProviderError> {
        let query_text = year.map_or_else(|| name.to_owned(), |year| format!("{name} {year}"));
        let query = vec![("q", query_text.as_str()), ("maxResults", "20")];
        let response = self
            .get_json::<GoogleBooksResponse>("/volumes", &query)
            .await?;
        Ok(response
            .items
            .into_iter()
            .map(book_search_to_remote_result)
            .collect())
    }

    pub(crate) async fn volume(
        &self,
        id: &str,
    ) -> Result<GoogleBookVolume, GoogleBooksProviderError> {
        self.get_json(&format!("/volumes/{id}"), &[]).await
    }

    async fn get_json<T>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, GoogleBooksProviderError>
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
            .map_err(|_| GoogleBooksProviderError::Json)
    }
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_GOOGLEBOOKS_PROXY",
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

/// Fetches book metadata from Google Books and merges it into library items.
pub struct GoogleBooksMetadataProvider {
    client: GoogleBooksClient,
    items: BaseItemRepository,
    updates: ItemUpdateRepository,
}

impl GoogleBooksMetadataProvider {
    #[must_use]
    pub fn new(items: BaseItemRepository, updates: ItemUpdateRepository) -> Self {
        Self {
            client: GoogleBooksClient::new(),
            items,
            updates,
        }
    }

    /// Refreshes one book item.
    ///
    /// # Errors
    ///
    /// Returns a provider or persistence error when the lookup fails.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, GoogleBooksProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        if !matches!(item.item_type.as_str(), "Book" | "AudioBook") {
            return Ok(false);
        }
        let Some(volume) = self.fetch_volume(&item).await? else {
            return Ok(false);
        };
        self.apply(item.id, &volume).await?;
        Ok(true)
    }

    async fn fetch_volume(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<GoogleBookVolume>, GoogleBooksProviderError> {
        if let Some(id) =
            provider_id(item.data.as_ref(), "GoogleBooks").filter(|id| !id.trim().is_empty())
        {
            return self.client.volume(&id).await.map(Some);
        }
        if let Some(isbn) =
            provider_id(item.data.as_ref(), "ISBN").filter(|id| !id.trim().is_empty())
        {
            let query = format!("isbn:{isbn}");
            if let Some(volume) = self
                .client
                .search(&query, None)
                .await?
                .into_iter()
                .next()
                .and_then(|result| result.provider_ids.get("GoogleBooks").cloned())
            {
                return self.client.volume(&volume).await.map(Some);
            }
        }
        if let Some(id) = self
            .client
            .search(
                item.name.as_deref().unwrap_or_default(),
                item.production_year,
            )
            .await?
            .into_iter()
            .next()
            .and_then(|result| result.provider_ids.get("GoogleBooks").cloned())
        {
            return self.client.volume(&id).await.map(Some);
        }
        Ok(None)
    }

    async fn apply(
        &self,
        item_id: Uuid,
        volume: &GoogleBookVolume,
    ) -> Result<(), GoogleBooksProviderError> {
        let info = &volume.volume_info;
        let mut provider_ids =
            std::collections::BTreeMap::from([("GoogleBooks".to_owned(), volume.id.clone())]);
        if let Some(isbn) = isbn_identifier(info) {
            provider_ids.insert("ISBN".to_owned(), isbn);
        }
        let categories = info
            .categories
            .iter()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(categories.clone()),
                    genres: Some(categories),
                    provider_ids: Some(provider_ids),
                },
            )
            .await?;

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(name) = full_title(info) {
            item.sort_name = Some(name.clone());
            item.name = Some(name);
        }
        item.overview = info
            .description
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        item.production_year = parse_year(info.published_date.as_deref());
        item.data = Some(book_extra_data(item.data.as_ref(), info));
        self.items.update(item).await?;
        Ok(())
    }
}

fn book_search_to_remote_result(volume: GoogleBookVolume) -> RemoteSearchResult {
    RemoteSearchResult {
        name: full_title(&volume.volume_info),
        r#type: Some("Book".to_owned()),
        provider_ids: HashMap::from([("GoogleBooks".to_owned(), volume.id)]),
        production_year: parse_year(volume.volume_info.published_date.as_deref()),
        image_url: volume
            .volume_info
            .image_links
            .and_then(|links| links.thumbnail)
            .filter(|url| !url.is_empty()),
        search_provider_name: Some(GOOGLE_BOOKS_PROVIDER_NAME.to_owned()),
        overview: volume.volume_info.description,
        ..RemoteSearchResult::default()
    }
}

fn full_title(info: &GoogleBookVolumeInfo) -> Option<String> {
    let title = info
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    let subtitle = info
        .subtitle
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    Some(subtitle.map_or_else(
        || title.to_owned(),
        |subtitle| format!("{title}: {subtitle}"),
    ))
}

fn parse_year(value: Option<&str>) -> Option<i32> {
    value?
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|year| year.parse::<i32>().ok())
        .filter(|year| (1000..=9999).contains(year))
}

fn isbn_identifier(info: &GoogleBookVolumeInfo) -> Option<String> {
    let isbn = |kind: &str| {
        info.industry_identifiers
            .iter()
            .filter(|identifier| {
                identifier
                    .r#type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(kind))
            })
            .find_map(|identifier| {
                identifier
                    .identifier
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
    };
    isbn("isbn_13").or_else(|| isbn("isbn_10"))
}

fn book_extra_data(existing: Option<&Value>, info: &GoogleBookVolumeInfo) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(title) = info.title.as_deref().filter(|value| !value.is_empty()) {
        object.insert("OriginalTitle".to_owned(), json!(title));
    }
    if !info.authors.is_empty() {
        object.insert("Authors".to_owned(), json!(info.authors));
    }
    if let Some(publisher) = info.publisher.as_deref().filter(|value| !value.is_empty()) {
        object.insert("Publisher".to_owned(), json!(publisher));
    }
    if let Some(page_count) = info.page_count {
        object.insert("PageCount".to_owned(), json!(page_count));
    }
    if let Some(language) = info.language.as_deref().filter(|value| !value.is_empty()) {
        object.insert("Language".to_owned(), json!(language));
    }
    Value::Object(object)
}

fn provider_id(data: Option<&Value>, key: &str) -> Option<String> {
    data?
        .get("ProviderIds")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GoogleBooksResponse {
    items: Vec<GoogleBookVolume>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GoogleBookVolume {
    pub(crate) id: String,
    pub(crate) volume_info: GoogleBookVolumeInfo,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GoogleBookVolumeInfo {
    pub(crate) title: Option<String>,
    pub(crate) subtitle: Option<String>,
    pub(crate) authors: Vec<String>,
    pub(crate) publisher: Option<String>,
    pub(crate) published_date: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) page_count: Option<i32>,
    pub(crate) categories: Vec<String>,
    pub(crate) language: Option<String>,
    pub(crate) image_links: Option<GoogleBookImageLinks>,
    pub(crate) industry_identifiers: Vec<GoogleBookIndustryIdentifier>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GoogleBookImageLinks {
    pub(crate) thumbnail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GoogleBookIndustryIdentifier {
    pub(crate) r#type: Option<String>,
    pub(crate) identifier: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_results_map_google_books_fields() {
        let volume = GoogleBookVolume {
            id: "abc123".to_owned(),
            volume_info: GoogleBookVolumeInfo {
                title: Some("The Lord of the Rings".to_owned()),
                subtitle: Some("The Fellowship of the Ring".to_owned()),
                authors: vec!["J. R. R. Tolkien".to_owned()],
                published_date: Some("1954-07-29".to_owned()),
                description: Some("A classic fantasy novel.".to_owned()),
                image_links: Some(GoogleBookImageLinks {
                    thumbnail: Some("https://example.test/cover.jpg".to_owned()),
                }),
                ..GoogleBookVolumeInfo::default()
            },
        };

        let result = book_search_to_remote_result(volume);
        assert_eq!(
            result.name.as_deref(),
            Some("The Lord of the Rings: The Fellowship of the Ring")
        );
        assert_eq!(result.provider_ids["GoogleBooks"], "abc123");
        assert_eq!(result.production_year, Some(1954));
        assert_eq!(result.r#type.as_deref(), Some("Book"));
    }

    #[test]
    fn isbn_identifier_prefers_isbn_13() {
        let info = GoogleBookVolumeInfo {
            industry_identifiers: vec![
                GoogleBookIndustryIdentifier {
                    r#type: Some("ISBN_10".to_owned()),
                    identifier: Some("0306406152".to_owned()),
                },
                GoogleBookIndustryIdentifier {
                    r#type: Some("ISBN_13".to_owned()),
                    identifier: Some("9780306406157".to_owned()),
                },
            ],
            ..GoogleBookVolumeInfo::default()
        };

        assert_eq!(isbn_identifier(&info).as_deref(), Some("9780306406157"));
    }

    #[test]
    fn year_parser_handles_common_google_books_shapes() {
        assert_eq!(parse_year(Some("2010-01-02")), Some(2010));
        assert_eq!(parse_year(Some("2010")), Some(2010));
        assert_eq!(parse_year(Some("c. 2010")), Some(2010));
        assert_eq!(parse_year(Some("unknown")), None);
    }
}
