use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "ancestor_ids", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub parent_item_id: Uuid,
    pub depth: i32,
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
    Item,
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ParentItemId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Parent,
}

impl ActiveModelBehavior for ActiveModel {}
