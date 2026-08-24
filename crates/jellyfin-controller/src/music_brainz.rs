use std::{collections::BTreeMap, time::Duration};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError,
};
use jellyfin_model::MetadataProvider;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const MUSIC_BRAINZ_API_BASE_URL: &str = "https://musicbrainz.org/ws/2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum MusicBrainzProviderError {
    #[error("MusicBrainz provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("MusicBrainz provider returned an invalid response")]
    Json,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
}

#[derive(Clone)]
struct MusicBrainzClient {
    http: reqwest::Client,
    base_url: String,
}

impl MusicBrainzClient {
    #[must_use]
    fn new() -> Self {
        Self::with_base_url(MUSIC_BRAINZ_API_BASE_URL)
    }

    #[must_use]
    fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: builder
                .default_headers(
                    std::iter::once((
                        reqwest::header::USER_AGENT,
                        reqwest::header::HeaderValue::from_static(
                            "jellyfin-rust/0.1 (https://github.com/jellyfin/jellyfin)",
                        ),
                    ))
                    .collect(),
                )
                .build()
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "could not configure the MusicBrainz HTTP client");
                    reqwest::Client::new()
                }),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    async fn get_json<T>(&self, endpoint: &str) -> Result<T, MusicBrainzProviderError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self
            .http
            .get(format!("{}{endpoint}", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        response
            .json()
            .await
            .map_err(|_| MusicBrainzProviderError::Json)
    }
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_MUSICBRAINZ_PROXY",
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

/// Fetches `MusicBrainz` artist and release-group metadata for music items.
pub struct MusicBrainzMetadataProvider {
    client: MusicBrainzClient,
    items: BaseItemRepository,
    updates: ItemUpdateRepository,
}

impl MusicBrainzMetadataProvider {
    #[must_use]
    pub fn new(items: BaseItemRepository, updates: ItemUpdateRepository) -> Self {
        Self {
            client: MusicBrainzClient::new(),
            items,
            updates,
        }
    }

    /// Refreshes one `MusicArtist` or `MusicAlbum` item.
    ///
    /// # Errors
    ///
    /// Returns a provider or persistence error when the lookup fails.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, MusicBrainzProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        match item.item_type.as_str() {
            "MusicArtist" => {
                let Some(id) = provider_id(item.data.as_ref(), MetadataProvider::MusicBrainzArtist)
                else {
                    return Ok(false);
                };
                let artist = self
                    .client
                    .get_json::<ArtistResponse>(&format!(
                        "/artist/{id}?inc=url-rels+tags+annotation&fmt=json"
                    ))
                    .await?;
                self.apply_artist(item.id, &artist).await?;
                Ok(true)
            }
            "MusicAlbum" => {
                let Some(id) = provider_id(
                    item.data.as_ref(),
                    MetadataProvider::MusicBrainzReleaseGroup,
                ) else {
                    return Ok(false);
                };
                let album = self
                    .client
                    .get_json::<ReleaseGroupResponse>(&format!(
                        "/release-group/{id}?inc=artists+releases+url-rels+tags+annotation&fmt=json"
                    ))
                    .await?;
                self.apply_album(item.id, &album).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn apply_artist(
        &self,
        item_id: Uuid,
        artist: &ArtistResponse,
    ) -> Result<(), MusicBrainzProviderError> {
        let genres = artist.tags.iter().map(|tag| tag.name.clone()).collect();
        let provider_ids = BTreeMap::from([(
            MetadataProvider::MusicBrainzArtist.as_str().to_owned(),
            artist.id.clone(),
        )]);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    tags: Some(genres),
                    provider_ids: Some(provider_ids),
                    ..ItemMetadataPatch::default()
                },
            )
            .await?;

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        if let Some(overview) = annotation_text(artist.annotation.as_ref())
            .filter(|overview| !overview.trim().is_empty())
        {
            item.overview = Some(overview);
        }
        item.data = Some(artist_data(item.data.as_ref(), artist));
        self.items.update(item).await?;
        Ok(())
    }

    async fn apply_album(
        &self,
        item_id: Uuid,
        album: &ReleaseGroupResponse,
    ) -> Result<(), MusicBrainzProviderError> {
        let genres = album.tags.iter().map(|tag| tag.name.clone()).collect();
        let provider_ids = BTreeMap::from([(
            MetadataProvider::MusicBrainzReleaseGroup
                .as_str()
                .to_owned(),
            album.id.clone(),
        )]);
        self.updates
            .update(
                item_id,
                ItemMetadataPatch {
                    genres: Some(genres),
                    provider_ids: Some(provider_ids),
                    ..ItemMetadataPatch::default()
                },
            )
            .await?;

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        item.production_year = album
            .first_release_date
            .as_deref()
            .and_then(|date| date.get(..4).and_then(|year| year.parse::<i32>().ok()));
        if let Some(overview) = annotation_text(album.annotation.as_ref())
            .filter(|overview| !overview.trim().is_empty())
        {
            item.overview = Some(overview);
        }
        self.items.update(item).await?;
        Ok(())
    }
}

fn artist_data(existing: Option<&Value>, artist: &ArtistResponse) -> Value {
    let mut object = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(name) = artist
        .disambiguation
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("Disambiguation".to_owned(), json!(name));
    }
    Value::Object(object)
}

fn annotation_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("content"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        Value::Array(values) => values.iter().find_map(|value| annotation_text(Some(value))),
        _ => None,
    }
}

fn provider_id(data: Option<&Value>, provider: MetadataProvider) -> Option<String> {
    data?
        .get("ProviderIds")?
        .get(provider.as_str())?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Default, Deserialize)]
struct ArtistResponse {
    id: String,
    disambiguation: Option<String>,
    annotation: Option<Value>,
    tags: Vec<Tag>,
}

#[derive(Debug, Default, Deserialize)]
struct ReleaseGroupResponse {
    id: String,
    first_release_date: Option<String>,
    annotation: Option<Value>,
    tags: Vec<Tag>,
}

#[derive(Debug, Default, Deserialize)]
struct Tag {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_text_accepts_object_and_array_shapes() {
        assert_eq!(
            annotation_text(Some(&json!({ "text": "Artist notes" }))),
            Some("Artist notes".to_owned())
        );
        assert_eq!(
            annotation_text(Some(&json!([{ "content": "Album notes" }]))),
            Some("Album notes".to_owned())
        );
        assert_eq!(annotation_text(Some(&json!(null))), None);
    }

    #[test]
    fn provider_id_reads_the_requested_musicbrainz_key() {
        let data = json!({
            "ProviderIds": {
                "MusicBrainzArtist": "artist-id",
                "MusicBrainzReleaseGroup": "release-id"
            }
        });
        assert_eq!(
            provider_id(Some(&data), MetadataProvider::MusicBrainzArtist).as_deref(),
            Some("artist-id")
        );
        assert_eq!(
            provider_id(Some(&data), MetadataProvider::MusicBrainzReleaseGroup).as_deref(),
            Some("release-id")
        );
    }
}
