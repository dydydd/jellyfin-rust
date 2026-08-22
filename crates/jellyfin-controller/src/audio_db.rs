use std::{collections::BTreeMap, time::Duration};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError,
    entities::base_item,
};
use jellyfin_model::MetadataProvider;
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const AUDIO_DB_API_BASE_URL: &str = "https://www.theaudiodb.com/api/v1/json/195003";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum AudioDbMetadataProviderError {
    #[error("TheAudioDB provider HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("TheAudioDB provider returned an invalid response")]
    Json,
    #[error("no MusicBrainz or TheAudioDB provider id is available")]
    NoProviderId,
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    Update(#[from] ItemUpdateStoreError),
}

#[derive(Clone)]
struct AudioDbClient {
    http: reqwest::Client,
    base_url: String,
}

impl AudioDbClient {
    #[must_use]
    fn new() -> Self {
        Self::with_base_url(AUDIO_DB_API_BASE_URL)
    }

    #[must_use]
    fn with_base_url(base_url: impl Into<String>) -> Self {
        let mut builder = reqwest::Client::builder().timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = proxy_from_environment() {
            builder = builder.proxy(proxy);
        }
        Self {
            http: builder.build().unwrap_or_else(|error| {
                tracing::error!(%error, "could not configure the AudioDB HTTP client");
                reqwest::Client::new()
            }),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    async fn get_json<T>(&self, endpoint: &str) -> Result<T, AudioDbMetadataProviderError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = self
            .http
            .get(format!("{}{endpoint}", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(response
            .json()
            .await
            .map_err(|_| AudioDbMetadataProviderError::Json)?)
    }
}

fn proxy_from_environment() -> Option<reqwest::Proxy> {
    [
        "JELLYFIN_AUDIODB_PROXY",
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

/// Fetches TheAudioDB artist and album metadata for music library items.
pub struct AudioDbMetadataProvider {
    client: AudioDbClient,
    items: BaseItemRepository,
    updates: ItemUpdateRepository,
}

impl AudioDbMetadataProvider {
    #[must_use]
    pub fn new(items: BaseItemRepository, updates: ItemUpdateRepository) -> Self {
        Self {
            client: AudioDbClient::new(),
            items,
            updates,
        }
    }

    /// Refreshes one `MusicArtist` or `MusicAlbum` item.
    ///
    /// # Errors
    ///
    /// Returns a provider or persistence error when the lookup fails.
    pub async fn refresh_item(&self, item_id: Uuid) -> Result<bool, AudioDbMetadataProviderError> {
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(false);
        };
        match item.item_type.as_str() {
            "MusicArtist" => {
                let Some(artist) = self.fetch_artist(&item).await? else {
                    return Ok(false);
                };
                self.apply_artist(item.id, &artist).await?;
                Ok(true)
            }
            "MusicAlbum" => {
                let Some(album) = self.fetch_album(&item).await? else {
                    return Ok(false);
                };
                self.apply_album(item.id, &album).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn fetch_artist(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<Artist>, AudioDbMetadataProviderError> {
        let audio_db_id = provider_id(item.data.as_ref(), MetadataProvider::AudioDbArtist);
        let music_brainz_id = provider_id(item.data.as_ref(), MetadataProvider::MusicBrainzArtist);
        let artists = match (audio_db_id, music_brainz_id) {
            (Some(id), _) => self
                .client
                .get_json::<ArtistRoot>(&format!("/artist.php?i={id}"))
                .await?
                .artists
                .unwrap_or_default(),
            (None, Some(id)) => self
                .client
                .get_json::<ArtistRoot>(&format!("/artist-mb.php?i={id}"))
                .await?
                .artists
                .unwrap_or_default(),
            (None, None) => return Ok(None),
        };
        Ok(artists.into_iter().next())
    }

    async fn fetch_album(
        &self,
        item: &base_item::Model,
    ) -> Result<Option<Album>, AudioDbMetadataProviderError> {
        let Some(release_group_id) =
            provider_id(item.data.as_ref(), MetadataProvider::MusicBrainzReleaseGroup)
        else {
            return Ok(None);
        };
        Ok(self
            .client
            .get_json::<AlbumRoot>(&format!("/album-mb.php?i={release_group_id}"))
            .await?
            .album
            .unwrap_or_default()
            .into_iter()
            .next())
    }

    async fn apply_artist(
        &self,
        item_id: Uuid,
        artist: &Artist,
    ) -> Result<(), AudioDbMetadataProviderError> {
        let mut provider_ids = BTreeMap::new();
        if let Some(id) = artist.id_artist.as_deref().filter(|value| !value.is_empty()) {
            provider_ids.insert(MetadataProvider::AudioDbArtist.as_str().to_owned(), id.to_owned());
        }
        if let Some(id) = artist
            .str_music_brainz_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            provider_ids.insert(
                MetadataProvider::MusicBrainzArtist.as_str().to_owned(),
                id.to_owned(),
            );
        }
        let mut genres = Vec::new();
        if let Some(genre) = artist
            .str_genre
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            genres.push(genre.to_owned());
        }
        if let Some(sub_genre) = artist
            .str_sub_genre
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            genres.push(sub_genre.to_owned());
        }
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

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        item.overview = artist
            .preferred_overview()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        item.production_year = artist.formed_year();
        item.data = Some(artist_extra_data(item.data.as_ref(), artist));
        self.items.update(item).await?;
        Ok(())
    }

    async fn apply_album(
        &self,
        item_id: Uuid,
        album: &Album,
    ) -> Result<(), AudioDbMetadataProviderError> {
        let mut provider_ids = BTreeMap::new();
        if let Some(id) = album.id_album.as_deref().filter(|value| !value.is_empty()) {
            provider_ids.insert(MetadataProvider::AudioDbAlbum.as_str().to_owned(), id.to_owned());
        }
        if let Some(id) = album.id_artist.as_deref().filter(|value| !value.is_empty()) {
            provider_ids.insert(
                MetadataProvider::AudioDbArtist.as_str().to_owned(),
                id.to_owned(),
            );
        }
        if let Some(id) = album
            .str_music_brainz_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            provider_ids.insert(
                MetadataProvider::MusicBrainzReleaseGroup.as_str().to_owned(),
                id.to_owned(),
            );
        }
        if let Some(id) = album
            .str_music_brainz_artist_id
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            provider_ids.insert(
                MetadataProvider::MusicBrainzAlbumArtist.as_str().to_owned(),
                id.to_owned(),
            );
        }
        let genres = album
            .str_genre
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|genre| vec![genre.to_owned()])
            .unwrap_or_default();
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

        let mut item = self
            .items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
        item.overview = album
            .preferred_overview()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        item.production_year = album
            .int_year_released
            .as_deref()
            .and_then(|year| year.trim().parse().ok());
        item.data = Some(album_extra_data(item.data.as_ref(), album));
        self.items.update(item).await?;
        Ok(())
    }
}

fn artist_extra_data(existing: Option<&Value>, artist: &Artist) -> Value {
    let mut object = metadata_object(existing);
    if let Some(website) = artist
        .str_website
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("HomePageUrl".to_owned(), json!(website));
    }
    if let Some(country) = artist
        .str_country
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("ProductionLocations".to_owned(), json!([country]));
    }
    Value::Object(object)
}

fn album_extra_data(existing: Option<&Value>, album: &Album) -> Value {
    let mut object = metadata_object(existing);
    if let Some(artist) = album
        .str_artist
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        object.insert("AlbumArtist".to_owned(), json!(artist));
    }
    Value::Object(object)
}

