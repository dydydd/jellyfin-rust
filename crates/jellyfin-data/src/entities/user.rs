use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "users", schema_name = "jellyfin")]
#[allow(clippy::struct_excessive_bools)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub username: String,
    pub normalized_username: String,
    pub password_hash: Option<String>,
    pub must_update_password: bool,
    pub is_administrator: bool,
    pub is_hidden: bool,
    pub is_disabled: bool,
    pub enable_auto_login: bool,
    pub last_login_date: Option<DateTime<Utc>>,
    pub last_activity_date: Option<DateTime<Utc>>,
    pub authentication_provider_id: String,
    pub password_reset_provider_id: String,
    pub policy: Value,
    pub preferences: Value,
    pub row_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_one = "super::user_profile_image::Entity")]
    ProfileImage,
}

impl Related<super::user_profile_image::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ProfileImage.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
