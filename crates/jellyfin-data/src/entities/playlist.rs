use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "playlists", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub playlist_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub open_access: bool,
    pub media_type: Option<String>,
    pub shares: Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::base_item::Entity",
        from = "Column::PlaylistId",
        to = "super::base_item::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    BaseItem,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::OwnerUserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "SetNull"
    )]
    Owner,
}

impl ActiveModelBehavior for ActiveModel {}

impl Related<super::base_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BaseItem.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Owner.def()
    }
}
