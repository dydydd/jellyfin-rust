use std::{fmt, sync::Arc};

use axum::{
    Json, Router,
    extract::{OriginalUri, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use jellyfin_model::FileSystemEntryInfo;
use serde::{Deserialize, Deserializer, Serialize, de};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Environment/DefaultDirectoryBrowser", get(default_browser))
        .route("/environment/defaultdirectorybrowser", get(default_browser))
        .route(
            "/Environment/DirectoryContents",
            get(directory_contents).post(directory_contents_post),
        )
        .route(
            "/environment/directorycontents",
            get(directory_contents).post(directory_contents_post),
        )
        .route("/Environment/Drives", get(drives))
        .route("/environment/drives", get(drives))
        .route("/Environment/NetworkDevices", get(empty_entries))
        .route("/environment/networkdevices", get(empty_entries))
        .route("/Environment/NetworkShares", get(network_shares))
        .route("/environment/networkshares", get(network_shares))
        .route("/Environment/ParentPath", get(parent_path))
        .route("/environment/parentpath", get(parent_path))
        .route("/Environment/ValidatePath", post(validate_path))
        .route("/environment/validatepath", post(validate_path))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PathQuery {
    path: Option<String>,
    include_files: Option<bool>,
    include_directories: Option<bool>,
}

impl<'de> Deserialize<'de> for PathQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = PathQuery;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby environment path query")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut query = PathQuery::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("Path") {
                        query.path = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("IncludeFiles") {
                        query.include_files = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("IncludeDirectories") {
                        query.include_directories = Some(map.next_value()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(query)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DirectoryContentsBody {
    #[allow(dead_code)]
    #[serde(alias = "username")]
    username: Option<String>,
    #[allow(dead_code)]
    #[serde(alias = "password")]
    password: Option<String>,
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> Result<(), Response> {
    state.require_emby_administrator(headers, uri).await
}

async fn default_browser(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<DefaultDirectoryBrowserInfo>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(DefaultDirectoryBrowserInfo { path: None }))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DefaultDirectoryBrowserInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

async fn directory_contents(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(Json(state.environment_directory_contents(
        &path,
        query.include_files.unwrap_or(false),
        query.include_directories.unwrap_or(false),
    )?))
}

async fn directory_contents_post(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
    Json(_body): Json<DirectoryContentsBody>,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(Json(state.environment_directory_contents(
        &path,
        query.include_files.unwrap_or(false),
        query.include_directories.unwrap_or(false),
    )?))
}

async fn drives(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(state.environment_drives()))
}

async fn empty_entries(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    Ok(Json(Vec::new()))
}

async fn network_shares(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<Vec<FileSystemEntryInfo>>, Response> {
    authorize(&state, &headers, &uri).await?;
    let _path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(Json(Vec::new()))
}

async fn parent_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Response, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    Ok(state.environment_parent_path(&path).map_or_else(
        || StatusCode::NO_CONTENT.into_response(),
        |parent| Json(parent).into_response(),
    ))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ValidatePath {
    is_file: Option<bool>,
    validate_writable: Option<bool>,
}

impl<'de> Deserialize<'de> for ValidatePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = ValidatePath;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an Emby ValidatePath object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut body = ValidatePath::default();
                while let Some(name) = map.next_key::<String>()? {
                    if name.eq_ignore_ascii_case("IsFile") {
                        body.is_file = Some(map.next_value()?);
                    } else if name.eq_ignore_ascii_case("ValidateWriteable")
                        || name.eq_ignore_ascii_case("ValidateWritable")
                    {
                        // Emby spells the property `Writeable`; accepting the
                        // corrected Jellyfin spelling keeps both generated
                        // mobile contracts usable on the protocol adapter.
                        body.validate_writable = Some(map.next_value()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(body)
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

async fn validate_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
    Json(body): Json<ValidatePath>,
) -> Result<StatusCode, Response> {
    authorize(&state, &headers, &uri).await?;
    let path = query.path.ok_or(StatusCode::BAD_REQUEST.into_response())?;
    state.environment_validate_path(
        Some(&path),
        body.is_file,
        body.validate_writable.unwrap_or(false),
    )?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_query_is_case_insensitive_and_last_duplicate_wins() {
        let query: PathQuery = serde_json::from_str(
            r#"{"Path":"first","pAtH":"second","IncludeFiles":false,"iNcLuDeFiLeS":true,"includeDirectories":true,"INCLUDEDIRECTORIES":false,"Ignored":"value"}"#,
        )
        .expect("environment path query");
        assert_eq!(
            query,
            PathQuery {
                path: Some("second".to_owned()),
                include_files: Some(true),
                include_directories: Some(false),
            }
        );
    }

    #[test]
    fn validate_path_body_supports_both_mobile_spellings_and_last_wins() {
        let body: ValidatePath = serde_json::from_str(
            r#"{"IsFile":true,"iSfIlE":false,"ValidateWriteable":false,"vAlIdAtEwRiTaBlE":true,"Ignored":"value"}"#,
        )
        .expect("Emby ValidatePath body");
        assert_eq!(
            body,
            ValidatePath {
                is_file: Some(false),
                validate_writable: Some(true),
            }
        );

        let jellyfin_spelling: ValidatePath = serde_json::from_str(r#"{"vAlIdAtEwRiTaBlE":true}"#)
            .expect("Jellyfin ValidateWritable spelling");
        assert_eq!(jellyfin_spelling.validate_writable, Some(true));
    }
}
