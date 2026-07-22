use jellyfin_data::{
    BaseItemError, BaseItemPage, BaseItemQuery, BaseItemRepository,
    entities::{base_item, user},
};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

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
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelatedItemKind {
    Intro,
    LocalTrailer,
    SpecialFeature,
}

/// Coordinates user authorization with PostgreSQL-backed library hierarchy
/// queries used by the user-library endpoints.
#[derive(Clone)]
pub struct UserLibraryService {
    users: UserService,
    items: BaseItemRepository,
}

impl UserLibraryService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database),
        }
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
        self.load_item(item_id).await
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
        if query.parent_id.is_none() && query.ids.is_empty() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.items.query(&query).await?)
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
        query.recursive = true;
        query.is_virtual_item = Some(false);
        if query.parent_id.is_none() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.items.query_resumable(target_user_id, &query).await?)
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
            .map(|entry| entry.item)
            .filter(|candidate| related_item_matches(candidate, kind))
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

    async fn load_item(&self, item_id: Uuid) -> Result<base_item::Model, UserLibraryError> {
        if item_id.is_nil() {
            return self.ensure_user_root().await;
        }
        self.items
            .get(item_id)
            .await?
            .ok_or(UserLibraryError::ItemNotFound)
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

fn metadata_value<'a>(data: Option<&'a Value>, keys: &[&str]) -> Option<&'a Value> {
    let object = data?.as_object()?;
    keys.iter().find_map(|key| object.get(*key))
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
            index_number: None,
            parent_index_number: None,
            production_year: None,
            runtime_ticks: None,
            is_folder: false,
            is_virtual_item: false,
            presentation_unique_key: None,
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
}
