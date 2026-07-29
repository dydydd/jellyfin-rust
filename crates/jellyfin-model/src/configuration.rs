use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    enums::SubtitlePlaybackMode,
    metadata_editor::NameValuePair,
    providers::ImageType,
    system::{CastReceiverApplication, RepositoryInfo},
};

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum ImageSavingConvention {
    #[default]
    Legacy,
    Compatible,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum ImageResolution {
    #[default]
    MatchSource = 0,
    #[serde(rename = "P144")]
    P144 = 1,
    #[serde(rename = "P240")]
    P240 = 2,
    #[serde(rename = "P360")]
    P360 = 3,
    #[serde(rename = "P480")]
    P480 = 4,
    #[serde(rename = "P720")]
    P720 = 5,
    #[serde(rename = "P1080")]
    P1080 = 6,
    #[serde(rename = "P1440")]
    P1440 = 7,
    #[serde(rename = "P2160")]
    P2160 = 8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum TrickplayScanBehavior {
    Blocking,
    #[default]
    NonBlocking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct MetadataOptions {
    pub item_type: String,
    pub disabled_metadata_savers: Vec<String>,
    pub local_metadata_reader_order: Vec<String>,
    pub disabled_metadata_fetchers: Vec<String>,
    pub metadata_fetcher_order: Vec<String>,
    pub disabled_image_fetchers: Vec<String>,
    pub image_fetcher_order: Vec<String>,
}

impl MetadataOptions {
    #[must_use]
    pub fn for_item_type(item_type: impl Into<String>) -> Self {
        Self {
            item_type: item_type.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn official_defaults() -> Vec<Self> {
        let mut music_video = Self::for_item_type("MusicVideo");
        music_video.disabled_metadata_fetchers = vec!["The Open Movie Database".to_owned()];
        music_video.disabled_image_fetchers = vec!["The Open Movie Database".to_owned()];

        let mut music_album = Self::for_item_type("MusicAlbum");
        music_album.disabled_metadata_fetchers = vec!["TheAudioDB".to_owned()];

        let mut music_artist = Self::for_item_type("MusicArtist");
        music_artist.disabled_metadata_fetchers = vec!["TheAudioDB".to_owned()];

        vec![
            Self::for_item_type("Book"),
            Self::for_item_type("Movie"),
            music_video,
            Self::for_item_type("Series"),
            music_album,
            music_artist,
            Self::for_item_type("BoxSet"),
            Self::for_item_type("Season"),
            Self::for_item_type("Episode"),
        ]
    }
}

impl Default for MetadataOptions {
    fn default() -> Self {
        Self {
            item_type: String::new(),
            disabled_metadata_savers: Vec::new(),
            local_metadata_reader_order: Vec::new(),
            disabled_metadata_fetchers: Vec::new(),
            metadata_fetcher_order: Vec::new(),
            disabled_image_fetchers: Vec::new(),
            image_fetcher_order: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PathSubstitution {
    pub from: String,
    pub to: String,
}

impl Default for PathSubstitution {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct TrickplayOptions {
    pub enable_hw_acceleration: bool,
    pub enable_hw_encoding: bool,
    pub enable_key_frame_only_extraction: bool,
    pub scan_behavior: TrickplayScanBehavior,
    pub process_priority: String,
    pub interval: i32,
    pub width_resolutions: Vec<i32>,
    pub tile_width: i32,
    pub tile_height: i32,
    pub qscale: i32,
    pub jpeg_quality: i32,
    pub process_threads: i32,
}

impl Default for TrickplayOptions {
    fn default() -> Self {
        Self {
            enable_hw_acceleration: false,
            enable_hw_encoding: false,
            enable_key_frame_only_extraction: false,
            scan_behavior: TrickplayScanBehavior::NonBlocking,
            process_priority: "BelowNormal".to_owned(),
            interval: 10_000,
            width_resolutions: vec![320],
            tile_width: 10,
            tile_height: 10,
            qscale: 4,
            jpeg_quality: 90,
            process_threads: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct ServerConfiguration {
    pub log_file_retention_days: i32,
    pub is_startup_wizard_completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version_str: Option<String>,
    pub enable_metrics: bool,
    pub enable_normalized_item_by_name_ids: bool,
    pub is_port_authorized: bool,
    pub quick_connect_available: bool,
    pub enable_case_sensitive_item_ids: bool,
    pub disable_live_tv_channel_user_data_name: bool,
    pub metadata_path: String,
    pub preferred_metadata_language: String,
    pub metadata_country_code: String,
    pub sort_replace_characters: Vec<String>,
    pub sort_remove_characters: Vec<String>,
    pub sort_remove_words: Vec<String>,
    pub min_resume_pct: i32,
    pub max_resume_pct: i32,
    pub min_resume_duration_seconds: i32,
    pub min_audiobook_resume: i32,
    pub max_audiobook_resume: i32,
    pub inactive_session_threshold: i32,
    pub library_monitor_delay: i32,
    pub library_update_duration: i32,
    pub cache_size: i32,
    pub image_saving_convention: ImageSavingConvention,
    pub metadata_options: Vec<MetadataOptions>,
    pub skip_deserialization_for_basic_types: bool,
    pub server_name: String,
    #[serde(rename = "UICulture")]
    pub ui_culture: String,
    pub save_metadata_hidden: bool,
    pub content_types: Vec<NameValuePair>,
    pub remote_client_bitrate_limit: i32,
    pub enable_folder_view: bool,
    pub enable_grouping_movies_into_collections: bool,
    pub enable_grouping_shows_into_collections: bool,
    pub display_specials_within_seasons: bool,
    pub codecs_used: Vec<String>,
    pub plugin_repositories: Vec<RepositoryInfo>,
    pub enable_external_content_in_suggestions: bool,
    pub image_extraction_timeout_ms: i32,
    pub path_substitutions: Vec<PathSubstitution>,
    pub enable_slow_response_warning: bool,
    pub slow_response_threshold_ms: i64,
    pub cors_hosts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_log_retention_days: Option<i32>,
    pub library_scan_fanout_concurrency: i32,
    pub library_metadata_refresh_concurrency: i32,
    pub allow_client_log_upload: bool,
    pub dummy_chapter_duration: i32,
    pub chapter_image_resolution: ImageResolution,
    pub parallel_image_encoding_limit: i32,
    pub cast_receiver_applications: Vec<CastReceiverApplication>,
    pub trickplay_options: TrickplayOptions,
    pub enable_legacy_authorization: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tmdb_api_key: String,
}

impl Default for ServerConfiguration {
    fn default() -> Self {
        Self {
            log_file_retention_days: 3,
            is_startup_wizard_completed: false,
            cache_path: None,
            previous_version: None,
            previous_version_str: None,
            enable_metrics: false,
            enable_normalized_item_by_name_ids: true,
            is_port_authorized: false,
            quick_connect_available: true,
            enable_case_sensitive_item_ids: true,
            disable_live_tv_channel_user_data_name: true,
            metadata_path: String::new(),
            preferred_metadata_language: "en".to_owned(),
            metadata_country_code: "US".to_owned(),
            sort_replace_characters: vec![".".to_owned(), "+".to_owned(), "%".to_owned()],
            sort_remove_characters: vec![
                ",".to_owned(),
                "&".to_owned(),
                "-".to_owned(),
                "{".to_owned(),
                "}".to_owned(),
                "'".to_owned(),
            ],
            sort_remove_words: vec!["the".to_owned(), "a".to_owned(), "an".to_owned()],
            min_resume_pct: 5,
            max_resume_pct: 90,
            min_resume_duration_seconds: 300,
            min_audiobook_resume: 5,
            max_audiobook_resume: 5,
            inactive_session_threshold: 0,
            library_monitor_delay: 60,
            library_update_duration: 30,
            cache_size: default_cache_size(),
            image_saving_convention: ImageSavingConvention::Legacy,
            metadata_options: MetadataOptions::official_defaults(),
            skip_deserialization_for_basic_types: true,
            server_name: String::new(),
            ui_culture: "en-US".to_owned(),
            save_metadata_hidden: false,
            content_types: Vec::new(),
            remote_client_bitrate_limit: 0,
            enable_folder_view: false,
            enable_grouping_movies_into_collections: false,
            enable_grouping_shows_into_collections: false,
            display_specials_within_seasons: true,
            codecs_used: Vec::new(),
            plugin_repositories: Vec::new(),
            enable_external_content_in_suggestions: true,
            image_extraction_timeout_ms: 0,
            path_substitutions: Vec::new(),
            enable_slow_response_warning: true,
            slow_response_threshold_ms: 500,
            cors_hosts: vec!["*".to_owned()],
            activity_log_retention_days: Some(30),
            library_scan_fanout_concurrency: 0,
            library_metadata_refresh_concurrency: 0,
            allow_client_log_upload: true,
            dummy_chapter_duration: 0,
            chapter_image_resolution: ImageResolution::MatchSource,
            parallel_image_encoding_limit: 0,
            cast_receiver_applications: Vec::new(),
            trickplay_options: TrickplayOptions::default(),
            enable_legacy_authorization: false,
            tmdb_api_key: String::new(),
        }
    }
}

fn default_cache_size() -> i32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .saturating_mul(100)
        .try_into()
        .unwrap_or(i32::MAX)
}
