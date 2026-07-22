use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "people", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub name: String,
    pub clean_name: String,
    pub provider_ids: Value,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    pub row_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::person_base_item_map::Entity")]
    BaseItemMap,
}

impl Related<super::person_base_item_map::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BaseItemMap.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
