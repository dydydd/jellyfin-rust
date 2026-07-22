use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

/// Severity values used by Jellyfin activity log entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "i16", db_type = "SmallInteger")]
pub enum LogSeverity {
    #[sea_orm(num_value = 0)]
    Trace,
    #[sea_orm(num_value = 1)]
    Debug,
    #[sea_orm(num_value = 2)]
    #[default]
    Information,
    #[sea_orm(num_value = 3)]
    Warning,
    #[sea_orm(num_value = 4)]
    Error,
    #[sea_orm(num_value = 5)]
    Critical,
    #[sea_orm(num_value = 6)]
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "activity_logs", schema_name = "jellyfin")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    pub activity_type: String,
    pub user_id: Uuid,
    pub item_id: Option<String>,
    pub date_created: DateTime<Utc>,
    pub log_severity: LogSeverity,
    pub row_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
