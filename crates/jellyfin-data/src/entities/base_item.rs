use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "base_items", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub item_type: String,
    pub data: Option<Value>,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub top_parent_id: Option<Uuid>,
    pub name: Option<String>,
    pub sort_name: Option<String>,
    pub media_type: Option<String>,
    pub overview: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub runtime_ticks: Option<i64>,
    pub is_folder: bool,
    pub is_virtual_item: bool,
    pub presentation_unique_key: Option<String>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub series_presentation_unique_key: Option<String>,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    pub row_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "Entity",
        from = "Column::ParentId",
        to = "Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Parent,
}

impl ActiveModelBehavior for ActiveModel {}
