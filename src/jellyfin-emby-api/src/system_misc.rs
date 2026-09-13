//! Small Emby compatibility endpoints whose Jellyfin equivalents are not
//! exposed as reusable public handlers.

use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{ConnectInfo, OriginalUri, Request, State},
    http::{StatusCode, Uri, uri::PathAndQuery},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;
use tower::ServiceExt;

/// Emby system/discovery routes.  The parent router supplies the state.
pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/System/Ping", get(ping).post(ping).head(ping))
        .route("/system/ping", get(ping).post(ping).head(ping))
        .route("/System/WakeOnLanInfo", get(wake_on_lan_info))
        .route("/system/wakeonlaninfo", get(wake_on_lan_info))
        .route("/System/ReleaseNotes", get(release_notes))
        .route("/system/releasenotes", get(release_notes))
        .route("/System/ReleaseNotes/Versions", get(release_note_versions))
        .route("/system/releasenotes/versions", get(release_note_versions))
        .route(
            "/System/Logs/{name}/Lines",
            get(jellyfin_api::emby_log_file_lines),
        )
        .route(
            "/system/logs/{name}/lines",
            get(jellyfin_api::emby_log_file_lines),
        )
        .route("/Shows/Missing", get(shows_missing))
        .route("/shows/missing", get(shows_missing))
        .route("/StreamLanguages", get(stream_languages))
        .route("/streamlanguages", get(stream_languages))
        .route("/SubtitleCodecs", get(subtitle_codecs))
        .route("/subtitlecodecs", get(subtitle_codecs))
        .route("/VideoCodecs", get(video_codecs))
        .route("/videocodecs", get(video_codecs))
        .route("/Tags", get(tags))
        .route("/tags", get(tags))
}

async fn ping() -> StatusCode {
    // Emby's generated clients model Ping as `void`, with a successful 200.
    StatusCode::OK
}

// The server does not expose Wake-on-LAN configuration.  An empty list is the
// valid Emby response when no Wake-on-LAN devices are configured.
async fn wake_on_lan_info() -> Json<Vec<()>> {
    Json(Vec::new())
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PackageVersionInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_str: Option<String>,
}

async fn release_notes() -> Json<PackageVersionInfo> {
    Json(PackageVersionInfo {
        name: None,
        version_str: None,
    })
}

async fn release_note_versions() -> Json<Vec<PackageVersionInfo>> {
    Json(Vec::new())
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T> {
    items: Vec<T>,
    total_record_count: usize,
}

async fn shows_missing(
    State(state): State<Arc<AppState>>,
    OriginalUri(original_uri): OriginalUri,
    mut request: Request,
) -> Result<Response, Response> {
    let query = force_missing_episode_query(original_uri.query());
    rewrite_as_items(request.uri_mut(), &query)
        .map_err(|()| StatusCode::BAD_REQUEST.into_response())?;

    // The outer Emby route's private routing extensions must not reach the
    // shared Router. Preserve only transport context and the original Emby
    // URI, which selects the protocol-local response adapter in `/Items`.
    let connect_info = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied();
    let (mut parts, body) = request.into_parts();
    parts.extensions.clear();
    parts.extensions.insert(OriginalUri(original_uri));
    if let Some(connect_info) = connect_info {
        parts.extensions.insert(connect_info);
    }
    let request = Request::from_parts(parts, body);

    Ok(jellyfin_api::unprefixed_router(state.as_ref().clone())
        .oneshot(request)
        .await
        .unwrap_or_else(|error: Infallible| match error {}))
}

fn force_missing_episode_query(query: Option<&str>) -> String {
    let mut retained = query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| {
            let key = pair.split_once('=').map_or(*pair, |(key, _)| key);
            let key = percent_decode_query_key(key);
            !key.eq_ignore_ascii_case("IncludeItemTypes") && !key.eq_ignore_ascii_case("IsMissing")
        })
        .filter(|pair| !pair.is_empty())
        .collect::<Vec<_>>();
    retained.extend(["IncludeItemTypes=Episode", "IsMissing=true"]);
    retained.join("&")
}

fn rewrite_as_items(uri: &mut Uri, query: &str) -> Result<(), ()> {
    let path_and_query = PathAndQuery::try_from(format!("/Items?{query}")).map_err(|_| ())?;
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    *uri = Uri::from_parts(parts).map_err(|_| ())?;
    Ok(())
}

fn percent_decode_query_key(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let Some(high) = hex_value(bytes[index + 1]) else {
                    decoded.push(bytes[index]);
                    index += 1;
                    continue;
                };
                let Some(low) = hex_value(bytes[index + 2]) else {
                    decoded.push(bytes[index]);
                    index += 1;
                    continue;
                };
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

async fn stream_languages() -> Json<QueryResult<String>> {
    strings()
}
async fn subtitle_codecs() -> Json<QueryResult<String>> {
    strings()
}
async fn video_codecs() -> Json<QueryResult<String>> {
    strings()
}
async fn tags() -> Json<QueryResult<String>> {
    strings()
}

fn strings() -> Json<QueryResult<String>> {
    Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use sea_orm::DatabaseConnection;
    use tower::ServiceExt;

    #[tokio::test]
    async fn mobile_discovery_routes_are_reachable() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in [
            "/System/ReleaseNotes",
            "/System/ReleaseNotes/Versions",
            "/System/WakeOnLanInfo",
            "/Shows/Missing",
            "/StreamLanguages",
            "/SubtitleCodecs",
            "/VideoCodecs",
            "/Tags",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            if path == "/Shows/Missing" {
                // The real handler delegates to the PostgreSQL-backed Items
                // query; the integration test covers its success path.
                assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
            } else {
                assert!(response.status().is_success(), "{path}");
            }
            let _ = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        }
    }

    #[tokio::test]
    async fn ping_is_an_empty_success_response() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        let response = app
            .oneshot(Request::get("/System/Ping").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_episode_query_preserves_other_pairs_and_cannot_be_overridden() {
        let query = force_missing_episode_query(Some(
            "Fields=Overview&includeitemtypes=Movie&ISmissing=false&%49ncludeItemTypes=Series&Limit=-1&Fields=ProviderIds",
        ));
        assert_eq!(
            query,
            "Fields=Overview&Limit=-1&Fields=ProviderIds&IncludeItemTypes=Episode&IsMissing=true"
        );
    }

    #[tokio::test]
    async fn log_lines_aliases_are_protected() {
        let app = routes().with_state(Arc::new(AppState::new(
            DatabaseConnection::Disconnected,
            "test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )));
        for path in [
            "/System/Logs/server.log/Lines",
            "/system/logs/server.log/lines",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }
}
