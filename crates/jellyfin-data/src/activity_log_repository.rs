use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::NotSet,
    ColumnTrait, DatabaseConnection, DbErr, DeleteResult, EntityTrait, JoinType, Order,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Select, Set,
    sea_query::{Expr, extension::postgres::PgExpr},
};
use thiserror::Error;
use uuid::Uuid;

use crate::entities::{activity_log, user};

const DEFAULT_PAGE_SIZE: u64 = 100;

/// Data required to create an activity log entry.
#[derive(Debug, Clone)]
pub struct NewActivityLog {
    pub name: String,
    pub activity_type: String,
    pub user_id: Uuid,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    pub item_id: Option<String>,
    pub date_created: Option<DateTime<Utc>>,
    pub log_severity: activity_log::LogSeverity,
}

impl NewActivityLog {
    #[must_use]
    pub fn new(name: impl Into<String>, activity_type: impl Into<String>, user_id: Uuid) -> Self {
        Self {
            name: name.into(),
            activity_type: activity_type.into(),
            user_id,
            overview: None,
            short_overview: None,
            item_id: None,
            date_created: None,
            log_severity: activity_log::LogSeverity::Information,
        }
    }
}

/// Supported activity log sort columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLogSortBy {
    Name,
    Overview,
    ShortOverview,
    Type,
    DateCreated,
    Username,
    LogSeverity,
}

/// Direction for an activity log sort column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Filters and pagination for activity log queries.
#[derive(Debug, Clone, Default)]
pub struct ActivityLogQuery {
    pub skip: Option<u64>,
    pub limit: Option<u64>,
    pub has_user_id: Option<bool>,
    pub min_date: Option<DateTime<Utc>>,
    pub max_date: Option<DateTime<Utc>>,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    pub activity_type: Option<String>,
    pub item_id: Option<Uuid>,
    pub username: Option<String>,
    pub severity: Option<activity_log::LogSeverity>,
    pub order_by: Vec<(ActivityLogSortBy, SortDirection)>,
}

/// A page of activity log entries and the unpaged match count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityLogPage {
    pub start_index: Option<u64>,
    pub total_record_count: u64,
    pub items: Vec<activity_log::Model>,
}

