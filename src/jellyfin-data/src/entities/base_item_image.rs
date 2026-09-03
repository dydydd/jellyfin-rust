use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "base_item_images", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub image_type: i16,
    #[sea_orm(primary_key, auto_increment = false)]
    pub image_index: i32,
    pub path: String,
    pub date_modified: DateTime<Utc>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub blurhash: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ItemId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    BaseItem,
}

impl Related<super::base_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BaseItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
