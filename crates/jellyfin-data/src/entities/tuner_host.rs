use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tuner_hosts", schema_name = "jellyfin")]
#[allow(clippy::struct_excessive_bools)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub url: String,
    pub tuner_type: String,
    pub device_id: Option<String>,
    pub friendly_name: Option<String>,
    pub import_favorites_only: bool,
    pub allow_hw_transcoding: bool,
    pub allow_fmp4_transcoding_container: bool,
    pub allow_stream_sharing: bool,
    pub fallback_max_streaming_bitrate: i32,
    pub enable_stream_looping: bool,
    pub source: Option<String>,
    pub tuner_count: i32,
    pub user_agent: Option<String>,
    pub ignore_dts: bool,
    pub read_at_native_framerate: bool,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    pub row_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
