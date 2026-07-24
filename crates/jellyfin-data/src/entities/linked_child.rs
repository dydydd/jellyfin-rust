use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "linked_children", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub parent_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub child_id: Uuid,
    pub child_type: i16,
    pub sort_order: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ParentId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Parent,
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::ChildId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Child,
}

impl ActiveModelBehavior for ActiveModel {}
