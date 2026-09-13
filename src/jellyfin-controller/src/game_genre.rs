use std::path::PathBuf;

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueCounts, ItemValueError, ItemValueInfo,
    ItemValueQuery, ItemValueRepository,
    entities::{base_item, item_value, user},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserService};

#[derive(Debug, Error)]
pub enum GameGenreError {
    #[error("target user not found")]
    UserNotFound,
    #[error("game genre not found")]
    NotFound,
    #[error("the authenticated user cannot access this user's game genres")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameGenre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
    pub counts: ItemValueCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameGenrePage {
    pub genres: Vec<GameGenre>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameGenreDetail {
    pub item: base_item::Model,
    pub item_count: u64,
    pub counts: ItemValueCounts,
}

/// Implements Emby's removed GameGenre item-by-name behavior without adding
/// Game routes to Jellyfin's unprefixed protocol tree.
#[derive(Clone)]
pub struct GameGenreService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
    item_by_name: ItemByNameService,
}

impl GameGenreService {
    #[must_use]
    pub fn with_item_by_name_service(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_by_name: ItemByNameService,
    ) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            item_values: ItemValueRepository::new(database),
            item_by_name,
        }
    }

    pub fn set_item_by_name_directories(
        &self,
        program_data_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) {
        self.item_by_name
            .set_directories(program_data_directory, internal_metadata_directory);
    }

    /// Resolves an ordinary name by the official deterministic GameGenre path
    /// and uses the legacy `&`, `/`, `?` slug lookup order for hyphenated names.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
        query: ItemValueQuery,
    ) -> Result<GameGenreDetail, GameGenreError> {
        self.authorize_target_user(authenticated_user, target_user_id)?;
        self.get_authorized(name, query).await
    }

    /// Resolves a detail after the protocol adapter has already authorized its
    /// optional target user. This separate entry point lets administrator API
    /// keys retain Emby's user-less global behavior.
    pub async fn get_authorized(
        &self,
        name: &str,
        mut query: ItemValueQuery,
    ) -> Result<GameGenreDetail, GameGenreError> {
        // Emby created these entities in its post-scan task. The Rust adapter
        // performs the equivalent bounded reconciliation lazily, so detail and
        // image requests must not depend on a list request having run first.
        self.item_by_name.reconcile_game_genres_once().await?;
        let item = self
            .item_by_name
            .resolve(ItemByNameKind::GameGenre, name)
            .await?
            .ok_or(GameGenreError::NotFound)?;
        let Some(name) = item.name.as_deref() else {
            return Ok(empty_detail(item));
        };
        let Some(value) = self
            .item_values
            .get_normalized(item_value::ItemValueType::Genre, name)
            .await?
        else {
            return Ok(empty_detail(item));
        };
        query.search_term = Some(value.value);
        let candidate = self
            .item_values
            .query_values(item_value::ItemValueType::Genre, &query)
            .await?
            .values
            .into_iter()
            .find(|candidate| candidate.id == value.item_value_id);
        Ok(GameGenreDetail {
            item,
            item_count: candidate.as_ref().map_or(0, |value| {
                value.item_count.saturating_add(value.counts.game_count)
            }),
            counts: candidate.map_or_else(ItemValueCounts::default, |value| value.counts),
        })
    }

    /// Loads the persisted GameGenre entity that owns image metadata.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, GameGenreError> {
        self.item_by_name.reconcile_game_genres_once().await?;
        self.item_by_name
            .resolve(ItemByNameKind::GameGenre, name)
            .await
            .map_err(Into::into)
    }

    /// Lists GameGenre entities after the API has authorized and applied the
    /// target user's policy to the contributing media query.
    pub async fn list_authorized(
        &self,
        query: ItemValueQuery,
    ) -> Result<GameGenrePage, GameGenreError> {
        self.item_by_name.reconcile_game_genres_once().await?;
        let mut query = self.scope_parent(query).await?;
        query.by_name_item_type = Some("GameGenre".to_owned());
        let page = self
            .item_values
            .query_persisted_item_by_name_values(
                item_value::ItemValueType::Genre,
                "GameGenre",
                &query,
            )
            .await?;
        Ok(GameGenrePage {
            genres: page.values.into_iter().map(GameGenre::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn scope_parent(
        &self,
        mut query: ItemValueQuery,
    ) -> Result<ItemValueQuery, GameGenreError> {
        let Some(parent_id) = query.parent_id else {
            return Ok(query);
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(GameGenreError::NotFound)?;
        if parent.is_folder {
            query.recursive = true;
        } else {
            query.parent_id = None;
            query.recursive = false;
            query.ids = vec![parent_id];
        }
        Ok(query)
    }

    /// Validates a target user for callers that have not already done so.
    pub async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), GameGenreError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(GameGenreError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        self.authorize_target_user(authenticated_user, target_user_id)
    }

    fn authorize_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), GameGenreError> {
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(GameGenreError::Forbidden);
        }
        Ok(())
    }
}

fn empty_detail(item: base_item::Model) -> GameGenreDetail {
    GameGenreDetail {
        item,
        item_count: 0,
        counts: ItemValueCounts::default(),
    }
}

impl From<ItemValueInfo> for GameGenre {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            // The shared Jellyfin count deliberately excludes removed Game
            // items. Add the Emby-only bucket back only on this protocol model.
            item_count: value.item_count.saturating_add(value.counts.game_count),
            counts: value.counts,
        }
    }
}
