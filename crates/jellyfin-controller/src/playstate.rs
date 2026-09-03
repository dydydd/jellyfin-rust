use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemRepository, ServerConfigurationRepository,
    ServerConfigurationStoreError, UserDataError, UserDataPatch, UserDataRepository,
    entities::{base_item, server_configuration, user, user_data},
};
use jellyfin_model::UserConfiguration;
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

use jellyfin_model::UserItemDataDto;

use crate::{
    UserError, UserService,
    user_data::{current_user_data_keys, user_data_to_dto},
};

#[derive(Debug, Error)]
pub enum PlaystateError {
    #[error("target user not found")]
    UserNotFound,
    #[error("item not found")]
    ItemNotFound,
    #[error("the authenticated user cannot update this user's playstate")]
    Forbidden,
    #[error("invalid date played")]
    InvalidDatePlayed,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    UserData(#[from] UserDataError),
    #[error(transparent)]
    ServerConfiguration(#[from] ServerConfigurationStoreError),
}

/// Parses the current ISO-8601 query format and Jellyfin's legacy compact UTC
/// timestamp used by older clients.
///
/// # Errors
///
/// Returns [`PlaystateError::InvalidDatePlayed`] for an invalid timestamp.
pub fn parse_date_played(value: &str) -> Result<DateTime<Utc>, PlaystateError> {
    if let Ok(date) = DateTime::parse_from_rfc3339(value.trim()) {
        return Ok(date.with_timezone(&Utc));
    }
    NaiveDateTime::parse_from_str(value.trim(), "%Y%m%d%H%M%S")
        .map(|date| date.and_utc())
        .map_err(|_| PlaystateError::InvalidDatePlayed)
}

/// Formats a UTC timestamp using Jellyfin's JSON date representation.
#[must_use]
pub fn format_date_played(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaystateUpdate {
    pub user_data: user_data::Model,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackProgressUpdate {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<i64>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackStartUpdate {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackStopUpdate {
    pub item_id: Uuid,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<i64>,
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlaybackStopEffect {
    playback_position_ticks: i64,
    played: Option<bool>,
    increment_play_count: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlaystateConfiguration {
    min_resume_pct: i32,
    max_resume_pct: i32,
    min_resume_duration_seconds: i64,
    min_audiobook_resume_minutes: i32,
    max_audiobook_resume_minutes: i32,
}

impl From<&server_configuration::Model> for PlaystateConfiguration {
    fn from(configuration: &server_configuration::Model) -> Self {
        Self {
            min_resume_pct: configuration.min_resume_pct,
            max_resume_pct: configuration.max_resume_pct,
            min_resume_duration_seconds: i64::from(configuration.min_resume_duration_seconds),
            min_audiobook_resume_minutes: configuration.min_audiobook_resume,
            max_audiobook_resume_minutes: configuration.max_audiobook_resume,
        }
    }
}

impl From<PlaystateUpdate> for UserItemDataDto {
    fn from(update: PlaystateUpdate) -> Self {
        user_data_to_dto(update.user_data, None)
    }
}

/// Coordinates authorization, item validation, and atomic playstate writes.
#[derive(Clone)]
pub struct PlaystateService {
    users: UserService,
    items: BaseItemRepository,
    user_data: UserDataRepository,
    server_configuration: ServerConfigurationRepository,
}

impl PlaystateService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            user_data: UserDataRepository::new(std::sync::Arc::clone(&database)),
            server_configuration: ServerConfigurationRepository::new(database),
        }
    }

    /// Marks an item played for the target user.
    ///
    /// # Errors
    ///
    /// Returns not-found, permission, or persistence errors after checking the
    /// target user and item in official controller order.
    pub async fn mark_played(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
        date_played: Option<DateTime<Utc>>,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        let item = self
            .validate_request(authenticated_user, target_user_id, item_id)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .mark_played(item_id, target_user_id, &keys[0], date_played)
            .await?;
        if should_propagate_played_state(&item, &user_data) {
            self.user_data
                .mark_alternate_versions_played(item.id, target_user_id, true)
                .await?;
        }
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item played after the API layer has authorized the target user.
    ///
    /// This entry point exists for API-key authentication, which has
    /// administrator-equivalent access but no user model of its own.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors after checking the target user
    /// and item in official controller order.
    pub async fn mark_played_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
        date_played: Option<DateTime<Utc>>,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        let item = self
            .validate_authorized_request(target_user_id, item_id)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .mark_played(item_id, target_user_id, &keys[0], date_played)
            .await?;
        if should_propagate_played_state(&item, &user_data) {
            self.user_data
                .mark_alternate_versions_played(item.id, target_user_id, true)
                .await?;
        }
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item unplayed for the target user.
    ///
    /// # Errors
    ///
    /// Returns not-found, permission, or persistence errors after checking the
    /// target user and item in official controller order.
    pub async fn mark_unplayed(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        let item = self
            .validate_request(authenticated_user, target_user_id, item_id)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .mark_unplayed(item_id, target_user_id, &keys[0])
            .await?;
        if is_video_item(&item) {
            self.user_data
                .mark_alternate_versions_unplayed(item.id, target_user_id)
                .await?;
        }
        Ok(PlaystateUpdate { user_data })
    }

    /// Marks an item unplayed after the API layer has authorized the target user.
    ///
    /// This entry point exists for API-key authentication, which has
    /// administrator-equivalent access but no user model of its own.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors after checking the target user
    /// and item in official controller order.
    pub async fn mark_unplayed_for_authorized_user(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<PlaystateUpdate, PlaystateError> {
        let item = self
            .validate_authorized_request(target_user_id, item_id)
            .await?;
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .mark_unplayed(item_id, target_user_id, &keys[0])
            .await?;
        if is_video_item(&item) {
            self.user_data
                .mark_alternate_versions_unplayed(item.id, target_user_id)
                .await?;
        }
        Ok(PlaystateUpdate { user_data })
    }

    /// Applies the user-data side effects of a playback progress report.
    ///
    /// Jellyfin treats session progress as best-effort: an empty or unknown
    /// item id records no user data and still succeeds. When an item is known,
    /// `PostgreSQL` atomically updates the resume position and remembered stream
    /// selections according to the reporting user's configuration.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors for invalid playback values or
    /// database failures.
    pub async fn report_playback_progress(
        &self,
        user: &user::Model,
        update: PlaybackProgressUpdate,
    ) -> Result<Option<PlaystateUpdate>, PlaystateError> {
        if update
            .position_ticks
            .is_some_and(|position_ticks| position_ticks < 0)
        {
            return Err(UserDataError::NegativePlaybackValue.into());
        }
        let Some(item) = self
            .progress_item(update.item_id, update.media_source_id.as_deref())
            .await?
        else {
            return Ok(None);
        };
        let configuration = UserConfiguration::deserialize(&user.preferences).unwrap_or_default();
        let playstate_configuration = self.playstate_configuration().await?;
        let patch =
            playback_progress_patch(&configuration, &update, &item, &playstate_configuration);
        if !has_playback_progress_changes(&patch) {
            return Ok(None);
        }

        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .apply_playback_progress_patch(item.id, user.id, &keys, patch)
            .await?;
        if should_propagate_played_state(&item, &user_data) {
            self.user_data
                .mark_alternate_versions_played(item.id, user.id, true)
                .await?;
        }
        Ok(Some(PlaystateUpdate { user_data }))
    }

    /// Applies the user-data side effects of a playback start report.
    ///
    /// Empty or unknown item identifiers behave like Jellyfin session reports:
    /// the call succeeds without writing user data. Known items increment the
    /// reporting user's play count and update the last-played timestamp.
    ///
    /// # Errors
    ///
    /// Returns persistence errors for database failures.
    pub async fn report_playback_start(
        &self,
        user: &user::Model,
        update: PlaybackStartUpdate,
    ) -> Result<Option<PlaystateUpdate>, PlaystateError> {
        let Some(item) = self
            .progress_item(update.item_id, update.media_source_id.as_deref())
            .await?
        else {
            return Ok(None);
        };
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .record_playback_start(
                item.id,
                user.id,
                &keys,
                Utc::now(),
                should_mark_played_on_start(&item),
            )
            .await?;
        Ok(Some(PlaystateUpdate { user_data }))
    }

    /// Applies the user-data side effects of a playback stop report.
    ///
    /// Negative positions are rejected before item lookup, matching Jellyfin's
    /// stopped-session validation. Failed playback reports and empty or unknown
    /// item identifiers succeed without writing user data. Known successful
    /// stops apply Jellyfin's default resume/completion thresholds.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors for invalid playback values or
    /// database failures.
    pub async fn report_playback_stop(
        &self,
        user: &user::Model,
        update: PlaybackStopUpdate,
    ) -> Result<Option<PlaystateUpdate>, PlaystateError> {
        if update
            .position_ticks
            .is_some_and(|position_ticks| position_ticks < 0)
        {
            return Err(UserDataError::NegativePlaybackValue.into());
        }
        if update.failed {
            return Ok(None);
        }
        let Some(item) = self
            .progress_item(update.item_id, update.media_source_id.as_deref())
            .await?
        else {
            return Ok(None);
        };
        let playstate_configuration = self.playstate_configuration().await?;
        let effect = playback_stop_effect(&item, update.position_ticks, &playstate_configuration);
        let keys = current_user_data_keys(&item);
        let user_data = self
            .user_data
            .record_playback_stop(
                item.id,
                user.id,
                &keys,
                effect.playback_position_ticks,
                effect.played,
                effect.increment_play_count,
            )
            .await?;
        if should_propagate_played_state(&item, &user_data) {
            self.user_data
                .mark_alternate_versions_played(item.id, user.id, true)
                .await?;
        }
        Ok(Some(PlaystateUpdate { user_data }))
    }

    async fn validate_request(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, PlaystateError> {
        self.validate_target_user(target_user_id).await?;
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(PlaystateError::Forbidden);
        }
        self.validate_item(item_id).await
    }

    async fn validate_authorized_request(
        &self,
        target_user_id: Uuid,
        item_id: Uuid,
    ) -> Result<base_item::Model, PlaystateError> {
        self.validate_target_user(target_user_id).await?;
        self.validate_item(item_id).await
    }

    async fn validate_target_user(&self, target_user_id: Uuid) -> Result<(), PlaystateError> {
        match self.users.get(target_user_id).await {
            Ok(_) => Ok(()),
            Err(UserError::NotFound) => Err(PlaystateError::UserNotFound),
            Err(error) => Err(error.into()),
        }
    }

    async fn validate_item(&self, item_id: Uuid) -> Result<base_item::Model, PlaystateError> {
        self.items
            .get(item_id)
            .await?
            .ok_or(PlaystateError::ItemNotFound)
    }

    async fn progress_item(
        &self,
        item_id: Uuid,
        media_source_id: Option<&str>,
    ) -> Result<Option<base_item::Model>, PlaystateError> {
        if item_id.is_nil() {
            return Ok(None);
        }
        let Some(item) = self.items.get(item_id).await? else {
            return Ok(None);
        };
        if is_video_item(&item)
            && let Some(version_item_id) = parse_media_source_item_id(media_source_id)
            && version_item_id != item.id
            && let Some(version) = self
                .items
                .alternate_video_version(item.id, version_item_id)
                .await?
        {
            return Ok(Some(version));
        }
        Ok(Some(item))
    }

    async fn playstate_configuration(&self) -> Result<PlaystateConfiguration, PlaystateError> {
        let configuration = self.server_configuration.load().await?;
        Ok(PlaystateConfiguration::from(&configuration))
    }
}

fn parse_media_source_item_id(media_source_id: Option<&str>) -> Option<Uuid> {
    media_source_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn playback_progress_patch(
    configuration: &UserConfiguration,
    update: &PlaybackProgressUpdate,
    item: &base_item::Model,
    playstate_configuration: &PlaystateConfiguration,
) -> UserDataPatch {
    let (playback_position_ticks, played) = if let Some(position_ticks) = update.position_ticks {
        let (playback_position_ticks, played) =
            update_play_state_effect(item, position_ticks, playstate_configuration);
        (Some(playback_position_ticks), played)
    } else {
        (None, None)
    };
    UserDataPatch {
        playback_position_ticks,
        played,
        audio_stream_index: if configuration.remember_audio_selections {
            update.audio_stream_index.map(Some)
        } else {
            Some(None)
        },
        subtitle_stream_index: if configuration.remember_subtitle_selections {
            update.subtitle_stream_index.map(Some)
        } else {
            Some(None)
        },
        ..Default::default()
    }
}

const fn has_playback_progress_changes(patch: &UserDataPatch) -> bool {
    patch.playback_position_ticks.is_some()
        || patch.played.is_some()
        || patch.audio_stream_index.is_some()
        || patch.subtitle_stream_index.is_some()
}

fn playback_stop_effect(
    item: &base_item::Model,
    reported_position_ticks: Option<i64>,
    playstate_configuration: &PlaystateConfiguration,
) -> PlaybackStopEffect {
    let Some(reported_position_ticks) = reported_position_ticks else {
        return PlaybackStopEffect {
            playback_position_ticks: 0,
            played: Some(supports_played_status(item)),
            increment_play_count: true,
        };
    };

    let (playback_position_ticks, played) =
        update_play_state_effect(item, reported_position_ticks, playstate_configuration);
    PlaybackStopEffect {
        playback_position_ticks,
        played,
        increment_play_count: false,
    }
}

fn update_play_state_effect(
    item: &base_item::Model,
    reported_position_ticks: i64,
    configuration: &PlaystateConfiguration,
) -> (i64, Option<bool>) {
    let runtime_ticks = item.runtime_ticks.unwrap_or_default();
    let mut position_ticks = reported_position_ticks;
    let has_runtime = runtime_ticks > 0;
    let mut played = None;

    if position_ticks > 0 && has_runtime && !is_audio_book(item) && !is_book(item) {
        if playback_percentage_less_than(
            position_ticks,
            runtime_ticks,
            configuration.min_resume_pct,
        ) {
            position_ticks = 0;
        } else if playback_percentage_greater_than(
            position_ticks,
            runtime_ticks,
            configuration.max_resume_pct,
        ) || position_ticks >= runtime_ticks - TICKS_PER_SECOND
            || runtime_ticks < configuration.min_resume_duration_seconds * TICKS_PER_SECOND
        {
            position_ticks = 0;
            played = Some(true);
        }
    } else if position_ticks > 0 && has_runtime && is_audio_book(item) {
        if position_ticks < i64::from(configuration.min_audiobook_resume_minutes) * TICKS_PER_MINUTE
        {
            position_ticks = 0;
        } else if runtime_ticks - position_ticks
            < i64::from(configuration.max_audiobook_resume_minutes) * TICKS_PER_MINUTE
            || position_ticks >= runtime_ticks
        {
            position_ticks = 0;
            played = Some(true);
        }
    } else if !has_runtime {
        position_ticks = 0;
        played = Some(true);
    }

    if !supports_played_status(item) {
        position_ticks = 0;
        played = Some(false);
    }
    if !supports_position_ticks_resume(item) {
        position_ticks = 0;
    }

    (position_ticks, played)
}

fn should_mark_played_on_start(item: &base_item::Model) -> bool {
    supports_played_status(item) && !supports_position_ticks_resume(item)
}

fn should_propagate_played_state(item: &base_item::Model, data: &user_data::Model) -> bool {
    data.played && is_video_item(item)
}

fn supports_played_status(item: &base_item::Model) -> bool {
    if item.item_type == "Playlist" {
        return item
            .media_type
            .as_deref()
            .is_some_and(|media_type| media_type == "Video");
    }
    matches!(
        item.item_type.as_str(),
        "Audio"
            | "AudioBook"
            | "Book"
            | "BoxSet"
            | "Episode"
            | "Folder"
            | "Movie"
            | "MusicVideo"
            | "Season"
            | "Series"
            | "Trailer"
            | "Video"
    )
}

fn supports_position_ticks_resume(item: &base_item::Model) -> bool {
    matches!(item.item_type.as_str(), "AudioBook" | "Book") || is_video_item(item)
}

const TICKS_PER_SECOND: i64 = 10_000_000;
const TICKS_PER_MINUTE: i64 = 60 * TICKS_PER_SECOND;

fn is_audio_book(item: &base_item::Model) -> bool {
    item.item_type == "AudioBook"
}

fn is_book(item: &base_item::Model) -> bool {
    item.item_type == "Book"
}

fn is_video_item(item: &base_item::Model) -> bool {
    matches!(
        item.item_type.as_str(),
        "Episode" | "Movie" | "MusicVideo" | "Trailer" | "Video"
    )
}

fn playback_percentage_less_than(position_ticks: i64, runtime_ticks: i64, percent: i32) -> bool {
    i128::from(position_ticks) * 100 < i128::from(runtime_ticks) * i128::from(percent)
}

fn playback_percentage_greater_than(position_ticks: i64, runtime_ticks: i64, percent: i32) -> bool {
    i128::from(position_ticks) * 100 > i128::from(runtime_ticks) * i128::from(percent)
}