#[derive(Debug, Error)]
pub enum ActivityLogError {
    #[error("activity log {0} cannot be empty")]
    EmptyField(&'static str),
    #[error("activity log {field} exceeds its {max} character limit")]
    FieldTooLong { field: &'static str, max: usize },
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// PostgreSQL-backed activity log persistence and query operations.
#[derive(Clone)]
pub struct ActivityLogRepository {
    database: DatabaseConnection,
}

impl ActivityLogRepository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Creates one activity log entry.
    ///
    /// # Errors
    ///
    /// Returns a validation error for missing or oversized fields, or a
    /// database error when insertion fails.
    pub async fn create(
        &self,
        entry: NewActivityLog,
    ) -> Result<activity_log::Model, ActivityLogError> {
        validate_required("name", &entry.name, 512)?;
        validate_required("type", &entry.activity_type, 256)?;
        validate_optional("overview", entry.overview.as_deref(), 512)?;
        validate_optional("short overview", entry.short_overview.as_deref(), 512)?;
        validate_optional("item id", entry.item_id.as_deref(), 256)?;

        Ok(activity_log::ActiveModel {
            id: NotSet,
            name: Set(entry.name),
            overview: Set(entry.overview),
            short_overview: Set(entry.short_overview),
            activity_type: Set(entry.activity_type),
            user_id: Set(entry.user_id),
            item_id: Set(entry.item_id),
            date_created: entry.date_created.map_or(NotSet, Set),
            log_severity: Set(entry.log_severity),
            row_version: NotSet,
        }
        .insert(&self.database)
        .await?)
    }

    /// Returns a filtered, ordered page and total match count.
    ///
    /// # Errors
    ///
    /// Returns a database error when counting or loading entries fails.
    pub async fn query(
        &self,
        query: &ActivityLogQuery,
    ) -> Result<ActivityLogPage, ActivityLogError> {
        let requires_user_join = query.username.is_some()
            || query
                .order_by
                .iter()
                .any(|(column, _)| *column == ActivityLogSortBy::Username);
        let mut entries = activity_log::Entity::find();
        if requires_user_join {
            entries = entries.join(JoinType::LeftJoin, activity_log::Relation::User.def());
        }

        entries = apply_filters(entries, query);
        let total_record_count = entries.clone().count(&self.database).await?;
        entries = apply_ordering(entries, &query.order_by);

        let items = entries
            .offset(query.skip.unwrap_or(0))
            .limit(query.limit.unwrap_or(DEFAULT_PAGE_SIZE))
            .all(&self.database)
            .await?;

        Ok(ActivityLogPage {
            start_index: query.skip,
            total_record_count,
            items,
        })
    }

    /// Deletes entries created at or before `cutoff`.
    ///
    /// # Errors
    ///
    /// Returns a database error when deletion fails.
    pub async fn clean(&self, cutoff: DateTime<Utc>) -> Result<u64, ActivityLogError> {
        let DeleteResult { rows_affected } = activity_log::Entity::delete_many()
            .filter(activity_log::Column::DateCreated.lte(cutoff))
            .exec(&self.database)
            .await?;
        Ok(rows_affected)
    }
}

fn apply_filters(
    mut entries: Select<activity_log::Entity>,
    query: &ActivityLogQuery,
) -> Select<activity_log::Entity> {
    if let Some(has_user_id) = query.has_user_id {
        let condition = if has_user_id {
            activity_log::Column::UserId.ne(Uuid::nil())
        } else {
            activity_log::Column::UserId.eq(Uuid::nil())
        };
        entries = entries.filter(condition);
    }
    if let Some(min_date) = query.min_date {
        entries = entries.filter(activity_log::Column::DateCreated.gte(min_date));
    }
    if let Some(max_date) = query.max_date {
        entries = entries.filter(activity_log::Column::DateCreated.lte(max_date));
    }

    let text_filters = [
        (activity_log::Column::Name, query.name.as_deref()),
        (activity_log::Column::Overview, query.overview.as_deref()),
        (
            activity_log::Column::ShortOverview,
            query.short_overview.as_deref(),
        ),
        (
            activity_log::Column::ActivityType,
            query.activity_type.as_deref(),
        ),
    ];
    for (column, value) in text_filters {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            entries = entries
                .filter(Expr::col((activity_log::Entity, column)).ilike(format!("%{value}%")));
        }
    }

    if let Some(item_id) = query.item_id {
        entries = entries.filter(activity_log::Column::ItemId.eq(item_id.simple().to_string()));
    }
    if let Some(username) = query.username.as_deref().filter(|value| !value.is_empty()) {
        entries = entries.filter(
            Expr::col((user::Entity, user::Column::Username)).ilike(format!("%{username}%")),
        );
    }
    if let Some(severity) = query.severity {
        entries = entries.filter(activity_log::Column::LogSeverity.eq(severity));
    }

    entries
}

fn apply_ordering(
    mut entries: Select<activity_log::Entity>,
    order_by: &[(ActivityLogSortBy, SortDirection)],
) -> Select<activity_log::Entity> {
    if order_by.is_empty() {
        return entries
            .order_by_desc(activity_log::Column::DateCreated)
            .order_by_desc(activity_log::Column::Id);
    }

    for &(column, direction) in order_by {
        let column = match column {
            ActivityLogSortBy::Name => {
                Expr::col((activity_log::Entity, activity_log::Column::Name))
            }
            ActivityLogSortBy::Overview => {
                Expr::col((activity_log::Entity, activity_log::Column::Overview))
            }
            ActivityLogSortBy::ShortOverview => {
                Expr::col((activity_log::Entity, activity_log::Column::ShortOverview))
            }
            ActivityLogSortBy::Type => {
                Expr::col((activity_log::Entity, activity_log::Column::ActivityType))
            }
            ActivityLogSortBy::DateCreated => {
                Expr::col((activity_log::Entity, activity_log::Column::DateCreated))
            }
            ActivityLogSortBy::Username => Expr::col((user::Entity, user::Column::Username)),
            ActivityLogSortBy::LogSeverity => {
                Expr::col((activity_log::Entity, activity_log::Column::LogSeverity))
            }
        };
        let direction = match direction {
            SortDirection::Ascending => Order::Asc,
            SortDirection::Descending => Order::Desc,
        };
        entries = entries.order_by(column, direction);
    }

    entries
}

fn validate_required(field: &'static str, value: &str, max: usize) -> Result<(), ActivityLogError> {
    if value.is_empty() {
        return Err(ActivityLogError::EmptyField(field));
    }
    validate_length(field, value, max)
}

fn validate_optional(
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<(), ActivityLogError> {
    value.map_or(Ok(()), |value| validate_length(field, value, max))
}

fn validate_length(field: &'static str, value: &str, max: usize) -> Result<(), ActivityLogError> {
    if value.chars().count() > max {
        Err(ActivityLogError::FieldTooLong { field, max })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_required_and_bounded_fields() {
        assert!(matches!(
            validate_required("name", "", 512),
            Err(ActivityLogError::EmptyField("name"))
        ));
        assert!(matches!(
            validate_optional("overview", Some(&"x".repeat(513)), 512),
            Err(ActivityLogError::FieldTooLong {
                field: "overview",
                max: 512
            })
        ));
    }
}
