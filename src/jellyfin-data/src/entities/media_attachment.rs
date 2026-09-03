use sea_orm::entity::prelude::*;

// The entity intentionally mirrors Jellyfin's normalized attachment columns.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "media_attachments", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub attachment_index: i32,
    pub codec: Option<String>,
    pub codec_tag: Option<String>,
    pub comment: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub delivery_url: Option<String>,
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
