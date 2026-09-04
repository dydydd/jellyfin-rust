use std::{collections::HashMap, error::Error, fmt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod media_attachment;
mod media_stream;

pub use media_attachment::MediaAttachment;
pub use media_stream::{
    AudioSpatialFormat, MediaStream, MediaStreamType, SubtitleDeliveryMethod, VideoRange,
    VideoRangeType,
};

pub type ProviderIdMap = HashMap<String, String>;

/// Chapter metadata attached to an item or media source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChapterInfo {
    pub start_position_ticks: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    #[serde(with = "crate::serde_datetime::required")]
    pub image_date_modified: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tag: Option<String>,
}

impl Default for ChapterInfo {
    fn default() -> Self {
        Self {
            start_position_ticks: 0,
            name: None,
            image_path: None,
            image_date_modified: DateTime::<Utc>::UNIX_EPOCH,
            image_tag: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaUrl {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum ExtraType {
    #[default]
    Unknown = 0,
    Clip = 1,
    Trailer = 2,
    BehindTheScenes = 3,
    DeletedScene = 4,
    Interview = 5,
    Scene = 6,
    Sample = 7,
    ThemeSong = 8,
    ThemeVideo = 9,
    Featurette = 10,
    Short = 11,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[repr(i32)]
pub enum LocationType {
    #[default]
    FileSystem = 0,
    Remote = 1,
    Virtual = 2,
    Offline = 3,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MetadataField {
    #[default]
    Cast,
    Genres,
    ProductionLocations,
    Studios,
    Tags,
    Name,
    Overview,
    Runtime,
    OfficialRating,
}

/// The official person-kind enum used by `BaseItemPerson`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PersonKind {
    #[default]
    Unknown,
    Actor,
    Director,
    Composer,
    Writer,
    GuestStar,
    Producer,
    Conductor,
    Lyricist,
    Arranger,
    Engineer,
    Mixer,
    Remixer,
    Creator,
    Artist,
    AlbumArtist,
    Author,
    Illustrator,
    Penciller,
    Inker,
    Colorist,
    Letterer,
    CoverArtist,
    Editor,
    Translator,
    Narrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum MetadataProvider {
    Custom = 0,
    Imdb = 2,
    Tmdb = 3,
    Tvdb = 4,
    Tvcom = 5,
    TmdbCollection = 7,
    MusicBrainzAlbum = 8,
    MusicBrainzAlbumArtist = 9,
    MusicBrainzArtist = 10,
    MusicBrainzReleaseGroup = 11,
    Zap2It = 12,
    TvRage = 15,
    AudioDbArtist = 16,
    AudioDbAlbum = 17,
    MusicBrainzTrack = 18,
    TvMaze = 19,
    MusicBrainzRecording = 20,
}

impl MetadataProvider {
    pub const ALL: [Self; 17] = [
        Self::Custom,
        Self::Imdb,
        Self::Tmdb,
        Self::Tvdb,
        Self::Tvcom,
        Self::TmdbCollection,
        Self::MusicBrainzAlbum,
        Self::MusicBrainzAlbumArtist,
        Self::MusicBrainzArtist,
        Self::MusicBrainzReleaseGroup,
        Self::Zap2It,
        Self::TvRage,
        Self::AudioDbArtist,
        Self::AudioDbAlbum,
        Self::MusicBrainzTrack,
        Self::TvMaze,
        Self::MusicBrainzRecording,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Imdb => "Imdb",
            Self::Tmdb => "Tmdb",
            Self::Tvdb => "Tvdb",
            Self::Tvcom => "Tvcom",
            Self::TmdbCollection => "TmdbCollection",
            Self::MusicBrainzAlbum => "MusicBrainzAlbum",
            Self::MusicBrainzAlbumArtist => "MusicBrainzAlbumArtist",
            Self::MusicBrainzArtist => "MusicBrainzArtist",
            Self::MusicBrainzReleaseGroup => "MusicBrainzReleaseGroup",
            Self::Zap2It => "Zap2It",
            Self::TvRage => "TvRage",
            Self::AudioDbArtist => "AudioDbArtist",
            Self::AudioDbAlbum => "AudioDbAlbum",
            Self::MusicBrainzTrack => "MusicBrainzTrack",
            Self::TvMaze => "TvMaze",
            Self::MusicBrainzRecording => "MusicBrainzRecording",
        }
    }
}

impl fmt::Display for MetadataProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Checks whether a value has a plausible format for its provider.
///
/// Unknown provider names accept any non-blank value so plugin-owned IDs stay
/// extensible. Known providers follow Jellyfin's provider-specific rules.
#[must_use]
pub fn is_valid_provider_id(name: Option<&str>, value: Option<&str>) -> bool {
    let (Some(name), Some(value)) = (name, value) else {
        return false;
    };
    if name.trim().is_empty() || value.trim().is_empty() {
        return false;
    }

    let provider = MetadataProvider::ALL
        .iter()
        .copied()
        .find(|provider| provider.as_str().eq_ignore_ascii_case(name));
    match provider {
        Some(MetadataProvider::Imdb) => is_imdb_id(value),
        Some(
            MetadataProvider::Tmdb
            | MetadataProvider::TmdbCollection
            | MetadataProvider::AudioDbArtist
            | MetadataProvider::AudioDbAlbum,
        ) => {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<i32>().is_ok_and(|id| id > 0)
        }
        Some(
            MetadataProvider::MusicBrainzAlbum
            | MetadataProvider::MusicBrainzAlbumArtist
            | MetadataProvider::MusicBrainzArtist
            | MetadataProvider::MusicBrainzReleaseGroup
            | MetadataProvider::MusicBrainzTrack
            | MetadataProvider::MusicBrainzRecording,
        ) => Uuid::parse_str(value).is_ok(),
        _ => true,
    }
}

/// Trims and canonicalizes a valid provider-id pair.
#[must_use]
pub fn normalize_provider_id(name: Option<&str>, value: Option<&str>) -> Option<(String, String)> {
    let name = name?.trim();
    let value = value?.trim();
    if name.contains('=') || !is_valid_provider_id(Some(name), Some(value)) {
        return None;
    }
    let name = MetadataProvider::ALL
        .iter()
        .find(|provider| provider.as_str().eq_ignore_ascii_case(name))
        .map_or(name, |provider| provider.as_str());
    Some((name.to_owned(), value.to_owned()))
}

fn is_imdb_id(value: &str) -> bool {
    let digits = value.get(..2).map_or(value, |prefix| {
        if ["tt", "nm", "co", "ev", "ch", "ni"]
            .iter()
            .any(|expected| prefix.eq_ignore_ascii_case(expected))
        {
            &value[2..]
        } else {
            value
        }
    });
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Implemented by model entities that own a nullable provider-id dictionary.
pub trait HasProviderIds {
    fn provider_ids(&self) -> Option<&ProviderIdMap>;
    fn provider_ids_mut(&mut self) -> &mut Option<ProviderIdMap>;
}

/// Entity-local provider-id dictionary operations.
pub trait ProviderIdsExtensions: HasProviderIds {
    #[must_use]
    fn has_provider_id(&self, provider: MetadataProvider) -> bool {
        self.get_provider_id(provider).is_some()
    }

    /// # Errors
    ///
    /// Returns [`ProviderIdError::NullName`] when `name` is `None` and the
    /// entity has an initialized provider dictionary.
    fn has_provider_id_named(&self, name: Option<&str>) -> Result<bool, ProviderIdError> {
        Ok(self.get_provider_id_named(name)?.is_some())
    }

    #[must_use]
    fn get_provider_id(&self, provider: MetadataProvider) -> Option<&str> {
        get_from_map(self.provider_ids(), Some(provider.as_str())).ok()?
    }

    /// # Errors
    ///
    /// Returns [`ProviderIdError::NullName`] when `name` is `None` and the
    /// entity has an initialized provider dictionary.
    fn get_provider_id_named(&self, name: Option<&str>) -> Result<Option<&str>, ProviderIdError> {
        get_from_map(self.provider_ids(), name)
    }

    #[must_use]
    fn try_get_provider_id(&self, provider: MetadataProvider) -> Option<&str> {
        self.get_provider_id(provider)
    }

    /// Attempts to set a named provider id without returning validation errors.
    fn try_set_provider_id_named(&mut self, name: Option<&str>, value: Option<&str>) -> bool {
        let Some((name, value)) = normalize_provider_id(name, value) else {
            return false;
        };
        insert_provider_id(self.provider_ids_mut(), &name, &value);
        true
    }

    /// # Errors
    ///
    /// Returns an error when the provider id value is blank.
    fn set_provider_id(
        &mut self,
        provider: MetadataProvider,
        value: &str,
    ) -> Result<(), ProviderIdError> {
        self.set_provider_id_named(Some(provider.as_str()), Some(value))
    }

    /// # Errors
    ///
    /// Returns an error when the provider name or value is null, blank, or the
    /// name contains `=`.
    fn set_provider_id_named(
        &mut self,
        name: Option<&str>,
        value: Option<&str>,
    ) -> Result<(), ProviderIdError> {
        let name = name.ok_or(ProviderIdError::NullName)?;
        let value = value.ok_or(ProviderIdError::NullValue)?;
        if name.trim().is_empty() {
            return Err(ProviderIdError::EmptyName);
        }
        if value.trim().is_empty() {
            return Err(ProviderIdError::EmptyValue);
        }
        if name.contains('=') {
            return Err(ProviderIdError::InvalidName);
        }
        let Some((name, value)) = normalize_provider_id(Some(name), Some(value)) else {
            return Err(ProviderIdError::InvalidValue);
        };
        insert_provider_id(self.provider_ids_mut(), &name, &value);
        Ok(())
    }

    fn remove_provider_id(&mut self, provider: MetadataProvider) {
        remove_from_map(self.provider_ids_mut(), provider.as_str());
    }

    /// # Errors
    ///
    /// Returns an error when `name` is null or empty.
    fn remove_provider_id_named(&mut self, name: Option<&str>) -> Result<(), ProviderIdError> {
        let name = name.ok_or(ProviderIdError::NullName)?;
        if name.is_empty() {
            return Err(ProviderIdError::EmptyName);
        }
        remove_from_map(self.provider_ids_mut(), name);
        Ok(())
    }
}

impl<T: HasProviderIds + ?Sized> ProviderIdsExtensions for T {}

/// Compatibility entry point for the official null-instance behavior.
///
/// # Errors
///
/// Returns [`ProviderIdError::NullInstance`] when `instance` is `None`.
pub fn has_provider_id<T: HasProviderIds>(
    instance: Option<&T>,
    provider: MetadataProvider,
) -> Result<bool, ProviderIdError> {
    Ok(instance
        .ok_or(ProviderIdError::NullInstance)?
        .has_provider_id(provider))
}

/// Compatibility entry point for the official null-instance behavior.
///
/// # Errors
///
/// Returns [`ProviderIdError::NullInstance`] when `instance` is `None`.
pub fn get_provider_id<T: HasProviderIds>(
    instance: Option<&T>,
    provider: MetadataProvider,
) -> Result<Option<&str>, ProviderIdError> {
    Ok(instance
        .ok_or(ProviderIdError::NullInstance)?
        .get_provider_id(provider))
}

/// Compatibility entry point for the official null-instance behavior.
///
/// # Errors
///
/// Returns [`ProviderIdError::NullInstance`] when `instance` is `None`, or a
/// value-validation error from [`ProviderIdsExtensions::set_provider_id`].
pub fn set_provider_id<T: HasProviderIds>(
    instance: Option<&mut T>,
    provider: MetadataProvider,
    value: &str,
) -> Result<(), ProviderIdError> {
    instance
        .ok_or(ProviderIdError::NullInstance)?
        .set_provider_id(provider, value)
}

fn get_from_map<'a>(
    provider_ids: Option<&'a ProviderIdMap>,
    name: Option<&str>,
) -> Result<Option<&'a str>, ProviderIdError> {
    let Some(provider_ids) = provider_ids else {
        return Ok(None);
    };
    let name = name.ok_or(ProviderIdError::NullName)?;
    Ok(provider_ids
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty()))
}

fn insert_provider_id(provider_ids: &mut Option<ProviderIdMap>, name: &str, value: &str) {
    let provider_ids = provider_ids.get_or_insert_with(HashMap::new);
    if let Some(existing_key) = provider_ids
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned()
    {
        provider_ids.insert(existing_key, value.to_owned());
        return;
    }

    let canonical_name = MetadataProvider::ALL
        .iter()
        .find(|provider| provider.as_str().eq_ignore_ascii_case(name))
        .map_or(name, |provider| provider.as_str());
    provider_ids.insert(canonical_name.to_owned(), value.to_owned());
}

fn remove_from_map(provider_ids: &mut Option<ProviderIdMap>, name: &str) {
    let Some(provider_ids) = provider_ids else {
        return;
    };
    if let Some(key) = provider_ids
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned()
    {
        provider_ids.remove(&key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderIdError {
    NullInstance,
    NullName,
    EmptyName,
    NullValue,
    EmptyValue,
    InvalidName,
    InvalidValue,
}

impl fmt::Display for ProviderIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NullInstance => "provider-id entity cannot be null",
            Self::NullName => "provider name cannot be null",
            Self::EmptyName => "provider name cannot be blank",
            Self::NullValue => "provider id cannot be null",
            Self::EmptyValue => "provider id cannot be blank",
            Self::InvalidName => "provider name cannot contain '='",
            Self::InvalidValue => "provider id has an invalid format for its provider",
        })
    }
}

impl Error for ProviderIdError {}
