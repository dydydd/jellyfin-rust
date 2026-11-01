use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemCounts, BaseItemError, BaseItemOrder, BaseItemPage, BaseItemQuery, BaseItemRepository,
    ItemValueQuery, LatestTvGroup, ScoredBaseItem, ScoredBaseItemPage,
    ServerConfigurationRepository,
    entities::{base_item, user},
};
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use serde_json::Value;
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    HydratedBaseItem, ItemTypeRegistry, LocalizationService, LyricManager, LyricManagerError,
    LyricProvider, LyricSearchRequest, MediaStreamFilter, MediaStreamService,
    MediaStreamServiceError, UserError, UserService, decode_lyric_bytes,
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
    #[error("invalid lyric file name or format")]
    InvalidLyricFile,
    #[error("lyric path is outside the internal metadata directory")]
    InvalidLyricPath,
    #[error(transparent)]
    LyricManager(#[from] LyricManagerError),
    #[error("stored user policy is invalid")]
    InvalidPolicy(#[source] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    MediaStream(#[from] MediaStreamServiceError),
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
    server_configuration: ServerConfigurationRepository,
    lyrics: LyricManager,
    media_streams: MediaStreamService,
    internal_metadata_directory: PathBuf,
}

impl UserLibraryService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::with_item_type_registry(database, ItemTypeRegistry::default())
    }

    #[must_use]
    pub fn with_item_type_registry(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_types: ItemTypeRegistry,
    ) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            item_types,
            localization: LocalizationService,
            server_configuration: ServerConfigurationRepository::new(std::sync::Arc::clone(
                &database,
            )),
            lyrics: LyricManager::default(),
            media_streams: MediaStreamService::new(database),
            internal_metadata_directory: PathBuf::from("metadata"),
        }
    }

    /// Replaces the internal metadata directory used for managed lyric files.
    pub fn set_internal_metadata_directory(&mut self, directory: impl Into<PathBuf>) {
        self.internal_metadata_directory = directory.into();
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

    /// Returns configured lyric provider names in provider execution order.
    #[must_use]
    pub fn lyric_provider_names(&self) -> impl Iterator<Item = &str> {
        self.lyrics.provider_names()
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
        if item_id.is_nil() {
            self.validate_user(authenticated_user, target_user_id)
                .await?;
            return self.ensure_user_root().await;
        }
        let mut query = BaseItemQuery {
            ids: vec![item_id],
            include_alternate_versions: true,
            ..BaseItemQuery::default()
        };
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
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
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        query.user_id = Some(target_user_id);
        if query.parent_id.is_none() && query.parent_ids.is_empty() && query.ids.is_empty() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.hydrate_page(self.items.query(&query).await?))
    }

    /// Queries the persisted library without a user context.
    ///
    /// Jellyfin's optional-user endpoints use this path when their user id is omitted, empty, or
    /// resolves to no persisted user. Unlike [`Self::query_items`], this intentionally applies no
    /// user policy, user-data joins, or implicit user-root scope.
    ///
    /// # Errors
    ///
    /// Returns persistence errors unchanged.
    pub async fn query_items_without_user(
        &self,
        mut query: BaseItemQuery,
    ) -> Result<BaseItemPage, UserLibraryError> {
        query.user_id = None;
        query.allowed_official_ratings.clear();
        query.allowed_parental_ratings.clear();
        query.block_unrated_items.clear();
        query.blocked_tags.clear();
        query.allowed_tags.clear();
        query.enabled_folders.clear();
        query.enable_all_folders = true;
        query.blocked_media_folders = None;
        Ok(self.hydrate_page(self.items.query(&query).await?))
    }

    /// Computes latest-TV grouping for the top series under the target user's policy.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, invalid-policy, or persistence errors.
    pub async fn latest_tv_groups(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: BaseItemQuery,
        limit: u64,
    ) -> Result<Vec<LatestTvGroup>, UserLibraryError> {
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        query.user_id = Some(target_user_id);
        if query.parent_id.is_none() && query.parent_ids.is_empty() && query.ids.is_empty() {
            query.parent_id = Some(self.ensure_user_root().await?.id);
        }
        Ok(self.items.latest_tv_groups(&query, limit).await?)
    }

    /// Applies the target user's normal library visibility policy to a query.
    ///
    /// This is used by non-`Items` endpoints whose candidates still flow
    /// through Jellyfin's `InternalItemsQuery(user)` policy checks.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, stored-policy, or persistence errors.
    pub(crate) async fn apply_base_item_policy(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: &mut BaseItemQuery,
    ) -> Result<(), UserLibraryError> {
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, query)
            .await?;
        query.user_id = Some(target_user_id);
        Ok(())
    }

    /// Counts visible direct children for several library parents in one
    /// PostgreSQL query.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, stored-policy, or persistence errors.
    pub async fn child_counts_by_parent(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: BaseItemQuery,
    ) -> Result<HashMap<Uuid, u64>, UserLibraryError> {
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        query.user_id = Some(target_user_id);
        Ok(self.items.child_counts_by_parent(&query).await?)
    }

    /// Counts real leaf descendants for several folders using the target user's normal library
    /// access policy.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-policy, or persistence errors.
    pub async fn recursive_item_counts(
        &self,
        target_user_id: Uuid,
        parent_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, u64>, UserLibraryError> {
        let mut query = BaseItemQuery {
            is_virtual_item: Some(false),
            user_id: Some(target_user_id),
            ..BaseItemQuery::default()
        };
        self.apply_user_policy(&mut query, target_user_id).await?;
        Ok(self
            .items
            .dto_recursive_item_counts(parent_ids, &query)
            .await?)
    }

    /// Counts played real leaf descendants for several folders using the
    /// target user's normal library policy.
    pub async fn recursive_played_item_counts(
        &self,
        target_user_id: Uuid,
        parent_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, u64>, UserLibraryError> {
        let mut query = BaseItemQuery {
            is_virtual_item: Some(false),
            is_played: Some(true),
            user_id: Some(target_user_id),
            ..BaseItemQuery::default()
        };
        self.apply_user_policy(&mut query, target_user_id).await?;
        Ok(self
            .items
            .dto_recursive_item_counts(parent_ids, &query)
            .await?)
    }

    /// Counts non-virtual items visible to one target user using the normal library policy.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-policy, or persistence errors.
    pub async fn item_counts(
        &self,
        target_user_id: Uuid,
        is_favorite: Option<bool>,
    ) -> Result<BaseItemCounts, UserLibraryError> {
        let mut query = BaseItemQuery {
            recursive: true,
            is_virtual_item: Some(false),
            is_favorite,
            user_id: Some(target_user_id),
            ..BaseItemQuery::default()
        };
        self.apply_user_policy(&mut query, target_user_id).await?;
        Ok(self.items.item_counts(&query).await?)
    }

    /// Applies the same target-user access policy used by ordinary item pages to an item-by-name
    /// query. This keeps value discovery and its counts from exposing disabled libraries, blocked
    /// tags, or parental-rating restricted items.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, or stored-policy errors.
    pub async fn apply_item_value_policy(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: &mut ItemValueQuery,
    ) -> Result<(), UserLibraryError> {
        self.authorize_and_apply_user_policy(
            authenticated_user,
            target_user_id,
            &mut query.access_policy,
        )
        .await?;
        query.user_id = Some(target_user_id);
        Ok(())
    }

    /// Loads one authorized target user's library policy as a reusable query template.
    ///
    /// Filter endpoints clone this template into their base-item, item-value,
    /// and media-stream queries so every bucket uses one policy snapshot.
    ///
    /// # Errors
    ///
    /// Returns forbidden, not-found, stored-policy, or persistence errors.
    pub async fn filter_access_policy(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<BaseItemQuery, UserLibraryError> {
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(UserLibraryError::Forbidden);
        }
        let user = match self.users.get(target_user_id).await {
            Ok(user) => user,
            Err(UserError::NotFound) => return Err(UserLibraryError::UserNotFound),
            Err(error) => return Err(error.into()),
        };
        let mut query = BaseItemQuery {
            user_id: Some(target_user_id),
            ..BaseItemQuery::default()
        };
        self.apply_stored_user_policy(&mut query, user.policy)
            .await?;
        Ok(query)
    }

    /// Filters already-resolved item identifiers through a target user's library policy.
    ///
    /// Callers must first authorize access to the target user. This lightweight lookup is used
    /// after expanding alternate media-source groups so hidden versions can be discarded without
    /// loading their base-item rows again.
    ///
    /// # Errors
    ///
    /// Returns a missing-user, invalid-policy, or persistence error.
    pub async fn visible_item_ids(
        &self,
        target_user_id: Uuid,
        item_ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, UserLibraryError> {
        let mut access_policy = BaseItemQuery::default();
        self.apply_user_policy(&mut access_policy, target_user_id)
            .await?;
        Ok(self
            .items
            .visible_item_ids(item_ids, &access_policy)
            .await?)
    }

    /// Counts alternate media versions visible to an already-authorized target user.
    ///
    /// The repository performs one set-based aggregate for every requested displayed item and
    /// always includes that displayed item even when evaluating its siblings' policy visibility.
    ///
    /// # Errors
    ///
    /// Returns a missing-user, invalid-policy, or persistence error.
    pub async fn visible_media_source_counts(
        &self,
        target_user_id: Uuid,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, u64>, UserLibraryError> {
        let mut access_policy = BaseItemQuery::default();
        self.apply_user_policy(&mut access_policy, target_user_id)
            .await?;
        Ok(self
            .items
            .visible_media_source_counts(item_ids, &access_policy)
            .await?)
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
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
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
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        query.recursive = true;
        query.is_virtual_item = Some(false);
        if query.parent_id.is_none() && query.parent_ids.is_empty() {
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
        display_specials_within_seasons: bool,
        next_up_date_cutoff: Option<DateTime<Utc>>,
        start_index: u64,
        limit: Option<u64>,
        enable_total_record_count: bool,
    ) -> Result<BaseItemPage, UserLibraryError> {
        let mut query = BaseItemQuery {
            parent_id,
            recursive: true,
            include_item_types: vec!["Episode".to_owned()],
            is_virtual_item: Some(false),
            user_id: Some(target_user_id),
            enable_total_record_count: Some(enable_total_record_count),
            ..BaseItemQuery::default()
        };
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;
        let page = self
            .items
            .next_up(
                target_user_id,
                &query,
                enable_rewatching,
                enable_resumable,
                display_specials_within_seasons,
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
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        let descendants = self.items.descendants(item.id).await?;
        let mut candidates = descendants
            .into_iter()
            .filter_map(|entry| self.item_types.hydrate(entry.item))
            .map(HydratedBaseItem::into_model)
            .filter(|candidate| related_item_matches(candidate, kind))
            .collect::<Vec<_>>();
        let candidate_ids = candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        let visible_ids = self
            .visible_item_ids(target_user_id, &candidate_ids)
            .await?;
        candidates.retain(|candidate| visible_ids.contains(&candidate.id));
        Ok(candidates)
    }

    /// Loads theme extras for several possible owners in one policy-aware query.
    ///
    /// Results are grouped by the official persisted `OwnerId`, falling back
    /// to the direct parent used by the Rust scanner. This allows API callers
    /// to select the nearest ancestor independently for songs and videos
    /// without issuing one query per hierarchy level.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, stored-policy, or persistence errors.
    pub async fn theme_items_for_owners(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        owner_ids: &[Uuid],
        kind: RelatedItemKind,
        order: BaseItemOrder,
    ) -> Result<HashMap<Uuid, Vec<base_item::Model>>, UserLibraryError> {
        if owner_ids.is_empty() {
            self.validate_user(authenticated_user, target_user_id)
                .await?;
            return Ok(HashMap::new());
        }

        let mut query = BaseItemQuery {
            parent_ids: owner_ids.to_vec(),
            recursive: true,
            user_id: Some(target_user_id),
            order,
            enable_total_record_count: Some(false),
            ..BaseItemQuery::default()
        };
        self.authorize_and_apply_user_policy(authenticated_user, target_user_id, &mut query)
            .await?;

        let owners = owner_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut grouped = HashMap::<Uuid, Vec<base_item::Model>>::new();
        for item in self.hydrate_page(self.items.query(&query).await?).items {
            if !related_item_matches(&item, kind) {
                continue;
            }
            let owner_id = metadata_value(item.data.as_ref(), &["OwnerId", "ownerId", "owner_id"])
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .filter(|owner_id| owners.contains(owner_id))
                .or_else(|| item.parent_id.filter(|owner_id| owners.contains(owner_id)));
            let Some(owner_id) = owner_id else {
                continue;
            };
            grouped.entry(owner_id).or_default().push(item);
        }
        Ok(grouped)
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
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        let Some(item_type) = self.item_types.resolve(&item.item_type) else {
            return Ok(Vec::new());
        };
        if !is_video_item_type(item_type.name()) {
            return Ok(Vec::new());
        }

        let paths = additional_part_paths(item.data.as_ref());
        let mut candidates = self
            .items
            .by_paths(&paths)
            .await?
            .into_iter()
            .filter_map(|item| self.item_types.hydrate(item))
            .filter(|item| is_video_item_type(item.item_type().name()))
            .map(HydratedBaseItem::into_model)
            .collect::<Vec<_>>();
        let candidate_ids = candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        let visible_ids = self
            .visible_item_ids(target_user_id, &candidate_ids)
            .await?;
        candidates.retain(|candidate| visible_ids.contains(&candidate.id));
        Ok(candidates)
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
        let item = self
            .audio_item(authenticated_user, target_user_id, item_id)
            .await?;
        let mut streams = self
            .media_streams
            .get_media_streams(MediaStreamFilter {
                item_id,
                index: None,
                stream_type: Some(MediaStreamType::Lyric),
            })
            .await?;
        streams.sort_by_key(|stream| stream.index);
        let has_lyric_streams = !streams.is_empty();
        for stream in streams {
            let path = stream.path.ok_or(UserLibraryError::LyricsNotFound)?;
            let bytes = fs::read(&path).await?;
            let Some(format) = Path::new(&path)
                .extension()
                .and_then(|value| value.to_str())
            else {
                continue;
            };
            let content = decode_lyric_bytes(&bytes);
            if let Some(lyrics) = LyricManager::parse_lyrics(format, &content) {
                return Ok(lyrics);
            }
        }
        if has_lyric_streams {
            return Err(UserLibraryError::LyricsNotFound);
        }

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
        let item = self
            .audio_item(authenticated_user, target_user_id, item_id)
            .await?;
        let request = lyric_search_request(&item);
        Ok(self
            .lyrics
            .search(&request)
            .await
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
        let item = self
            .audio_item(authenticated_user, target_user_id, item_id)
            .await?;
        let Some(response) = self.lyrics.get_lyrics(lyric_id).await? else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        let content = decode_lyric_bytes(&response.content);
        let Some(lyrics) = LyricManager::parse_lyrics(&response.format, &content) else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        self.save_lyric_file(item, &response.format, &response.content, lyrics)
            .await
    }

    /// Returns parsed remote lyrics without attaching them to the item.
    ///
    /// # Errors
    ///
    /// Returns [`UserLibraryError::LyricsNotFound`] when no provider can
    /// resolve `lyric_id` or the payload cannot be parsed.
    pub async fn get_remote_lyrics(&self, lyric_id: &str) -> Result<Value, UserLibraryError> {
        let Some(response) = self.lyrics.get_lyrics(lyric_id).await? else {
            return Err(UserLibraryError::LyricsNotFound);
        };
        let content = decode_lyric_bytes(&response.content);
        LyricManager::parse_lyrics(&response.format, &content)
            .ok_or(UserLibraryError::LyricsNotFound)
    }

    /// Validates, parses, and saves uploaded lyrics on an audio item.
    ///
    /// # Errors
    ///
    /// Returns not-found before validating upload contents when the item is missing or is not an
    /// audio item. Returns invalid-file when the upload is empty, malformed, or unsupported.
    pub async fn save_lyrics(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        file_name: &str,
        content: &[u8],
    ) -> Result<Value, UserLibraryError> {
        let item = self
            .audio_item(authenticated_user, target_user_id, item_id)
            .await?;
        if content.is_empty() {
            return Err(UserLibraryError::InvalidLyricFile);
        }
        let format = uploaded_lyric_format(file_name).ok_or(UserLibraryError::InvalidLyricFile)?;
        let decoded = decode_lyric_bytes(content);
        let lyrics = LyricManager::parse_lyrics(format, &decoded)
            .ok_or(UserLibraryError::InvalidLyricFile)?;
        self.save_lyric_file(item, format, content, lyrics).await
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
        let mut item = self
            .audio_item(authenticated_user, target_user_id, item_id)
            .await?;
        let original_streams = self
            .media_streams
            .get_media_streams(MediaStreamFilter::for_item(item_id))
            .await?;
        let retained_streams = original_streams
            .iter()
            .filter(|stream| stream.stream_type != MediaStreamType::Lyric)
            .cloned()
            .collect::<Vec<_>>();
        let lyric_paths = original_streams
            .iter()
            .filter(|stream| stream.stream_type == MediaStreamType::Lyric)
            .filter_map(|stream| stream.path.as_deref())
            .collect::<HashSet<_>>();

        let mut staged = Vec::new();
        for path in lyric_paths {
            match self.stage_internal_lyric_delete(Path::new(path)).await {
                Ok(Some(pair)) => staged.push(pair),
                Ok(None) => {}
                Err(error) => {
                    restore_staged_files(&staged).await;
                    return Err(error);
                }
            }
        }

        if let Err(error) = self
            .media_streams
            .save_media_streams(item_id, retained_streams)
            .await
        {
            restore_staged_files(&staged).await;
            return Err(error.into());
        }

        if let Some(data) = item.data.as_mut().and_then(Value::as_object_mut) {
            data.remove("Lyrics");
            data.remove("lyrics");
        }
        if let Err(error) = self.items.update(item).await {
            let _ = self
                .media_streams
                .save_media_streams(item_id, original_streams)
                .await;
            restore_staged_files(&staged).await;
            return Err(error.into());
        }
        cleanup_staged_files(&staged).await;
        Ok(())
    }

    async fn save_lyric_file(
        &self,
        mut item: base_item::Model,
        format: &str,
        content: &[u8],
        lyrics: Value,
    ) -> Result<Value, UserLibraryError> {
        let format = normalized_lyric_format(format).ok_or(UserLibraryError::InvalidLyricFile)?;
        let stem = item
            .path
            .as_deref()
            .and_then(|path| Path::new(path).file_stem())
            .and_then(|stem| stem.to_str())
            .filter(|stem| is_safe_file_stem(stem))
            .ok_or(UserLibraryError::InvalidLyricFile)?;
        let directory = self.safe_item_metadata_directory(item.id).await?;
        let target = directory.join(format!("{stem}.{format}"));
        let backup = replace_file_atomically(&target, content).await?;

        let original_item = item.clone();
        let original_streams = match self
            .media_streams
            .get_media_streams(MediaStreamFilter::for_item(item.id))
            .await
        {
            Ok(streams) => streams,
            Err(error) => {
                restore_replaced_file(&target, backup.as_deref()).await;
                return Err(error.into());
            }
        };
        let target_string = target.to_string_lossy().into_owned();
        let mut updated_streams = original_streams.clone();
        if let Some(stream) = updated_streams.iter_mut().find(|stream| {
            stream.stream_type == MediaStreamType::Lyric
                && stream.path.as_deref() == Some(target_string.as_str())
        }) {
            stream.codec = Some(format.to_owned());
            stream.is_external = true;
        } else {
            let index = updated_streams
                .iter()
                .map(|stream| stream.index)
                .max()
                .unwrap_or(-1)
                .saturating_add(1);
            updated_streams.push(MediaStream {
                codec: Some(format.to_owned()),
                index,
                stream_type: MediaStreamType::Lyric,
                is_external: true,
                path: Some(target_string),
                ..MediaStream::default()
            });
        }
        if let Err(error) = self
            .media_streams
            .save_media_streams(item.id, updated_streams)
            .await
        {
            restore_replaced_file(&target, backup.as_deref()).await;
            return Err(error.into());
        }

        if !matches!(item.data, Some(Value::Object(_))) {
            item.data = Some(Value::Object(serde_json::Map::default()));
        }
        if let Some(Value::Object(object)) = item.data.as_mut() {
            object.insert("Lyrics".to_owned(), lyrics.clone());
            object.remove("lyrics");
        }
        if let Err(error) = self.items.update(item).await {
            let _ = self
                .media_streams
                .save_media_streams(original_item.id, original_streams)
                .await;
            restore_replaced_file(&target, backup.as_deref()).await;
            return Err(error.into());
        }
        if let Some(backup) = backup {
            let _ = fs::remove_file(backup).await;
        }
        Ok(lyrics)
    }

    async fn safe_item_metadata_directory(
        &self,
        item_id: Uuid,
    ) -> Result<PathBuf, UserLibraryError> {
        fs::create_dir_all(&self.internal_metadata_directory).await?;
        let root = fs::canonicalize(&self.internal_metadata_directory).await?;
        let id = item_id.simple().to_string();
        let directory = root.join("library").join(&id[..2]).join(id);
        fs::create_dir_all(&directory).await?;
        let directory = fs::canonicalize(directory).await?;
        if !directory.starts_with(&root) {
            return Err(UserLibraryError::InvalidLyricPath);
        }
        Ok(directory)
    }

    async fn stage_internal_lyric_delete(
        &self,
        path: &Path,
    ) -> Result<Option<(PathBuf, PathBuf)>, UserLibraryError> {
        let metadata = match fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(None);
        }
        let root = match fs::canonicalize(&self.internal_metadata_directory).await {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let canonical = fs::canonicalize(path).await?;
        if !canonical.starts_with(root) {
            return Ok(None);
        }
        let file_name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(UserLibraryError::InvalidLyricPath)?;
        let staged =
            canonical.with_file_name(format!(".{file_name}-{}.delete", Uuid::new_v4().simple()));
        fs::rename(&canonical, &staged).await?;
        Ok(Some((canonical, staged)))
    }

    /// Matches the official `GetItemById<Audio>(id, user)` lookup used by every item-scoped
    /// lyrics endpoint: hidden items and non-audio rows both surface as not found.
    async fn audio_item(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, UserLibraryError> {
        let item = self
            .item(authenticated_user, target_user_id, item_id)
            .await?;
        if !item.item_type.eq_ignore_ascii_case("Audio") {
            return Err(UserLibraryError::ItemNotFound);
        }
        Ok(item)
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), UserLibraryError> {
        self.authorized_target_user(authenticated_user, target_user_id)
            .await
            .map(|_| ())
    }

    /// Loads and authorizes a target user before a policy-aware query.
    ///
    /// The lookup deliberately precedes the access check to preserve Jellyfin's existing missing
    /// target-user error precedence. Callers that also need policy filtering reuse this row instead
    /// of issuing a second user lookup through [`Self::apply_user_policy`].
    async fn authorized_target_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<user::Model, UserLibraryError> {
        let target_user = match self.users.get(target_user_id).await {
            Ok(user) => user,
            Err(UserError::NotFound) => return Err(UserLibraryError::UserNotFound),
            Err(error) => return Err(error.into()),
        };
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(UserLibraryError::Forbidden);
        }
        Ok(target_user)
    }

    async fn authorize_and_apply_user_policy(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        query: &mut BaseItemQuery,
    ) -> Result<(), UserLibraryError> {
        let target_user = self
            .authorized_target_user(authenticated_user, target_user_id)
            .await?;
        self.apply_stored_user_policy(query, target_user.policy)
            .await
    }

    /// Applies the target user's library-access policy to a base-item query.
    ///
    /// # Errors
    ///
    /// Returns a user lookup or policy-deserialization error.
    pub async fn apply_user_policy(
        &self,
        query: &mut BaseItemQuery,
        target_user_id: Uuid,
    ) -> Result<(), UserLibraryError> {
        let user = self.users.get(target_user_id).await?;
        self.apply_stored_user_policy(query, user.policy).await
    }

    async fn apply_stored_user_policy(
        &self,
        query: &mut BaseItemQuery,
        stored_policy: Value,
    ) -> Result<(), UserLibraryError> {
        let policy: UserPolicy =
            serde_json::from_value(stored_policy).map_err(UserLibraryError::InvalidPolicy)?;
        query.blocked_tags = normalized_tags(&policy.blocked_tags);
        query.allowed_tags = normalized_tags(&policy.allowed_tags);
        query.block_unrated_items = policy
            .block_unrated_items
            .iter()
            .map(|item| item.as_str().to_owned())
            .collect();
        if let Some(maximum) = policy.max_parental_rating {
            let metadata_country_code = self
                .server_configuration
                .load()
                .await
                .map(|configuration| configuration.metadata_country_code)
                .unwrap_or_default();
            query.allowed_parental_ratings = self.localization.parental_rating_names_at_or_below(
                maximum,
                policy.max_parental_sub_rating,
                &metadata_country_code,
            );
        }
        query.enabled_folders = policy.enabled_folders;
        query.enable_all_folders = policy.enable_all_folders;
        query.blocked_media_folders = policy.blocked_media_folders;
        Ok(())
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

fn normalized_lyric_format(format: &str) -> Option<&'static str> {
    ["lrc", "elrc", "txt"]
        .into_iter()
        .find(|supported| format.eq_ignore_ascii_case(supported))
}

fn uploaded_lyric_format(file_name: &str) -> Option<&str> {
    if file_name.is_empty()
        || file_name.trim() != file_name
        || file_name.contains(['/', '\\', '\0'])
        || file_name.chars().any(char::is_control)
    {
        return None;
    }
    let (stem, extension) = file_name.rsplit_once('.')?;
    (!stem.is_empty()
        && stem != "."
        && stem != ".."
        && !extension.is_empty()
        && extension != "."
        && extension != "..")
        .then_some(extension)
}

fn is_safe_file_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem != "."
        && stem != ".."
        && !stem.chars().any(|character| {
            character == '/' || character == '\\' || character == '\0' || character.is_control()
        })
}

async fn replace_file_atomically(
    target: &Path,
    content: &[u8],
) -> Result<Option<PathBuf>, UserLibraryError> {
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(UserLibraryError::InvalidLyricFile)?;
    let token = Uuid::new_v4().simple();
    let temporary = target.with_file_name(format!(".{file_name}-{token}.tmp"));
    let backup = target.with_file_name(format!(".{file_name}-{token}.backup"));
    let write_result = async {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        file.write_all(content).await?;
        file.sync_all().await?;
        drop(file);
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary).await;
        return Err(error.into());
    }

    let had_target = match fs::symlink_metadata(target).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            let _ = fs::remove_file(&temporary).await;
            return Err(UserLibraryError::InvalidLyricPath);
        }
        Ok(_) => {
            if let Err(error) = fs::copy(target, &backup).await {
                let _ = fs::remove_file(&temporary).await;
                let _ = fs::remove_file(&backup).await;
                return Err(error.into());
            }
            if let Err(error) = fs::File::open(&backup).await?.sync_all().await {
                let _ = fs::remove_file(&temporary).await;
                let _ = fs::remove_file(&backup).await;
                return Err(error.into());
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(error.into());
        }
    };
    if let Err(error) = fs::rename(&temporary, target).await {
        let _ = fs::remove_file(&temporary).await;
        let _ = fs::remove_file(&backup).await;
        return Err(error.into());
    }
    Ok(had_target.then_some(backup))
}

async fn restore_replaced_file(target: &Path, backup: Option<&Path>) {
    let _ = fs::remove_file(target).await;
    if let Some(backup) = backup {
        let _ = fs::rename(backup, target).await;
    }
}

async fn restore_staged_files(staged: &[(PathBuf, PathBuf)]) {
    for (original, staged) in staged.iter().rev() {
        let _ = fs::rename(staged, original).await;
    }
}

async fn cleanup_staged_files(staged: &[(PathBuf, PathBuf)]) {
    for (_, staged) in staged {
        if let Err(error) = fs::remove_file(staged).await {
            tracing::warn!(path = %staged.display(), %error, "could not remove staged lyric file");
        }
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

fn normalized_tags(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(jellyfin_extensions::StringExtensions::clean_value)
        .filter(|value| !value.is_empty())
        .collect()
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

fn lyric_search_request(item: &base_item::Model) -> LyricSearchRequest {
    LyricSearchRequest {
        media_path: item.path.clone(),
        song_name: item.name.clone(),
        album_name: metadata_string(item.data.as_ref(), &["Album"]),
        artist_names: metadata_string_list(item.data.as_ref(), &["Artists"]),
        album_artist_names: metadata_string_list(item.data.as_ref(), &["AlbumArtists"]),
        duration_ticks: item.runtime_ticks,
        ..LyricSearchRequest::default()
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

    #[test]
    fn remote_lyric_search_maps_the_official_audio_request_fields() {
        let mut item = item_with_data(json!({
            "Album": "Album",
            "Artists": ["First Artist", "Second Artist"],
            "AlbumArtists": ["Album Artist"]
        }));
        item.path = Some("/media/Artist/Album/Song.flac".to_owned());
        item.name = Some("Song".to_owned());
        item.runtime_ticks = Some(1_234_567);

        assert_eq!(
            lyric_search_request(&item),
            LyricSearchRequest {
                media_path: Some("/media/Artist/Album/Song.flac".to_owned()),
                song_name: Some("Song".to_owned()),
                album_name: Some("Album".to_owned()),
                artist_names: vec!["First Artist".to_owned(), "Second Artist".to_owned()],
                album_artist_names: vec!["Album Artist".to_owned()],
                duration_ticks: Some(1_234_567),
                ..LyricSearchRequest::default()
            }
        );
    }
}
