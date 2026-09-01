use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "server_configuration", schema_name = "jellyfin")]
#[allow(clippy::struct_excessive_bools)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i16,
    pub server_name: String,
    pub ui_culture: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
    pub is_startup_wizard_completed: bool,
    pub content_types: Value,
    pub plugin_repositories: Value,
    pub min_resume_pct: i32,
    pub max_resume_pct: i32,
    pub min_resume_duration_seconds: i32,
    pub min_audiobook_resume: i32,
    pub max_audiobook_resume: i32,
    pub allow_client_log_upload: bool,
    pub trickplay_options: Value,
    pub cast_receiver_applications: Value,
    pub enable_remote_access: bool,
    pub server_id: String,
    pub row_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tmdb_api_key: String,
    pub quick_connect_available: bool,
    pub omdb_api_key: String,
    pub log_file_retention_days: i32,
    pub enable_metrics: bool,
    pub enable_normalized_item_by_name_ids: bool,
    pub metadata_path: String,
    pub sort_replace_characters: Value,
    pub sort_remove_characters: Value,
    pub sort_remove_words: Value,
    pub inactive_session_threshold: i32,
    pub library_monitor_delay: i32,
    pub library_update_duration: i32,
    pub cache_size: Option<i32>,
    pub image_saving_convention: i16,
    pub save_metadata_hidden: bool,
    pub remote_client_bitrate_limit: i32,
    pub enable_folder_view: bool,
    pub enable_grouping_movies_into_collections: bool,
    pub enable_grouping_shows_into_collections: bool,
    pub display_specials_within_seasons: bool,
    pub enable_external_content_in_suggestions: bool,
    pub cors_hosts: Value,
    pub activity_log_retention_days: Option<i32>,
    pub library_scan_fanout_concurrency: i32,
    pub library_metadata_refresh_concurrency: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
