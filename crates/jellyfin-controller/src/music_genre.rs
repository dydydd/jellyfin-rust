use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemValueError, ItemValueInfo, ItemValueQuery,
    ItemValueRepository,
    entities::{base_item, item_value, user},
};
use sea_orm::DatabaseConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::{UserError, UserService};

#[derive(Debug, Error)]
pub enum MusicGenreError {
    #[error("target user not found")]
    UserNotFound,
    #[error("music genre not found")]
    NotFound,
    #[error("the authenticated user cannot access this user's music genres")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicGenre {
    pub id: Uuid,
    pub name: String,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicGenrePage {
    pub genres: Vec<MusicGenre>,
    pub total_record_count: u64,
    pub start_index: u64,
}

/// Resolves persisted music genres and coordinates optional target-user
/// authorization for the API.
#[derive(Clone)]
pub struct MusicGenreService {
    users: UserService,
    items: BaseItemRepository,
    item_values: ItemValueRepository,
}

impl MusicGenreService {
    #[must_use]
    pub fn new(database: DatabaseConnection) -> Self {
        Self {
            users: UserService::new(database.clone()),
            items: BaseItemRepository::new(database.clone()),
            item_values: ItemValueRepository::new(database),
        }
    }

    /// Resolves a music genre by exact, legacy slug, or normalized name.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<MusicGenre, MusicGenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let value = self
            .find_value(name)
            .await?
            .ok_or(MusicGenreError::NotFound)?;
        let linked_items = self
            .item_values
            .items_for_value(item_value::ItemValueType::Genre, &value.value)
            .await?;
        let item_count = linked_items
            .iter()
            .filter(|item| is_music_item(item))
            .count();
        if item_count == 0 {
            return Err(MusicGenreError::NotFound);
        }
        Ok(MusicGenre {
            id: value.item_value_id,
            name: value.value,
            item_count: u64::try_from(item_count).unwrap_or(u64::MAX),
        })
    }

    /// Lists music genres attached to filtered music items.
    ///
    /// # Errors
    ///
    /// Returns forbidden, user lookup, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: ItemValueQuery,
    ) -> Result<MusicGenrePage, MusicGenreError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let query = self.scope_music_query(query).await?;
        let page = self
            .item_values
            .query_values(item_value::ItemValueType::Genre, &query)
            .await?;
        Ok(MusicGenrePage {
            genres: page.values.into_iter().map(MusicGenre::from).collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn find_value(&self, name: &str) -> Result<Option<item_value::Model>, MusicGenreError> {
        let mut candidates = vec![name.to_owned()];
        if name.contains('-') {
            candidates
                .extend(['&', '/', '?'].map(|separator| name.replace('-', &separator.to_string())));
        }
        for candidate in &candidates {
            match self
                .item_values
                .get_exact(item_value::ItemValueType::Genre, candidate)
                .await
            {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) | Err(ItemValueError::InvalidValue) => {}
                Err(error) => return Err(error.into()),
            }
        }
        for candidate in candidates {
            match self
                .item_values
                .get_normalized(item_value::ItemValueType::Genre, &candidate)
                .await
            {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) | Err(ItemValueError::InvalidValue) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), MusicGenreError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(MusicGenreError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(MusicGenreError::Forbidden);
        }
        Ok(())
    }

    async fn scope_music_query(
        &self,
        mut query: ItemValueQuery,
    ) -> Result<ItemValueQuery, MusicGenreError> {
        if query.include_item_types.is_empty() {
            query.include_item_types = MUSIC_ITEM_TYPES.iter().map(ToString::to_string).collect();
        } else {
            query
                .include_item_types
                .retain(|item_type| is_music_item_type(item_type));
            if query.include_item_types.is_empty() {
                query
                    .include_item_types
                    .push("__jellyfin_no_music_item_type__".to_owned());
            }
        }
        let Some(parent_id) = query.parent_id else {
            return Ok(query);
        };
        let parent = self
            .items
            .get(parent_id)
            .await?
            .ok_or(MusicGenreError::NotFound)?;
        if parent.is_folder {
            query.recursive = true;
        } else {
            query.parent_id = None;
            query.recursive = false;
            query.ids = vec![parent_id];
        }
        Ok(query)
    }
}

impl From<ItemValueInfo> for MusicGenre {
    fn from(value: ItemValueInfo) -> Self {
        Self {
            id: value.id,
            name: value.value,
            item_count: value.item_count,
        }
    }
}

const MUSIC_ITEM_TYPES: [&str; 4] = ["Audio", "MusicVideo", "MusicAlbum", "MusicArtist"];

fn is_music_item(item: &base_item::Model) -> bool {
    is_music_item_type(&item.item_type)
}

fn is_music_item_type(candidate: &str) -> bool {
    MUSIC_ITEM_TYPES.iter().any(|item_type| {
        candidate.eq_ignore_ascii_case(item_type) || candidate.ends_with(&format!(".{item_type}"))
    })
}
