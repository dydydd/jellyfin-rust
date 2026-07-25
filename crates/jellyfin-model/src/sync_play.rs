use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupStateType {
    #[default]
    Idle,
    Waiting,
    Paused,
    Playing,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupQueueMode {
    #[default]
    Queue,
    QueueNext,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupRepeatMode {
    RepeatOne,
    RepeatAll,
    #[default]
    RepeatNone,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupShuffleMode {
    #[default]
    Sorted,
    Shuffle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendCommandType {
    Unpause,
    Pause,
    Stop,
    Seek,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SendCommandDto {
    #[serde(with = "crate::serde_guid::single")]
    pub group_id: Uuid,
    #[serde(with = "crate::serde_guid::single")]
    pub playlist_item_id: Uuid,
    #[serde(with = "crate::serde_datetime::required")]
    pub when: DateTime<Utc>,
    pub position_ticks: Option<i64>,
    pub command: SendCommandType,
    #[serde(with = "crate::serde_datetime::required")]
    pub emitted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupUpdateType {
    UserJoined,
    UserLeft,
    GroupJoined,
    GroupLeft,
    StateUpdate,
    PlayQueue,
    NotInGroup,
    GroupDoesNotExist,
    LibraryAccessDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GroupUpdateDto<T> {
    #[serde(with = "crate::serde_guid::single")]
    pub group_id: Uuid,
    pub data: T,
    #[serde(rename = "Type")]
    pub update_type: GroupUpdateType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayQueueUpdateReason {
    NewPlaylist,
    SetCurrentItem,
    RemoveItems,
    MoveItem,
    Queue,
    QueueNext,
    NextItem,
    PreviousItem,
    RepeatMode,
    ShuffleMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SyncPlayQueueItemDto {
    #[serde(with = "crate::serde_guid::single")]
    pub item_id: Uuid,
    #[serde(with = "crate::serde_guid::single")]
    pub playlist_item_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayQueueUpdateDto {
    pub reason: PlayQueueUpdateReason,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_update: DateTime<Utc>,
    pub playlist: Vec<SyncPlayQueueItemDto>,
    pub playing_item_index: i32,
    pub start_position_ticks: i64,
    pub is_playing: bool,
    pub shuffle_mode: GroupShuffleMode,
    pub repeat_mode: GroupRepeatMode,
}

/// Playback position reported by a member that has started buffering.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct BufferRequestDto {
    #[serde(with = "crate::serde_datetime::required")]
    pub when: DateTime<Utc>,
    pub position_ticks: i64,
    pub is_playing: bool,
    #[serde(with = "crate::serde_guid::single")]
    pub playlist_item_id: Uuid,
}

/// Playback position reported by a member that is ready to rejoin playback.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ReadyRequestDto {
    #[serde(with = "crate::serde_datetime::required")]
    pub when: DateTime<Utc>,
    pub position_ticks: i64,
    pub is_playing: bool,
    #[serde(with = "crate::serde_guid::single")]
    pub playlist_item_id: Uuid,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct IgnoreWaitRequestDto {
    pub ignore_wait: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PingRequestDto {
    pub ping: i64,
}

/// Public summary of a `SyncPlay` group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GroupInfoDto {
    #[serde(with = "crate::serde_guid::single")]
    pub group_id: Uuid,
    pub group_name: String,
    pub state: GroupStateType,
    pub participants: Vec<String>,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_updated_at: DateTime<Utc>,
}

impl GroupInfoDto {
    #[must_use]
    pub const fn new(
        group_id: Uuid,
        group_name: String,
        state: GroupStateType,
        participants: Vec<String>,
        last_updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            group_id,
            group_name,
            state,
            participants,
            last_updated_at,
        }
    }
}

/// Response returned by Jellyfin's high-level UTC time sync endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UtcTimeResponse {
    #[serde(with = "crate::serde_datetime::required")]
    pub request_reception_time: DateTime<Utc>,
    #[serde(with = "crate::serde_datetime::required")]
    pub response_transmission_time: DateTime<Utc>,
}

impl UtcTimeResponse {
    #[must_use]
    pub const fn new(
        request_reception_time: DateTime<Utc>,
        response_transmission_time: DateTime<Utc>,
    ) -> Self {
        Self {
            request_reception_time,
            response_transmission_time,
        }
    }
}
