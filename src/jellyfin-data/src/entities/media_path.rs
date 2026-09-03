use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_paths", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub virtual_folder_id: Uuid,
    pub path: String,
    pub normalized_path: String,
    pub path_ancestors: Value,
    pub path_info: Value,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::virtual_folder::Entity",
        from = "Column::VirtualFolderId",
        to = "super::virtual_folder::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    VirtualFolder,
}

impl Related<super::virtual_folder::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::VirtualFolder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
