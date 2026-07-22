use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "virtual_folders", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub name: String,
    pub normalized_name: String,
    pub collection_type: Option<String>,
    pub library_options: Value,
    pub refresh_requested: bool,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::media_path::Entity")]
    MediaPath,
}

impl Related<super::media_path::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MediaPath.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
