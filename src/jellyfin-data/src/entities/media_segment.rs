use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_segments", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub item_id: Uuid,
    pub segment_type: i32,
    pub start_ticks: i64,
    pub end_ticks: i64,
    pub segment_provider_id: String,
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
