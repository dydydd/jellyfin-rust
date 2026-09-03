use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "trickplay_infos", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub width: i32,
    pub height: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub thumbnail_count: i32,
    pub interval: i32,
    pub bandwidth: i32,
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