fn metadata_object(existing: Option<&Value>) -> serde_json::Map<String, Value> {
    existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn provider_id(data: Option<&Value>, provider: MetadataProvider) -> Option<String> {
    data?.get("ProviderIds")?
        .get(provider.as_str())?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug, Default, Deserialize)]
struct ArtistRoot {
    artists: Option<Vec<Artist>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artist {
    id_artist: Option<String>,
    str_genre: Option<String>,
    str_sub_genre: Option<String>,
    int_formed_year: Option<String>,
    str_country: Option<String>,
    str_biography: Option<String>,
    str_biography_en: Option<String>,
    str_biography_de: Option<String>,
    str_biography_fr: Option<String>,
    str_biography_nl: Option<String>,
    str_biography_ru: Option<String>,
    str_biography_it: Option<String>,
    str_biography_pt: Option<String>,
    str_website: Option<String>,
    str_music_brainz_id: Option<String>,
}

impl Artist {
    fn preferred_overview(&self) -> Option<&str> {
        let language = std::env::var("JELLYFIN_METADATA_LANGUAGE")
            .unwrap_or_else(|_| "en".to_owned());
        let localized = match language.as_str() {
            "de" => self.str_biography_de.as_deref(),
            "fr" => self.str_biography_fr.as_deref(),
            "nl" => self.str_biography_nl.as_deref(),
            "ru" => self.str_biography_ru.as_deref(),
            "it" => self.str_biography_it.as_deref(),
            language if language.starts_with("pt") => self.str_biography_pt.as_deref(),
            _ => None,
        };
        localized
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.str_biography_en
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| {
                self.str_biography
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
    }

    fn formed_year(&self) -> Option<i32> {
        self.int_formed_year
            .as_deref()
            .and_then(|year| year.trim().parse().ok())
    }
}

#[derive(Debug, Default, Deserialize)]
struct AlbumRoot {
    album: Option<Vec<Album>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Album {
    id_album: Option<String>,
    id_artist: Option<String>,
    str_artist: Option<String>,
    int_year_released: Option<String>,
    str_genre: Option<String>,
    str_description: Option<String>,
    str_description_en: Option<String>,
    str_description_de: Option<String>,
    str_description_fr: Option<String>,
    str_description_nl: Option<String>,
    str_description_ru: Option<String>,
    str_description_it: Option<String>,
    str_description_pt: Option<String>,
    str_music_brainz_id: Option<String>,
    str_music_brainz_artist_id: Option<String>,
}

impl Album {
    fn preferred_overview(&self) -> Option<&str> {
        let language = std::env::var("JELLYFIN_METADATA_LANGUAGE")
            .unwrap_or_else(|_| "en".to_owned());
        let localized = match language.as_str() {
            "de" => self.str_description_de.as_deref(),
            "fr" => self.str_description_fr.as_deref(),
            "nl" => self.str_description_nl.as_deref(),
            "ru" => self.str_description_ru.as_deref(),
            "it" => self.str_description_it.as_deref(),
            language if language.starts_with("pt") => self.str_description_pt.as_deref(),
            _ => None,
        };
        localized
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.str_description_en
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| {
                self.str_description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
    }
}
