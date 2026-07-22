use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    http::HeaderMap,
};
use jellyfin_data::{
    ActivityLogQuery, ActivityLogSortBy, SortDirection,
    entities::activity_log::{self, LogSeverity},
};
use sea_orm::prelude::DateTimeUtc;
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ActivityLogParameters {
    start_index: Option<i32>,
    limit: Option<i32>,
    min_date: Option<DateTimeUtc>,
    max_date: Option<DateTimeUtc>,
    has_user_id: Option<bool>,
    name: Option<String>,
    overview: Option<String>,
    short_overview: Option<String>,
    #[serde(rename = "type")]
    activity_type: Option<String>,
    item_id: Option<Uuid>,
    username: Option<String>,
    severity: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ActivityLogResult {
    items: Vec<ActivityLogEntry>,
    total_record_count: u64,
    start_index: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ActivityLogEntry {
    id: i64,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_overview: Option<String>,
    #[serde(rename = "Type")]
    activity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<String>,
    #[serde(serialize_with = "serialize_date")]
    date: DateTimeUtc,
    user_id: String,
    severity: &'static str,
}

pub(crate) async fn entries(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    parameters: Result<Query<ActivityLogParameters>, QueryRejection>,
) -> Result<Json<ActivityLogResult>, ApiError> {
    let session = authentication::authenticated_session(&state, &headers).await?;
    if !session.user.is_administrator {
        return Err(ApiError::Forbidden);
    }
    let Query(parameters) = parameters.map_err(|_| ApiError::InvalidRequest)?;
    let query = parameters.try_into_query()?;
    let page = state.activity_logs.query(&query).await?;

    Ok(Json(ActivityLogResult {
        items: page.items.into_iter().map(ActivityLogEntry::from).collect(),
        total_record_count: page.total_record_count,
        start_index: page.start_index.unwrap_or(0),
    }))
}

impl ActivityLogParameters {
    fn try_into_query(self) -> Result<ActivityLogQuery, ApiError> {
        let sort_by = parse_csv(self.sort_by.as_deref(), parse_sort_by)?;
        let sort_order = parse_csv(self.sort_order.as_deref(), parse_sort_direction)?;
        let severity = self
            .severity
            .as_deref()
            .map(|value| parse_severity(value).ok_or(ApiError::InvalidRequest))
            .transpose()?;
        let start_index = self
            .start_index
            .map(u64::try_from)
            .transpose()
            .map_err(|_| ApiError::InvalidRequest)?;
        let limit = self
            .limit
            .map(u64::try_from)
            .transpose()
            .map_err(|_| ApiError::InvalidRequest)?;
        if sort_order.len() > sort_by.len() {
            return Err(ApiError::InvalidRequest);
        }
        let fallback_direction = sort_order
            .first()
            .copied()
            .unwrap_or(SortDirection::Ascending);
        let order_by = sort_by
            .into_iter()
            .enumerate()
            .map(|(index, column)| {
                (
                    column,
                    sort_order.get(index).copied().unwrap_or(fallback_direction),
                )
            })
            .collect();

        Ok(ActivityLogQuery {
            skip: start_index,
            limit,
            min_date: self.min_date,
            max_date: self.max_date,
            has_user_id: self.has_user_id,
            name: self.name,
            overview: self.overview,
            short_overview: self.short_overview,
            activity_type: self.activity_type,
            item_id: self.item_id,
            username: self.username,
            severity,
            order_by,
        })
    }
}

impl From<activity_log::Model> for ActivityLogEntry {
    fn from(entry: activity_log::Model) -> Self {
        Self {
            id: entry.id,
            name: entry.name,
            overview: entry.overview,
            short_overview: entry.short_overview,
            activity_type: entry.activity_type,
            item_id: entry.item_id,
            date: entry.date_created,
            user_id: entry.user_id.simple().to_string(),
            severity: severity_name(entry.log_severity),
        }
    }
}

fn parse_csv<T>(
    value: Option<&str>,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Vec<T>, ApiError> {
    value.filter(|value| !value.is_empty()).map_or_else(
        || Ok(Vec::new()),
        |value| {
            value
                .split(',')
                .map(str::trim)
                .map(|value| parse(value).ok_or(ApiError::InvalidRequest))
                .collect()
        },
    )
}

fn parse_sort_by(value: &str) -> Option<ActivityLogSortBy> {
    match value.to_ascii_lowercase().as_str() {
        "name" | "0" => Some(ActivityLogSortBy::Name),
        "overiew" | "overview" | "1" => Some(ActivityLogSortBy::Overview),
        "shortoverview" | "2" => Some(ActivityLogSortBy::ShortOverview),
        "type" | "3" => Some(ActivityLogSortBy::Type),
        "datecreated" | "5" => Some(ActivityLogSortBy::DateCreated),
        "username" | "6" => Some(ActivityLogSortBy::Username),
        "logseverity" | "7" => Some(ActivityLogSortBy::LogSeverity),
        _ => None,
    }
}

fn parse_sort_direction(value: &str) -> Option<SortDirection> {
    match value.to_ascii_lowercase().as_str() {
        "ascending" | "0" => Some(SortDirection::Ascending),
        "descending" | "1" => Some(SortDirection::Descending),
        _ => None,
    }
}

fn parse_severity(value: &str) -> Option<LogSeverity> {
    match value.to_ascii_lowercase().as_str() {
        "trace" | "0" => Some(LogSeverity::Trace),
        "debug" | "1" => Some(LogSeverity::Debug),
        "information" | "2" => Some(LogSeverity::Information),
        "warning" | "3" => Some(LogSeverity::Warning),
        "error" | "4" => Some(LogSeverity::Error),
        "critical" | "5" => Some(LogSeverity::Critical),
        "none" | "6" => Some(LogSeverity::None),
        _ => None,
    }
}

const fn severity_name(severity: LogSeverity) -> &'static str {
    match severity {
        LogSeverity::Trace => "Trace",
        LogSeverity::Debug => "Debug",
        LogSeverity::Information => "Information",
        LogSeverity::Warning => "Warning",
        LogSeverity::Error => "Error",
        LogSeverity::Critical => "Critical",
        LogSeverity::None => "None",
    }
}

fn serialize_date<S>(value: &DateTimeUtc, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let nanoseconds = value.format("%f").to_string();
    let mut output = format!(
        "{}.{}Z",
        value.format("%Y-%m-%dT%H:%M:%S"),
        &nanoseconds[..7]
    );
    if value.timestamp_subsec_millis() != 0 {
        output.pop();
        while output.ends_with('0') {
            output.pop();
        }
        output.push('Z');
    }
    serializer.serialize_str(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_sort_and_severity_values() {
        assert_eq!(parse_sort_by("Overiew"), Some(ActivityLogSortBy::Overview));
        assert_eq!(parse_sort_by("7"), Some(ActivityLogSortBy::LogSeverity));
        assert_eq!(
            parse_sort_direction("descending"),
            Some(SortDirection::Descending)
        );
        assert_eq!(parse_severity("Warning"), Some(LogSeverity::Warning));
        assert_eq!(parse_severity("6"), Some(LogSeverity::None));
    }
}
