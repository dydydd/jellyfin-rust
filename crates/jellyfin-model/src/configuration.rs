use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{enums::SubtitlePlaybackMode, providers::ImageType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ImageOption {
    #[serde(rename = "Type")]
    pub image_type: ImageType,
    pub limit: i32,
    pub min_width: i32,
}

impl Default for ImageOption {
    fn default() -> Self {
        Self {
            image_type: ImageType::Primary,
            limit: 1,
            min_width: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct UserConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_language_preference: Option<String>,
    pub play_default_audio_track: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_language_preference: Option<String>,
    pub display_missing_episodes: bool,
    #[serde(with = "crate::serde_guid::vec")]
    pub grouped_folders: Vec<Uuid>,
    pub subtitle_mode: SubtitlePlaybackMode,
    pub display_collections_view: bool,
    pub enable_local_password: bool,
    #[serde(with = "crate::serde_guid::vec")]
    pub ordered_views: Vec<Uuid>,
    #[serde(with = "crate::serde_guid::vec")]
    pub latest_items_excludes: Vec<Uuid>,
    #[serde(with = "crate::serde_guid::vec")]
    pub my_media_excludes: Vec<Uuid>,
    pub hide_played_in_latest: bool,
    pub remember_audio_selections: bool,
    pub remember_subtitle_selections: bool,
    pub enable_next_episode_auto_play: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cast_receiver_id: Option<String>,
}

impl Default for UserConfiguration {
    fn default() -> Self {
        Self {
            audio_language_preference: None,
            play_default_audio_track: true,
            subtitle_language_preference: None,
            display_missing_episodes: false,
            grouped_folders: Vec::new(),
            subtitle_mode: SubtitlePlaybackMode::default(),
            display_collections_view: false,
            enable_local_password: false,
            ordered_views: Vec::new(),
            latest_items_excludes: Vec::new(),
            my_media_excludes: Vec::new(),
            hide_played_in_latest: true,
            remember_audio_selections: true,
            remember_subtitle_selections: true,
            enable_next_episode_auto_play: true,
            cast_receiver_id: None,
        }
    }
}
