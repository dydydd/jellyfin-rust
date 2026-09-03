use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "item_value_map", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_value_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::item_value::Entity",
        from = "Column::ItemValueId",
        to = "super::item_value::Column::ItemValueId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    ItemValue,
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ItemId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    BaseItem,
}

impl Related<super::item_value::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ItemValue.def()
    }
}

impl Related<super::base_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BaseItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
