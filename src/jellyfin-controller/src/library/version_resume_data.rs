use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Per-user playback state used to select a multi-version item's resume state.
#[derive(Debug, Clone, PartialEq)]
pub struct UserItemData {
    pub key: String,
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub last_played_date: Option<DateTime<Utc>>,
    pub played: bool,
}

impl UserItemData {
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            playback_position_ticks: 0,
            play_count: 0,
            last_played_date: None,
            played: false,
        }
    }

    /// Clears all watched and resume state while retaining unrelated fields.
    pub fn reset_played_state(&mut self) {
        self.playback_position_ticks = 0;
        self.play_count = 0;
        self.last_played_date = None;
        self.played = false;
    }
}

/// API-facing playback state updated with the winning version's completion.
#[derive(Debug, Clone, PartialEq)]
pub struct UserItemDataDto {
    pub played_percentage: Option<f64>,
    pub playback_position_ticks: i64,
    pub last_played_date: Option<DateTime<Utc>>,
    pub played: bool,
    pub key: String,
    pub item_id: Uuid,
}

impl UserItemDataDto {
    #[must_use]
    pub fn new(item_id: Uuid, key: impl Into<String>) -> Self {
        Self {
            played_percentage: None,
            playback_position_ticks: 0,
            last_played_date: None,
            played: false,
            key: key.into(),
            item_id,
        }
    }
}

/// Most recently played alternate version and its per-user playback state.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionResumeData {
    pub version_id: Uuid,
    pub user_data: UserItemData,
}

impl VersionResumeData {
    #[must_use]
    pub const fn new(version_id: Uuid, user_data: UserItemData) -> Self {
        Self {
            version_id,
            user_data,
        }
    }

    /// Propagates completion and the most recent playback date to `dto`.
    ///
    /// A resume position belongs to its individual media version. Finishing a
    /// different version clears a stale primary-version resume bar, while an
    /// in-progress alternate version leaves the primary position unchanged.
    pub fn apply_to(&self, dto: &mut UserItemDataDto) {
        dto.played |= self.user_data.played;

        if self.user_data.last_played_date.is_some_and(|last_played| {
            dto.last_played_date
                .is_none_or(|dto_last_played| last_played > dto_last_played)
        }) {
            dto.last_played_date = self.user_data.last_played_date;
        }

        if self.version_id != dto.item_id
            && self.user_data.played
            && self.user_data.playback_position_ticks <= 0
        {
            dto.playback_position_ticks = 0;
            dto.played_percentage = None;
        }
    }
}
