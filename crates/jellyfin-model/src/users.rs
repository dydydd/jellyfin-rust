use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{DynamicDayOfWeek, SyncPlayUserAccessType, UnratedItem};

/// API representation of an access schedule.
///
/// Database-only `Id` and `UserId` properties are intentionally absent, as in
/// the official XML/API contract.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccessSchedule {
    pub day_of_week: DynamicDayOfWeek,
    pub start_hour: f64,
    pub end_hour: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserPolicy {
    pub is_administrator: bool,
    pub is_hidden: bool,
    pub enable_collection_management: bool,
    pub enable_subtitle_management: bool,
    pub enable_lyric_management: bool,
    pub is_disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_parental_rating: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_parental_sub_rating: Option<i32>,
    pub blocked_tags: Vec<String>,
    pub allowed_tags: Vec<String>,
    pub enable_user_preference_access: bool,
    pub access_schedules: Vec<AccessSchedule>,
    pub block_unrated_items: Vec<UnratedItem>,
    pub enable_remote_control_of_other_users: bool,
    pub enable_shared_device_control: bool,
    pub enable_remote_access: bool,
    pub enable_live_tv_management: bool,
    pub enable_live_tv_access: bool,
    pub enable_media_playback: bool,
    pub enable_audio_playback_transcoding: bool,
    pub enable_video_playback_transcoding: bool,
    pub enable_playback_remuxing: bool,
    pub force_remote_source_transcoding: bool,
    pub enable_content_deletion: bool,
    pub enable_content_deletion_from_folders: Vec<String>,
    pub enable_content_downloading: bool,
    pub enable_sync_transcoding: bool,
    pub enable_media_conversion: bool,
    pub enabled_devices: Vec<String>,
    pub enable_all_devices: bool,
    #[serde(with = "crate::serde_guid::vec")]
    pub enabled_channels: Vec<Uuid>,
    pub enable_all_channels: bool,
    #[serde(with = "crate::serde_guid::vec")]
    pub enabled_folders: Vec<Uuid>,
    pub enable_all_folders: bool,
    pub invalid_login_attempt_count: i32,
    pub login_attempts_before_lockout: i32,
    pub max_active_sessions: i32,
    pub enable_public_sharing: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option_vec"
    )]
    pub blocked_media_folders: Option<Vec<Uuid>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_guid::option_vec"
    )]
    pub blocked_channels: Option<Vec<Uuid>>,
    pub remote_client_bitrate_limit: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_reset_provider_id: Option<String>,
    pub sync_play_access: SyncPlayUserAccessType,
}

impl UserPolicy {
    pub const DEFAULT_AUTHENTICATION_PROVIDER_ID: &'static str =
        "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider";
    pub const DEFAULT_PASSWORD_RESET_PROVIDER_ID: &'static str =
        "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider";
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            is_administrator: false,
            is_hidden: true,
            enable_collection_management: false,
            enable_subtitle_management: false,
            enable_lyric_management: false,
            is_disabled: false,
            max_parental_rating: None,
            max_parental_sub_rating: None,
            blocked_tags: Vec::new(),
            allowed_tags: Vec::new(),
            enable_user_preference_access: true,
            access_schedules: Vec::new(),
            block_unrated_items: Vec::new(),
            enable_remote_control_of_other_users: false,
            enable_shared_device_control: true,
            enable_remote_access: true,
            enable_live_tv_management: true,
            enable_live_tv_access: true,
            enable_media_playback: true,
            enable_audio_playback_transcoding: true,
            enable_video_playback_transcoding: true,
            enable_playback_remuxing: true,
            force_remote_source_transcoding: false,
            enable_content_deletion: false,
            enable_content_deletion_from_folders: Vec::new(),
            enable_content_downloading: true,
            enable_sync_transcoding: true,
            enable_media_conversion: true,
            enabled_devices: Vec::new(),
            enable_all_devices: true,
            enabled_channels: Vec::new(),
            enable_all_channels: true,
            enabled_folders: Vec::new(),
            enable_all_folders: true,
            invalid_login_attempt_count: 0,
            login_attempts_before_lockout: -1,
            max_active_sessions: 0,
            enable_public_sharing: true,
            blocked_media_folders: None,
            blocked_channels: None,
            remote_client_bitrate_limit: 0,
            authentication_provider_id: None,
            password_reset_provider_id: None,
            sync_play_access: SyncPlayUserAccessType::CreateAndJoinGroups,
        }
    }
}
