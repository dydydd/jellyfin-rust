use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "devices", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub user_id: Uuid,
    /// Recoverable token required by Jellyfin session responses and logout.
    pub access_token: String,
    pub app_name: String,
    pub app_version: String,
    pub device_name: String,
    pub device_id: String,
    pub is_active: bool,
    pub capabilities: Value,
    pub play_state: Value,
    pub now_playing_item: Option<Value>,
    pub now_playing_queue: Value,
    pub playlist_item_id: Option<String>,
    pub now_viewing_item: Option<Value>,
    pub additional_users: Value,
    pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    pub date_last_activity: DateTime<Utc>,
    pub date_last_paused: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
