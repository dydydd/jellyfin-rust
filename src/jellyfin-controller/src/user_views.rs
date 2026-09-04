use jellyfin_data::{BaseItemError, BaseItemRepository, NewBaseItem, USER_ROOT_FOLDER_ID};
use jellyfin_model::{UserConfiguration, UserPolicy};
use md5::{Digest, Md5};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    UserError, UserService, VirtualFolder, VirtualFolderService, VirtualFolderServiceError,
};

#[derive(Debug, Error)]
pub enum UserViewManagerError {
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    VirtualFolder(#[from] VirtualFolderServiceError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error("stored user data is invalid: {0}")]
    InvalidUserData(#[source] serde_json::Error),
}

/// A library folder or generated user view returned by the view manager.
#[derive(Debug, Clone, PartialEq)]
pub struct UserViewItem {
    pub id: Uuid,
    pub name: String,
    pub collection_type: Option<String>,
    pub display_parent_id: Option<Uuid>,
    /// Physical collection folders whose direct children back this view.
    pub content_parent_ids: Vec<Uuid>,
    pub parent_id: Option<Uuid>,
    pub item_type: String,
    pub is_virtual_item: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserViewGroupingOption {
    pub id: Uuid,
    pub name: String,
}

/// Reimplements Jellyfin's `UserViewManager` against persisted virtual folders.
#[derive(Clone)]
pub struct UserViewManagerService {
    users: UserService,
    folders: VirtualFolderService,
    items: BaseItemRepository,
}

impl UserViewManagerService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self::with_services(
            UserService::new(std::sync::Arc::clone(&database)),
            VirtualFolderService::new(std::sync::Arc::clone(&database)),
            BaseItemRepository::new(database),
        )
    }

    #[must_use]
    pub const fn with_services(
        users: UserService,
        folders: VirtualFolderService,
        items: BaseItemRepository,
    ) -> Self {
        Self {
            users,
            folders,
            items,
        }
    }

    /// Builds the user's media-library views in official ordering.
    ///
    /// # Errors
    ///
    /// Returns user, folder, or stored-data errors.
    pub async fn list(
        &self,
        user_id: Uuid,
        preset_views: &[String],
        include_hidden: bool,
    ) -> Result<Vec<UserViewItem>, UserViewManagerError> {
        let user = self.users.get(user_id).await?;
        let config = parse_config(user.preferences)?;
        let policy = parse_policy(user.policy)?;
        let folders = self.visible_folders(&policy, include_hidden).await?;
        let mut grouped_folders = Vec::new();
        let mut list = Vec::new();

        for folder in folders {
            if is_user_specific(folder.collection_type.as_deref()) {
                list.push(named_user_view(user_id, folder));
                continue;
            }
            let collection_type = folder.collection_type.as_deref();
            if is_eligible_for_grouping(collection_type)
                && config.grouped_folders.contains(&folder.id)
            {
                grouped_folders.push(folder);
                continue;
            }
            if preset_matches(collection_type, preset_views) {
                list.push(shadow_user_view(folder));
            } else {
                list.push(collection_folder_view_owned(folder));
            }
        }

        for (name, collection_type) in [("Movies", "movies"), ("TvShows", "tvshows")] {
            let parents = grouped_folders
                .iter()
                .filter(|folder| {
                    folder
                        .collection_type
                        .as_deref()
                        .is_none_or(|kind| kind.eq_ignore_ascii_case(collection_type))
                })
                .cloned()
                .collect();
            add_grouped_view(&mut list, user_id, parents, name, preset_views);
        }

        list.retain(|view| {
            !config.my_media_excludes.contains(&view.id)
                && !view
                    .display_parent_id
                    .is_some_and(|id| config.my_media_excludes.contains(&id))
        });
        for view in &list {
            if view.is_virtual_item {
                self.ensure_persisted_view(view, user_id).await?;
            }
        }
        sort_views(&mut list, &config);
        Ok(list)
    }

    /// Resolves a generated grouped view to its configured physical folders.
    ///
    /// `None` means the identifier is not one of this user's grouped movie or
    /// television views. The returned folders have already been restricted by
    /// the user's policy and current virtual-folder visibility.
    ///
    /// # Errors
    ///
    /// Returns user, folder, or stored-data errors.
    pub async fn grouped_content_parent_ids(
        &self,
        user_id: Uuid,
        view_id: Uuid,
    ) -> Result<Option<Vec<Uuid>>, UserViewManagerError> {
        let user = self.users.get(user_id).await?;
        let config = parse_config(user.preferences)?;
        let policy = parse_policy(user.policy)?;

        for (name, collection_type) in [("Movies", "movies"), ("TvShows", "tvshows")] {
            if grouped_user_view(user_id, name, collection_type, Vec::new()).id != view_id {
                continue;
            }
            if config.my_media_excludes.contains(&view_id) {
                return Ok(Some(Vec::new()));
            }
            let mut parent_ids = self
                .visible_folders(&policy, false)
                .await?
                .into_iter()
                .filter(|folder| config.grouped_folders.contains(&folder.id))
                .filter(|folder| {
                    folder
                        .collection_type
                        .as_deref()
                        .is_none_or(|kind| kind.eq_ignore_ascii_case(collection_type))
                })
                .map(|folder| folder.id)
                .collect::<Vec<_>>();
            parent_ids.sort_unstable();
            parent_ids.dedup();
            return Ok(Some(parent_ids));
        }
        Ok(None)
    }

    async fn ensure_persisted_view(
        &self,
        view: &UserViewItem,
        user_id: Uuid,
    ) -> Result<(), UserViewManagerError> {
        self.items.ensure_user_root().await?;
        let data = serde_json::json!({
            "ViewType": view.collection_type,
            "DisplayParentId": view.display_parent_id.map(|id| id.simple().to_string()),
            "UserId": user_id.simple().to_string(),
            "ForcedSortName": view.name,
        });
        if let Some(mut existing) = self.items.get(view.id).await? {
            let mut changed = false;
            if existing.item_type != view.item_type {
                existing.item_type = view.item_type.clone();
                changed = true;
            }
            if existing.parent_id != view.parent_id {
                existing.parent_id = view.parent_id;
                changed = true;
            }
            if existing.name.as_deref() != Some(view.name.as_str()) {
                existing.name = Some(view.name.clone());
                changed = true;
            }
            if existing.sort_name.as_deref() != Some(view.name.as_str()) {
                existing.sort_name = Some(view.name.clone());
                changed = true;
            }
            if !existing.is_folder {
                existing.is_folder = true;
                changed = true;
            }
            if !existing.is_virtual_item {
                existing.is_virtual_item = true;
                changed = true;
            }
            if existing.data.as_ref() != Some(&data) {
                existing.data = Some(data);
                changed = true;
            }
            if changed {
                self.items.update(existing).await?;
            }
            return Ok(());
        }

        let mut item = NewBaseItem::new(view.id, &view.item_type);
        item.parent_id = view.parent_id;
        item.name = Some(view.name.clone());
        item.sort_name = Some(view.name.clone());
        item.is_folder = true;
        item.is_virtual_item = true;
        item.presentation_unique_key = Some(format!("userview:{}", view.id.simple()));
        item.data = Some(data);
        self.items.create(item).await?;
        Ok(())
    }

    /// Lists folders the user can group, matching `/UserViews/GroupingOptions`.
    ///
    /// # Errors
    ///
    /// Returns user, folder, or stored-data errors.
    pub async fn grouping_options(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<UserViewGroupingOption>, UserViewManagerError> {
        let user = self.users.get(user_id).await?;
        let policy = parse_policy(user.policy)?;
        let mut options = self
            .folders
            .list()
            .await?
            .into_iter()
            .filter(|folder| policy_allows_folder(&policy, folder))
            .filter(|folder| is_eligible_for_grouping(folder.collection_type.as_deref()))
            .map(|folder| UserViewGroupingOption {
                id: folder.id,
                name: folder.name,
            })
            .collect::<Vec<_>>();
        options.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(options)
    }

    async fn visible_folders(
        &self,
        policy: &UserPolicy,
        include_hidden: bool,
    ) -> Result<Vec<VirtualFolder>, UserViewManagerError> {
        let mut folders = self.folders.list().await?;
        folders.retain(|folder| {
            policy_allows_folder(policy, folder)
                && (include_hidden || !is_hidden(folder))
                && folder
                    .library_options
                    .as_object()
                    .and_then(|object| object.get("Enabled"))
                    .and_then(Value::as_bool)
                    .is_none_or(|enabled| enabled)
        });
        Ok(folders)
    }
}

fn parse_config(value: Value) -> Result<UserConfiguration, UserViewManagerError> {
    serde_json::from_value(value).map_err(UserViewManagerError::InvalidUserData)
}

fn parse_policy(value: Value) -> Result<UserPolicy, UserViewManagerError> {
    serde_json::from_value(value).map_err(UserViewManagerError::InvalidUserData)
}

fn collection_folder_view_owned(folder: VirtualFolder) -> UserViewItem {
    UserViewItem {
        id: folder.id,
        name: folder.name,
        collection_type: folder.collection_type,
        display_parent_id: None,
        content_parent_ids: vec![folder.id],
        parent_id: Some(USER_ROOT_FOLDER_ID),
        item_type: "CollectionFolder".to_owned(),
        is_virtual_item: false,
    }
}

fn shadow_user_view(folder: VirtualFolder) -> UserViewItem {
    let collection_type = folder.collection_type;
    let name = folder.name;
    let id = user_view_id(&format!(
        "38_namedview_{}{}{}",
        name,
        folder.id,
        collection_type.as_deref().unwrap_or_default()
    ));
    UserViewItem {
        id,
        name,
        collection_type,
        display_parent_id: Some(folder.id),
        content_parent_ids: vec![folder.id],
        parent_id: Some(USER_ROOT_FOLDER_ID),
        item_type: "UserView".to_owned(),
        is_virtual_item: true,
    }
}

fn named_user_view(user_id: Uuid, folder: VirtualFolder) -> UserViewItem {
    let collection_type = folder.collection_type;
    let name = folder.name;
    let id = user_view_id(&format!(
        "38_namedview_{}{}{}{}",
        name,
        user_id,
        folder.id,
        collection_type.as_deref().unwrap_or_default()
    ));
    UserViewItem {
        id,
        name,
        collection_type,
        display_parent_id: Some(folder.id),
        content_parent_ids: vec![folder.id],
        parent_id: Some(USER_ROOT_FOLDER_ID),
        item_type: "UserView".to_owned(),
        is_virtual_item: true,
    }
}

fn grouped_user_view(
    user_id: Uuid,
    name: &str,
    collection_type: &str,
    content_parent_ids: Vec<Uuid>,
) -> UserViewItem {
    let id = user_view_id(&format!("38_namedview_{name}{user_id}{collection_type}"));
    UserViewItem {
        id,
        name: name.to_owned(),
        collection_type: Some(collection_type.to_owned()),
        display_parent_id: None,
        content_parent_ids,
        parent_id: Some(USER_ROOT_FOLDER_ID),
        item_type: "UserView".to_owned(),
        is_virtual_item: true,
    }
}

fn add_grouped_view(
    list: &mut Vec<UserViewItem>,
    user_id: Uuid,
    mut folders: Vec<VirtualFolder>,
    view_type: &str,
    preset_views: &[String],
) {
    if folders.is_empty() {
        return;
    }
    if folders.len() == 1
        && folders[0]
            .collection_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case(view_type))
        && !preset_views
            .iter()
            .any(|preset| preset.eq_ignore_ascii_case(view_type))
    {
        list.push(collection_folder_view_owned(
            folders.pop().expect("single folder checked above"),
        ));
        return;
    }
    let collection_type = view_type.to_ascii_lowercase();
    let content_parent_ids = folders.iter().map(|folder| folder.id).collect();
    list.push(grouped_user_view(
        user_id,
        view_type,
        &collection_type,
        content_parent_ids,
    ));
}

fn user_view_id(key: &str) -> Uuid {
    let digest = Md5::digest(format!(
        "MediaBrowser.Controller.Entities.UserView{}",
        key.to_ascii_lowercase()
    ));
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest);
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn sort_views(views: &mut [UserViewItem], config: &UserConfiguration) {
    views.sort_by(|a, b| {
        ordered_index(a, &config.ordered_views)
            .cmp(&ordered_index(b, &config.ordered_views))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn ordered_index(view: &UserViewItem, orders: &[Uuid]) -> usize {
    orders
        .iter()
        .position(|id| *id == view.id)
        .or_else(|| {
            view.display_parent_id
                .and_then(|parent_id| orders.iter().position(|id| *id == parent_id))
        })
        .unwrap_or(usize::MAX)
}

fn preset_matches(collection_type: Option<&str>, preset_views: &[String]) -> bool {
    collection_type.is_some_and(|collection_type| {
        preset_views
            .iter()
            .any(|preset| preset.eq_ignore_ascii_case(collection_type))
    })
}

fn is_user_specific(collection_type: Option<&str>) -> bool {
    collection_type.is_some_and(|kind| kind.eq_ignore_ascii_case("playlists"))
}

fn is_eligible_for_grouping(collection_type: Option<&str>) -> bool {
    collection_type.is_none_or(|kind| {
        kind.eq_ignore_ascii_case("movies") || kind.eq_ignore_ascii_case("tvshows")
    })
}

fn policy_allows_folder(policy: &UserPolicy, folder: &VirtualFolder) -> bool {
    if policy
        .blocked_media_folders
        .as_ref()
        .is_some_and(|blocked| blocked.contains(&folder.id))
    {
        return false;
    }
    policy.enable_all_folders || policy.enabled_folders.contains(&folder.id)
}

fn is_hidden(folder: &VirtualFolder) -> bool {
    folder
        .library_options
        .as_object()
        .and_then(|object| {
            ["IsHidden", "isHidden", "Hidden", "hidden"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_bool))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_view_ids_are_stable_and_official_prefixed() {
        let first = user_view_id("38_namedview_Movies-0000-0000-0000-000000000000movies");
        assert_eq!(
            first,
            user_view_id("38_namedview_Movies-0000-0000-0000-000000000000movies")
        );
        assert_ne!(
            first,
            user_view_id("38_namedview_Shows-0000-0000-0000-000000000000tvshows")
        );
    }
}
