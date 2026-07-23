use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum MediaType {
    Unknown,
    Video,
    Audio,
    Photo,
    Book,
}

impl std::str::FromStr for MediaType {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum GeneralCommandType {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    PageUp,
    PageDown,
    PreviousLetter,
    NextLetter,
    ToggleOsd,
    ToggleContextMenu,
    Select,
    Back,
    TakeScreenshot,
    SendKey,
    SendString,
    GoHome,
    GoToSettings,
    VolumeUp,
    VolumeDown,
    Mute,
    Unmute,
    ToggleMute,
    SetVolume,
    SetAudioStreamIndex,
    SetSubtitleStreamIndex,
    ToggleFullscreen,
    DisplayContent,
    GoToSearch,
    DisplayMessage,
    SetRepeatMode,
    ChannelUp,
    ChannelDown,
    Guide,
    ToggleStats,
    PlayMediaSource,
    PlayTrailers,
    SetShuffleQueue,
    PlayState,
    PlayNext,
    ToggleOsdMenu,
    Play,
    SetMaxStreamingBitrate,
    SetPlaybackOrder,
}

impl std::str::FromStr for GeneralCommandType {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct ClientCapabilitiesDto {
    pub playable_media_types: Vec<MediaType>,
    pub supported_commands: Vec<GeneralCommandType>,
    pub supports_media_control: bool,
    pub supports_persistent_identifier: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_profile: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_store_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

impl Default for ClientCapabilitiesDto {
    fn default() -> Self {
        Self {
            playable_media_types: Vec::new(),
            supported_commands: Vec::new(),
            supports_media_control: false,
            supports_persistent_identifier: true,
            device_profile: None,
            app_store_url: None,
            icon_url: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct GeneralCommand {
    pub name: GeneralCommandType,
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub controlling_user_id: Uuid,
    pub arguments: HashMap<String, String>,
}

impl Default for GeneralCommand {
    fn default() -> Self {
        Self {
            name: GeneralCommandType::MoveUp,
            controlling_user_id: Uuid::nil(),
            arguments: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum PlayCommand {
    PlayNow,
    PlayNext,
    PlayLast,
    PlayInstantMix,
    PlayShuffle,
}

impl std::str::FromStr for PlayCommand {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PlayRequest {
    #[serde(
        serialize_with = "crate::serde_guid::vec::serialize",
        deserialize_with = "crate::serde_guid::vec::deserialize"
    )]
    pub item_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_position_ticks: Option<i64>,
    pub play_command: PlayCommand,
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub controlling_user_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_index: Option<i32>,
}

impl Default for PlayRequest {
    fn default() -> Self {
        Self {
            item_ids: Vec::new(),
            start_position_ticks: None,
            play_command: PlayCommand::PlayNow,
            controlling_user_id: Uuid::nil(),
            subtitle_stream_index: None,
            audio_stream_index: None,
            media_source_id: None,
            start_index: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum PlaystateCommand {
    #[default]
    Stop,
    Pause,
    Unpause,
    NextTrack,
    PreviousTrack,
    Seek,
    Rewind,
    FastForward,
    PlayPause,
}

impl std::str::FromStr for PlaystateCommand {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaystateRequest {
    pub command: PlaystateCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seek_position_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controlling_user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SessionUserInfo {
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub user_id: Uuid,
    pub user_name: String,
}

impl Default for SessionUserInfo {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            user_name: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct MessageCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct SessionInfoDto {
    pub additional_users: Vec<SessionUserInfo>,
    pub capabilities: ClientCapabilitiesDto,
    pub playable_media_types: Vec<MediaType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(with = "crate::serde_guid::single")]
    pub user_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_activity_date: DateTime<Utc>,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_playback_check_in: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_paused_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_version: Option<String>,
    pub is_active: bool,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub has_custom_device_name: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now_viewing_item: Option<Value>,
    pub supported_commands: Vec<GeneralCommandType>,
}

fn deserialize_enum_name<T>(value: &str) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::String(value.to_owned()))
}
