use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use jellyfin_data::entities::base_item;
use thiserror::Error;
use tracing::warn;

const DEFAULT_ITEM_TYPES: &[(&str, &str)] = &[
    (
        "AggregateFolder",
        "MediaBrowser.Controller.Entities.AggregateFolder",
    ),
    ("Audio", "MediaBrowser.Controller.Entities.Audio.Audio"),
    ("AudioBook", "MediaBrowser.Controller.Entities.AudioBook"),
    (
        "BasePluginFolder",
        "MediaBrowser.Controller.Entities.BasePluginFolder",
    ),
    ("Book", "MediaBrowser.Controller.Entities.Book"),
    ("BoxSet", "MediaBrowser.Controller.Entities.Movies.BoxSet"),
    ("Channel", "MediaBrowser.Controller.Channels.Channel"),
    (
        "CollectionFolder",
        "MediaBrowser.Controller.Entities.CollectionFolder",
    ),
    ("Episode", "MediaBrowser.Controller.Entities.TV.Episode"),
    ("Folder", "MediaBrowser.Controller.Entities.Folder"),
    ("Genre", "MediaBrowser.Controller.Entities.Genre"),
    ("Movie", "MediaBrowser.Controller.Entities.Movies.Movie"),
    (
        "LiveTvChannel",
        "MediaBrowser.Controller.LiveTv.LiveTvChannel",
    ),
    (
        "LiveTvProgram",
        "MediaBrowser.Controller.LiveTv.LiveTvProgram",
    ),
    (
        "MusicAlbum",
        "MediaBrowser.Controller.Entities.Audio.MusicAlbum",
    ),
    (
        "MusicArtist",
        "MediaBrowser.Controller.Entities.Audio.MusicArtist",
    ),
    (
        "MusicGenre",
        "MediaBrowser.Controller.Entities.Audio.MusicGenre",
    ),
    ("MusicVideo", "MediaBrowser.Controller.Entities.MusicVideo"),
    ("Person", "MediaBrowser.Controller.Entities.Person"),
    ("Photo", "MediaBrowser.Controller.Entities.Photo"),
    ("PhotoAlbum", "MediaBrowser.Controller.Entities.PhotoAlbum"),
    ("Playlist", "MediaBrowser.Controller.Playlists.Playlist"),
    (
        "PlaylistsFolder",
        "Emby.Server.Implementations.Playlists.PlaylistsFolder",
    ),
    ("Season", "MediaBrowser.Controller.Entities.TV.Season"),
    ("Series", "MediaBrowser.Controller.Entities.TV.Series"),
    ("Studio", "MediaBrowser.Controller.Entities.Studio"),
    ("Trailer", "MediaBrowser.Controller.Entities.Trailer"),
    (
        "UserRootFolder",
        "MediaBrowser.Controller.Entities.UserRootFolder",
    ),
    ("UserView", "MediaBrowser.Controller.Entities.UserView"),
    ("Video", "MediaBrowser.Controller.Entities.Video"),
    ("Year", "MediaBrowser.Controller.Entities.Year"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownItemType {
    name: Arc<str>,
}

impl KnownItemType {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HydratedBaseItem {
    model: base_item::Model,
    item_type: KnownItemType,
}

impl HydratedBaseItem {
    #[must_use]
    pub const fn model(&self) -> &base_item::Model {
        &self.model
    }

    #[must_use]
    pub const fn item_type(&self) -> &KnownItemType {
        &self.item_type
    }

    #[must_use]
    pub fn into_model(self) -> base_item::Model {
        self.model
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ItemTypeRegistrationError {
    #[error("item type names cannot be empty or surrounded by whitespace")]
    InvalidName,
    #[error("persisted item type name '{0}' is already registered")]
    DuplicatePersistedName(String),
}

/// Extensible mapping from persisted CLR-style names to controller item kinds.
///
/// Lookups are deliberately case-sensitive because persisted type names are
/// serialization identifiers, not user-entered labels. Clones share newly
/// registered plugin types.
#[derive(Debug, Clone)]
pub struct ItemTypeRegistry {
    types: Arc<RwLock<HashMap<Arc<str>, KnownItemType>>>,
}

impl Default for ItemTypeRegistry {
    fn default() -> Self {
        let registry = Self::empty();
        for &(short_name, qualified_name) in DEFAULT_ITEM_TYPES {
            registry
                .register(short_name, [qualified_name])
                .expect("default item type names must be unique and valid");
        }
        registry
    }
}

impl ItemTypeRegistry {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            types: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Registers a canonical short name and any persisted aliases atomically.
    ///
    /// # Errors
    ///
    /// Returns an error for empty, padded, or already registered names. No
    /// names are inserted when any name in the registration is invalid.
    pub fn register<I, S>(
        &self,
        canonical_name: impl Into<String>,
        persisted_aliases: I,
    ) -> Result<(), ItemTypeRegistrationError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let canonical_name = Arc::<str>::from(canonical_name.into());
        let names = std::iter::once(Arc::clone(&canonical_name))
            .chain(
                persisted_aliases
                    .into_iter()
                    .map(|name| Arc::<str>::from(name.into())),
            )
            .collect::<Vec<_>>();
        if names.iter().any(|name| !valid_type_name(name)) {
            return Err(ItemTypeRegistrationError::InvalidName);
        }

        let mut types = self
            .types
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut unique_names = names;
        unique_names.sort_unstable();
        unique_names.dedup();
        if let Some(duplicate_index) = unique_names
            .iter()
            .position(|name| types.contains_key(name.as_ref()))
        {
            return Err(ItemTypeRegistrationError::DuplicatePersistedName(
                unique_names.swap_remove(duplicate_index).to_string(),
            ));
        }

        for name in unique_names {
            types.insert(
                name,
                KnownItemType {
                    name: Arc::clone(&canonical_name),
                },
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn resolve(&self, persisted_name: &str) -> Option<KnownItemType> {
        self.types
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(persisted_name)
            .map(|item_type| KnownItemType {
                name: Arc::clone(&item_type.name),
            })
    }

    /// Hydrates a raw persistence model only when its serialization type is
    /// currently registered.
    #[must_use]
    pub fn hydrate(&self, model: base_item::Model) -> Option<HydratedBaseItem> {
        let item_type = self.resolve(&model.item_type);
        let Some(item_type) = item_type else {
            warn!(
                item_id = %model.id,
                item_type = %model.item_type,
                "Skipping persisted item with unknown type; this may indicate a removed plugin or database corruption"
            );
            return None;
        };
        Some(HydratedBaseItem { model, item_type })
    }
}

fn valid_type_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name
}
