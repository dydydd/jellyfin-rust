use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository, ScoredBaseItem,
    ScoredBaseItemPage,
    entities::{base_item, user},
};
use jellyfin_model::UserPolicy;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    HydratedBaseItem, ItemTypeRegistry, LocalizationService, LyricManager, LyricProvider,
    LyricSearchRequest, UserError, UserService,
};

#[derive(Debug, Error)]
pub enum UserLibraryError {
    #[error("target user not found")]
    UserNotFound,
    #[error("library item not found")]
    ItemNotFound,
    #[error("the authenticated user cannot access this user's library")]
    Forbidden,
    #[error("lyrics not found")]
    LyricsNotFound,
    #[error("stored user policy is invalid")]
    InvalidPolicy(#[source] serde_json::Error),
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelatedItemKind {
    Intro,
    ThemeSong,
    ThemeVideo,
    LocalTrailer,
    SpecialFeature,
}

/// Coordinates user authorization with PostgreSQL-backed library hierarchy
/// queries used by the user-library endpoints.
#[derive(Clone)]
pub struct UserLibraryService {
    users: UserService,
    items: BaseItemRepository,
    item_types: ItemTypeRegistry,
    localization: LocalizationService,
    lyrics: LyricManager,
}

impl UserLibraryService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: DatabaseConnection,
        item_types: ItemTypeRegistry,
    ) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database),
            item_types,
            localization: LocalizationService,
            lyrics: LyricManager::default(),
        }
    }

    /// Replaces the remote lyric providers used by search and download.
    #[must_use]
    pub fn with_lyric_providers(
        mut self,
        providers: Vec<std::sync::Arc<dyn LyricProvider>>,
    ) -> Self {
        self.lyrics = LyricManager::new(providers);
        self
    }

    /// Ensures that server initialization has exactly one stable user root.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when `PostgreSQL` cannot load or create it.
    pub async fn ensure_user_root(&self) -> Result<base_item::Model, UserLibraryError> {
        Ok(self.items.ensure_user_root().await?)
    }

    /// Loads the user root after validating target-user access.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn root(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<base_item::Model, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.ensure_user_root().await
    }

    /// Loads one library item after validating target-user access.
    ///
    /// A nil item identifier retains Jellyfin's legacy root-folder behavior.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        if item_id.is_nil() {
            return self.ensure_user_root().await;
        }
        let mut query = BaseItemQuery {
            ids: vec![item_id],
            ..BaseItemQuery::default()
        };
        self.apply_user_policy(&mut query, target_user_id).await?;
        let page = self.hydrate_page(self.items.query(&query).await?);
        page.items
            .into_iter()
            .next()
            .ok_or(UserLibraryError::ItemNotFound)
    }

    /// Queries a target user's persisted library with PostgreSQL-side filters,
    /// count, and pagination.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn query_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: BaseItemQuery,
    ) -> Result<BaseItemPage, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.apply_user_policy(&mut query, target_user_id).await?;
        query.user_id = Some(target_user_id);
        if query.parent_id.is_none() && query.ids.is_empty() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.hydrate_page(self.items.query(&query).await?))
    }

    /// Searches a target user's library with official score ordering.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn search_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: BaseItemQuery,
    ) -> Result<ScoredBaseItemPage, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.apply_user_policy(&mut query, target_user_id).await?;
        query.user_id = Some(target_user_id);
        let page = self.items.search(&query).await?;
        Ok(ScoredBaseItemPage {
            items: page
                .items
                .into_iter()
                .filter_map(|scored| {
                    self.item_types
                        .hydrate(scored.item)
                        .map(|item| ScoredBaseItem {
                            item: item.into_model(),
                            score: scored.score,
                        })
                })
                .collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    /// Queries resumable items using the target user's real `PostgreSQL`
    /// playback rows, preserving most-recent-play order after item filters.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn resume_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: BaseItemQuery,
    ) -> Result<BaseItemPage, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        self.apply_user_policy(&mut query, target_user_id).await?;
        query.recursive = true;
        query.is_virtual_item = Some(false);
        if query.parent_id.is_none() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.hydrate_page(self.items.query_resumable(target_user_id, &query).await?))
    }

    /// Queries the next unwatched episode for each eligible series.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn next_up(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        parent_id: Option<Uuid>,
        enable_rewatching: bool,
        enable_resumable: bool,
        next_up_date_cutoff: Option<DateTime<Utc>>,
        start_index: u64,
        limit: Option<u64>,
        enable_total_record_count: bool,
    ) -> Result<BaseItemPage, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut query = BaseItemQuery {
            parent_id,
            recursive: true,
            include_item_types: vec!["Episode".to_owned()],
            is_virtual_item: Some(false),
            user_id: Some(target_user_id),
            enable_total_record_count: Some(enable_total_record_count),
            ..BaseItemQuery::default()
        };
        self.apply_user_policy(&mut query, target_user_id).await?;
        let page = self
            .items
            .next_up(
                target_user_id,
                &query,
                enable_rewatching,
                enable_resumable,
                next_up_date_cutoff,
                start_index,
                limit,
            )
            .await?;
        Ok(self.hydrate_page(page))
    }

    /// Loads related items from the persisted closure-table subtree.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn related_items(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        kind: RelatedItemKind,
    ) -> Result<Vec<base_item::Model>, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let item = self.load_item(item_id).await?;
        let descendants = self.items.descendants(item.id).await?;
        Ok(descendants
            .into_iter()
            .filter_map(|entry| self.item_types.hydrate(entry.item))
            .map(HydratedBaseItem::into_model)
            .filter(|candidate| related_item_matches(candidate, kind))
            .collect())
    }

    /// Loads additional video parts referenced by a stacked-video item.
    ///
    /// Jellyfin persists additional parts as paths on the primary video. The
    /// persisted Rust model stores that compatible contract in item metadata
    /// under `AdditionalParts`.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or persistence errors.
    pub async fn additional_parts(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Vec<base_item::Model>, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let item = self.load_item(item_id).await?;
        let Some(item_type) = self.item_types.resolve(&item.item_type) else {
            return Ok(Vec::new());
        };
        if !is_video_item_type(item_type.name()) {
            return Ok(Vec::new());
        }

        let paths = additional_part_paths(item.data.as_ref());
        Ok(self
            .items
            .by_paths(&paths)
            .await?
            .into_iter()
            .filter_map(|item| self.item_types.hydrate(item))
            .filter(|item| is_video_item_type(item.item_type().name()))
            .map(HydratedBaseItem::into_model)
            .collect())
    }

    /// Loads embedded lyric data after validating the user and item.
    ///
    /// # Errors
    ///
    /// Returns `LyricsNotFound` when the item has no persisted lyrics.
    pub async fn lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Value, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let item = self.load_item(item_id).await?;
        metadata_value(item.data.as_ref(), &["Lyrics", "lyrics"])
            .cloned()
            .ok_or(UserLibraryError::LyricsNotFound)
    }

    /// Searches configured remote lyric providers for an audio item.
    ///
    /// # Errors
    ///
    /// Returns not-found when the item is missing or is not an audio item.
    pub async fn remote_lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<Vec<Value>, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let item = self.load_item(item_id).await?;
        if !item.item_type.eq_ignore_ascii_case("Audio") {
            return Err(UserLibraryError::ItemNotFound);
        }
        let request = LyricSearchRequest {
            song_name: item.name.clone(),
            album_name: metadata_string(item.data.as_ref(), &["Album"]),
            artist_names: metadata_string_list(item.data.as_ref(), &["Artists"]),
            album_artist_names: metadata_string_list(item.data.as_ref(), &["AlbumArtists"]),
            duration_ticks: item.runtime_ticks,
        };
        Ok(self
            .lyrics
            .search(&request)
            .into_iter()
            .filter_map(|result| serde_json::to_value(result).ok())
            .collect())
    }

    /// Downloads a lyric from a configured remote provider and stores it.
    ///
    /// # Errors
    ///
    /// Returns not-found when the item is missing, is not audio, or no remote
    /// provider can resolve the lyric id.
    pub async fn download_remote_lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        lyric_id: &str,
    ) -> Result<Value, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut item = self.load_item(item_id).await?;
        if !item.item_type.eq_ignore_ascii_case("Audio") {
            return Err(UserLibraryError::ItemNotFound);
        }
        let Some(lyric_file) = self.lyrics.get_lyrics(lyric_id) else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        let Some(format) = lyric_file
            .name
            .rsplit_once('.')
            .map(|(_, extension)| extension)
        else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        let Some(lyrics) = LyricManager::parse_lyrics(format, &lyric_file.content) else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(serde_json::Map::default()));
        }
        if let Some(Value::Object(object)) = item.data.as_mut() {
            object.insert("Lyrics".to_owned(), lyrics.clone());
            object.remove("lyrics");
        }
        self.items.update(item).await?;
        Ok(lyrics)
    }

    /// Returns parsed remote lyrics without attaching them to the item.
    ///
    /// # Errors
    ///
    /// Returns [`UserLibraryError::LyricsNotFound`] when no provider can
    /// resolve `lyric_id` or the payload cannot be parsed.
    pub fn get_remote_lyrics(&self, lyric_id: &str) -> Result<Value, UserLibraryError> {
        let Some(lyric_file) = self.lyrics.get_lyrics(lyric_id) else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        let Some(format) = lyric_file
            .name
            .rsplit_once('.')
            .map(|(_, extension)| extension)
        else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        LyricManager::parse_lyrics(format, &lyric_file.content)
            .ok_or(UserLibraryError::LyricsNotFound)
    }

    /// Saves parsed lyric metadata on an audio item.
    ///
    /// # Errors
    ///
    /// Returns not-found when the item is missing or is not an audio item.
    pub async fn save_lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        lyrics: Value,
    ) -> Result<Value, UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut item = self.load_item(item_id).await?;
        if !item.item_type.eq_ignore_ascii_case("Audio") {
            return Err(UserLibraryError::ItemNotFound);
        }
        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(serde_json::Map::default()));
        }
        if let Some(Value::Object(object)) = item.data.as_mut() {
            object.insert("Lyrics".to_owned(), lyrics.clone());
            object.remove("lyrics");
        }
        self.items.update(item).await?;
        Ok(lyrics)
    }

    /// Deletes embedded lyric metadata from an audio item.
    ///
    /// # Errors
    ///
    /// Returns not-found when the item is missing or is not an audio item.
    pub async fn delete_lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<(), UserLibraryError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut item = self.load_item(item_id).await?;
        if !item.item_type.eq_ignore_ascii_case("Audio") {
            return Err(UserLibraryError::ItemNotFound);
        }
        let Some(data) = item.data.as_mut().and_then(Value::as_object_mut) else {
            return Ok(());
        };
        data.remove("Lyrics");
        data.remove("lyrics");
        self.items.update(item).await?;
        Ok(())
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), UserLibraryError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(UserLibraryError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(UserLibraryError::Forbidden);
        }
        Ok(())
    }

    async fn apply_user_policy(
        &self,
        query: &mut BaseItemQuery,
        target_user_id: Uuid,
    ) -> Result<(), UserLibraryError> {
        let user = self.users.get(target_user_id).await?;
        let policy: UserPolicy =
            serde_json::from_value(user.policy.clone()).map_err(UserLibraryError::InvalidPolicy)?;
        query.blocked_tags = policy.blocked_tags;
        query.allowed_tags = policy.allowed_tags;
        query.enabled_folders = policy.enabled_folders;
        query.enable_all_folders = policy.enable_all_folders;
        query.blocked_media_folders = policy.blocked_media_folders;
        if let Some(maximum) = policy.max_parental_rating {
            query.allowed_official_ratings = self
                .localization
                .parental_ratings("US")
                .into_iter()
                .filter(|rating| rating.value.is_none_or(|value| value <= maximum))
                .map(|rating| rating.name)
                .collect();
        }
        Ok(())
    }

    async fn load_item(&self, item_id: Uuid) -> Result<base_item::Model, UserLibraryError> {
        if item_id.is_nil() {
            return self.ensure_user_root().await;
        }
        self.items
            .get(item_id)
            .await?
            .and_then(|item| self.item_types.hydrate(item))
            .map(HydratedBaseItem::into_model)
            .ok_or(UserLibraryError::ItemNotFound)
    }

    fn hydrate_page(&self, mut page: BaseItemPage) -> BaseItemPage {
        page.items = page
            .items
            .into_iter()
            .filter_map(|item| self.item_types.hydrate(item))
            .map(HydratedBaseItem::into_model)
            .collect();
        page
    }
}

