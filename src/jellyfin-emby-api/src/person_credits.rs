//! Emby's legacy Person filmography response.
//!
//! The generated Emby clients define only a required Person `Id` and return
//! an array of `CreditsList`; there is no query or pagination contract on
//! this operation. The shared service resolves the current authentication
//! context and returns only policy-visible PostgreSQL items. This module owns
//! the Emby-specific grouping and `RemoteSearchResult` wire shape.

use std::{array, collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Path, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use jellyfin_api::{AppState, EmbyPersonCreditRecord};
use serde::Serialize;
use uuid::Uuid;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/Persons/{person_id}/Credits", get(get_credits))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum PersonType {
    Actor,
    Director,
    Writer,
    Producer,
    GuestStar,
    Composer,
    Conductor,
    Lyricist,
}

impl PersonType {
    const ALL: [Self; 8] = [
        Self::Actor,
        Self::Director,
        Self::Writer,
        Self::Producer,
        Self::GuestStar,
        Self::Composer,
        Self::Conductor,
        Self::Lyricist,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|person_type| person_type.name().eq_ignore_ascii_case(value))
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Actor => "Actor",
            Self::Director => "Director",
            Self::Writer => "Writer",
            Self::Producer => "Producer",
            Self::GuestStar => "GuestStar",
            Self::Composer => "Composer",
            Self::Conductor => "Conductor",
            Self::Lyricist => "Lyricist",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Actor => 0,
            Self::Director => 1,
            Self::Writer => 2,
            Self::Producer => 3,
            Self::GuestStar => 4,
            Self::Composer => 5,
            Self::Conductor => 6,
            Self::Lyricist => 7,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CreditsList {
    person_type: PersonType,
    items: Vec<RemoteSearchResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteSearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_title: Option<String>,
    provider_ids: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_number_end: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    premiere_date: Option<String>,
    person_type: PersonType,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(rename = "Type")]
    item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    overview: Option<String>,
}

async fn get_credits(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(person_id): Path<Uuid>,
) -> Result<Json<Vec<CreditsList>>, Response> {
    let credits = state
        .emby_person_credits_for_request(&headers, &uri, person_id)
        .await?;
    Ok(Json(group_credits(credits)))
}

fn group_credits(records: Vec<EmbyPersonCreditRecord>) -> Vec<CreditsList> {
    let mut groups: [Vec<RemoteSearchResult>; 8] = array::from_fn(|_| Vec::new());
    for record in records {
        let Some(person_type) = PersonType::parse(&record.person_type) else {
            // Emby's generated enum predates Jellyfin's Narrator and other
            // expanded credit kinds. Omitting unsupported groups keeps Swift
            // Codable from rejecting the complete response.
            continue;
        };
        groups[person_type.index()].push(RemoteSearchResult {
            name: record.name,
            original_title: record.original_title,
            provider_ids: record.provider_ids,
            production_year: record.production_year,
            index_number: record.index_number,
            index_number_end: record.index_number_end,
            parent_index_number: record.parent_index_number,
            premiere_date: record.premiere_date,
            person_type,
            role: record.role,
            item_type: record.item_type,
            overview: record.overview,
        });
    }

    PersonType::ALL
        .into_iter()
        .zip(groups)
        .filter_map(|(person_type, items)| {
            (!items.is_empty()).then_some(CreditsList { person_type, items })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_supported_credits_in_generated_enum_order() {
        let result = group_credits(vec![
            record("Writer", "Writer role"),
            record("narrator", "Narrator role"),
            record("actor", "Lead"),
        ]);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].person_type, PersonType::Actor);
        assert_eq!(result[0].items[0].role.as_deref(), Some("Lead"));
        assert_eq!(result[1].person_type, PersonType::Writer);
    }

    fn record(person_type: &str, role: &str) -> EmbyPersonCreditRecord {
        EmbyPersonCreditRecord {
            name: Some("Item".to_owned()),
            original_title: None,
            provider_ids: HashMap::new(),
            production_year: None,
            index_number: None,
            index_number_end: None,
            parent_index_number: None,
            premiere_date: None,
            person_type: person_type.to_owned(),
            role: Some(role.to_owned()),
            item_type: "Movie".to_owned(),
            overview: None,
        }
    }
}
