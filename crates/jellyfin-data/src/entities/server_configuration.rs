use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "server_configuration", schema_name = "jellyfin")]
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
    pub enable_remote_access: bool,
    pub server_id: String,
    pub row_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tmdb_api_key: String,
    pub quick_connect_available: bool,
    pub omdb_api_key: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