fn related_item_matches(item: &base_item::Model, kind: RelatedItemKind) -> bool {
    let extra_type =
        metadata_value(item.data.as_ref(), &["ExtraType", "extra_type"]).and_then(Value::as_str);
    match kind {
        RelatedItemKind::Intro => {
            metadata_value(item.data.as_ref(), &["IsIntro", "is_intro"])
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || metadata_value(item.data.as_ref(), &["Relation", "relation"])
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("Intro"))
        }
        RelatedItemKind::LocalTrailer => {
            extra_type.is_some_and(|value| value.eq_ignore_ascii_case("Trailer"))
        }
        RelatedItemKind::ThemeSong => {
            extra_type.is_some_and(|value| value.eq_ignore_ascii_case("ThemeSong"))
        }
        RelatedItemKind::ThemeVideo => {
            extra_type.is_some_and(|value| value.eq_ignore_ascii_case("ThemeVideo"))
        }
        RelatedItemKind::SpecialFeature => extra_type.is_some_and(is_display_extra_type),
    }
}

fn is_display_extra_type(value: &str) -> bool {
    [
        "Unknown",
        "BehindTheScenes",
        "Clip",
        "DeletedScene",
        "Interview",
        "Sample",
        "Scene",
        "Featurette",
        "Short",
    ]
    .iter()
    .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn is_video_item_type(value: &str) -> bool {
    ["Video", "Movie", "Episode", "MusicVideo", "Trailer"]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn additional_part_paths(data: Option<&Value>) -> Vec<String> {
    metadata_value(data, &["AdditionalParts", "additional_parts"])
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn metadata_value<'a>(data: Option<&'a Value>, keys: &[&str]) -> Option<&'a Value> {
    let object = data?.as_object()?;
    keys.iter().find_map(|key| object.get(*key))
}

fn metadata_string(data: Option<&Value>, keys: &[&str]) -> Option<String> {
    metadata_value(data, keys)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn metadata_string_list(data: Option<&Value>, keys: &[&str]) -> Vec<String> {
    match metadata_value(data, keys) {
        Some(Value::String(value)) if !value.is_empty() => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item_with_data(data: Value) -> base_item::Model {
        base_item::Model {
            id: Uuid::new_v4(),
            item_type: "Video".to_owned(),
            data: Some(data),
            path: None,
            parent_id: None,
            top_parent_id: None,
            name: None,
            clean_name: None,
            sort_name: None,
            media_type: None,
            overview: None,
            official_rating: None,
            index_number: None,
            parent_index_number: None,
            production_year: None,
            premiere_date: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
            primary_version_id: None,
            series_id: None,
            season_id: None,
            series_presentation_unique_key: None,
            date_created: chrono::Utc::now(),
            date_modified: chrono::Utc::now(),
            row_version: 1,
        }
    }

    #[test]
    fn relation_metadata_matches_official_extra_groups() {
        assert!(related_item_matches(
            &item_with_data(json!({ "IsIntro": true })),
            RelatedItemKind::Intro
        ));
        assert!(related_item_matches(
            &item_with_data(json!({ "ExtraType": "Trailer" })),
            RelatedItemKind::LocalTrailer
        ));
        assert!(related_item_matches(
            &item_with_data(json!({ "ExtraType": "Featurette" })),
            RelatedItemKind::SpecialFeature
        ));
        assert!(!related_item_matches(
            &item_with_data(json!({ "ExtraType": "ThemeVideo" })),
            RelatedItemKind::SpecialFeature
        ));
    }

    #[test]
    fn additional_part_paths_use_official_metadata_key() {
        assert_eq!(
            additional_part_paths(Some(&json!({
                "AdditionalParts": [" /media/part2.mkv ", "", null, "/media/part3.mkv"]
            }))),
            ["/media/part2.mkv", "/media/part3.mkv"]
        );
        assert_eq!(
            additional_part_paths(Some(&json!({
                "additional_parts": ["/media/lowercase.mkv"]
            }))),
            ["/media/lowercase.mkv"]
        );
    }
}
