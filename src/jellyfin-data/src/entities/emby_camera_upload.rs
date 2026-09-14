use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "emby_camera_uploads", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub device_id: String,
    pub upload_id: String,
    pub name: String,
    pub album: String,
    pub mime_type: Option<String>,
    pub date_created: Option<DateTime<Utc>>,
    pub stored_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
