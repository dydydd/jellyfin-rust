//! Emby API surface for Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme and use Emby's `/emby` API base path.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, Uri, uri::PathAndQuery},
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;
use tower::{ServiceExt, service_fn};

mod auth_user;
mod backup;
mod bif;
mod encoding;
mod environment;
mod hide_from_resume;
mod home_sections;
mod library;
mod notifications;
mod packages;
mod plugins;
mod section_items;
mod system_misc;
mod track_selections;
mod typed_settings;
mod users;

/// Emby's Android and iOS API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Version advertised by the checked-in Emby client contract.
const EMBY_API_VERSION: &str = "4.10.0.40";

/// Builds the independent Emby route tree.
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let fallback = jellyfin_api::unprefixed_router(state.as_ref().clone());
    let routes = dedicated_routes()
        .fallback_service(fallback)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            jellyfin_api::protocol_route_auth,
        ))
        .with_state(state);

    Router::new().nest(EMBY_API_PREFIX, case_insensitive_dedicated_routes(routes))
}

// Axum matches paths case-sensitively, while the ASP.NET router used by Emby
// compares literal path segments without regard to ASCII case. Match against
// the dedicated Emby templates before dispatch so a mixed-case static path
// cannot be captured by a shared dynamic route (for example, `Updates` as a
// package name). Dynamic segments are copied from the request unchanged.
//
// This list contains only paths whose casing must be normalized inside the
// Emby tree. Most are protocol-owned; a small number deliberately reuse a
// shared handler after normalization. The shared Jellyfin fallback and the
// unprefixed Jellyfin tree remain untouched.
const DEDICATED_ROUTE_TEMPLATES: &[&str] = &[
    "/AudioBooks/NextUp",
    "/AudioCodecs",
    "/AudioLayouts",
    "/Audio/{item_id}/live.m3u8",
    "/Artists/Prefixes",
    "/BackupRestore/BackupInfo",
    "/Branding/Configuration",
    "/Containers",
    "/DisplayPreferences/{display_preferences_id}",
    "/Encoding/CodecConfiguration/Defaults",
    "/Encoding/CodecInformation/Video",
    "/Encoding/ToneMapOptions",
    "/Environment/DefaultDirectoryBrowser",
    "/Environment/DirectoryContents",
    "/Environment/Drives",
    "/Environment/NetworkDevices",
    "/Environment/NetworkShares",
    "/Environment/ParentPath",
    "/Environment/ValidatePath",
    "/ExtendedVideoTypes",
    "/Features",
    "/Items/Access",
    "/Items/Intros",
    "/Items/Prefixes",
    "/ItemTypes",
    "/Notifications/Types",
    // Keep the literal route before the dynamic package-name route. This is
    // the same precedence ASP.NET gives literal segments.
    "/Packages/Updates",
    "/Packages",
    "/Packages/{name}",
    "/OfficialRatings",
    "/Shows/Missing",
    "/StreamLanguages",
    "/SubtitleCodecs",
    "/System/Info/Public",
    "/System/Info",
    "/System/Logs/{name}/Lines",
    "/System/Ping",
    "/System/ReleaseNotes/Versions",
    "/System/ReleaseNotes",
    "/System/WakeOnLanInfo",
    "/Tags",
    "/UserSettings/{user_id}/Partial",
    "/UserSettings/{user_id}",
    "/Users/{user_id}/Items/{item_id}/HideFromResume",
    "/Users/{user_id}/HomeSections/Delete",
    "/Users/{user_id}/HomeSections/Move",
    "/Users/{user_id}/HomeSections",
    "/Users/{user_id}/Sections/{section_id}/Items",
    "/Users/{user_id}/TrackSelections/{track_type}/Delete",
    "/Users/{user_id}/TrackSelections/{track_type}",
    "/Users/{user_id}/TypedSettings/{key}",
    "/Users/ItemAccess",
    "/Users/CopyDataOptions",
    "/Users/Prefixes",
    // The shared handler is intentionally reused, but mixed-case Emby login
    // bootstrap requests must be normalized before the shared fallback and
    // route-policy matcher run.
    "/Users/Public",
    "/Users/Query",
    "/Videos/{item_id}/index.bif",
    "/Videos/{item_id}/Subtitles/{index}",
    "/Videos/{item_id}/Subtitles/{index}/Delete",
    "/VideoCodecs",
    "/Videos/{item_id}/live_subtitles.m3u8",
    "/Videos/{item_id}/subtitles.m3u8",
];

fn case_insensitive_dedicated_routes(routes: Router) -> Router {
    Router::new().fallback_service(service_fn(move |mut request: Request| {
        let routes = routes.clone();
        async move {
            normalize_dedicated_route_uri(request.uri_mut());
            routes.oneshot(request).await
        }
    }))
}

fn normalize_dedicated_route_uri(uri: &mut Uri) {
    let Some(path) = normalized_dedicated_path(uri.path()) else {
        return;
    };
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let Ok(path_and_query) = PathAndQuery::try_from(path_and_query) else {
        return;
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    if let Ok(normalized) = Uri::from_parts(parts) {
        *uri = normalized;
    }
}

fn normalized_dedicated_path(path: &str) -> Option<String> {
    let request_segments = path.strip_prefix('/')?.split('/').collect::<Vec<_>>();
    for template in DEDICATED_ROUTE_TEMPLATES {
        let template_segments = template
            .strip_prefix('/')
            .expect("dedicated route templates are absolute")
            .split('/')
            .collect::<Vec<_>>();
        if request_segments.len() != template_segments.len() {
            continue;
        }
        let mut normalized = String::with_capacity(path.len());
        let mut matches = true;
        for (request_segment, template_segment) in
            request_segments.iter().zip(template_segments.iter())
        {
            normalized.push('/');
            if template_segment.starts_with('{') && template_segment.ends_with('}') {
                normalized.push_str(request_segment);
            } else if request_segment.eq_ignore_ascii_case(template_segment) {
                normalized.push_str(template_segment);
            } else {
                matches = false;
                break;
            }
        }
        if matches {
            return Some(normalized);
        }
    }
    None
}

fn dedicated_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(jellyfin_api::emby_legacy_audio_hls_routes())
        .merge(jellyfin_api::emby_legacy_subtitle_delete_routes())
        .merge(jellyfin_api::emby_legacy_subtitle_hls_routes())
        .merge(auth_user::routes())
        .merge(backup::routes())
        .merge(bif::routes())
        .merge(encoding::routes())
        .merge(environment::routes())
        .merge(hide_from_resume::routes())
        .merge(home_sections::routes())
        .merge(library::routes())
        .merge(notifications::routes())
        .merge(packages::routes())
        .merge(plugins::routes())
        .merge(section_items::routes())
        .merge(system_misc::routes())
        .merge(track_selections::routes())
        .merge(typed_settings::routes())
        .merge(users::routes())
        .route("/Branding/Configuration", get(branding_configuration))
        .route("/branding/configuration", get(branding_configuration))
        .route("/System/Info/Public", get(public_system_info))
        .route("/system/info/public", get(public_system_info))
        .route("/System/Info", get(system_info))
        .route("/system/info", get(system_info))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    local_address: Option<String>,
    local_addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wan_address: Option<String>,
    remote_addresses: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct BrandingOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    login_disclaimer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_css: Option<String>,
}

impl From<jellyfin_model::PublicSystemInfo> for PublicSystemInfo {
    fn from(info: jellyfin_model::PublicSystemInfo) -> Self {
        Self {
            local_addresses: info.local_address.iter().cloned().collect(),
            local_address: info.local_address,
            wan_address: None,
            remote_addresses: Vec::new(),
            server_name: info.server_name,
            version: Some(EMBY_API_VERSION.to_owned()),
            id: info.id,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SystemInfo {
    #[serde(flatten)]
    public_info: PublicSystemInfo,
    operating_system_display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_name: Option<String>,
    has_pending_restart: bool,
    is_shutting_down: bool,
    operating_system: String,
    supports_library_monitor: bool,
    web_socket_port_number: i32,
    completed_installations: Vec<()>,
    can_self_restart: bool,
    can_launch_web_browser: bool,
    program_data_path: String,
    items_by_name_path: String,
    cache_path: String,
    log_path: String,
    internal_metadata_path: String,
    transcoding_temp_path: String,
    has_update_available: bool,
}

async fn public_system_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PublicSystemInfo>, StatusCode> {
    Ok(Json(state.public_system_info().await?.into()))
}

async fn branding_configuration(
    State(state): State<Arc<AppState>>,
) -> Result<Json<BrandingOptions>, StatusCode> {
    let options = state.branding_options().await?;
    Ok(Json(BrandingOptions {
        login_disclaimer: options.login_disclaimer,
        custom_css: options.custom_css,
    }))
}

async fn system_info(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<SystemInfo>, Response> {
    let info = state.system_info(&headers, &uri).await?;
    let public_info = PublicSystemInfo::from(info.public_info.clone());
    Ok(Json(SystemInfo {
        public_info,
        operating_system_display_name: info.operating_system_display_name,
        package_name: info.package_name,
        has_pending_restart: info.has_pending_restart,
        is_shutting_down: info.is_shutting_down,
        operating_system: info.public_info.operating_system,
        supports_library_monitor: info.supports_library_monitor,
        web_socket_port_number: info.web_socket_port_number,
        completed_installations: Vec::new(),
        can_self_restart: info.can_self_restart,
        can_launch_web_browser: info.can_launch_web_browser,
        program_data_path: info.program_data_path,
        items_by_name_path: info.items_by_name_path,
        cache_path: info.cache_path,
        log_path: info.log_path,
        internal_metadata_path: info.internal_metadata_path,
        transcoding_temp_path: info.transcoding_temp_path,
        has_update_available: info.has_update_available,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use axum::{
        body::{Body, to_bytes},
        extract::{MatchedPath, Path, Request},
        http::{HeaderName, HeaderValue, Method, StatusCode},
        middleware::{self, Next},
        response::IntoResponse,
    };
    use sea_orm::DatabaseConnection;
    use serde::Deserialize;
    use tower::ServiceExt;

    const MATCHED_PATH_HEADER: HeaderName = HeaderName::from_static("x-test-matched-path");

    #[derive(Deserialize)]
    struct ClientContract {
        version: String,
        operations: Vec<ClientOperation>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ClientOperation {
        method: String,
        path: String,
        tag: String,
    }

    #[tokio::test]
    async fn jellyfin_and_emby_are_separate_route_trees() {
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let jellyfin = jellyfin_api::router(state.clone());
        let emby = router(state);

        assert_eq!(status(&jellyfin, "/GetUtcTime").await, StatusCode::OK);
        assert_eq!(status(&jellyfin, "/api/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&jellyfin, "/emby/GetUtcTime").await, StatusCode::OK);

        assert_eq!(status(&emby, "/emby/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/api/GetUtcTime").await, StatusCode::OK);

        let jellyfin_info = body(&jellyfin, "/System/Info/Public").await;
        let emby_info = body(&emby, "/emby/System/Info/Public").await;
        assert!(jellyfin_info.get("ProductName").is_some());
        assert!(emby_info.get("ProductName").is_none());
        assert_eq!(emby_info["Version"], EMBY_API_VERSION);
        assert_eq!(emby_info["LocalAddresses"][0], "http://127.0.0.1:8096");
        assert!(emby_info["RemoteAddresses"].is_array());

        let jellyfin_info = body(&jellyfin, "/System/Info").await;
        let emby_info = body(&emby, "/emby/System/Info").await;
        assert!(jellyfin_info.get("WebPath").is_some());
        assert!(emby_info.get("WebPath").is_none());
        assert!(emby_info.get("LocalAddresses").is_some());
        assert!(emby_info.get("CompletedInstallations").is_some());

        let jellyfin_branding = body(&jellyfin, "/Branding/Configuration").await;
        let emby_branding = body(&emby, "/emby/Branding/Configuration").await;
        assert!(jellyfin_branding.get("SplashscreenEnabled").is_some());
        assert!(emby_branding.get("SplashscreenEnabled").is_none());
        assert_eq!(
            status(&emby, "/emby/branding/css.css").await,
            StatusCode::OK
        );
        assert_eq!(
            status(&emby, "/emby/localization/cultures").await,
            StatusCode::OK
        );
        assert_eq!(
            status(&emby, "/emby/startup/configuration").await,
            StatusCode::OK
        );

        // Axum paths are case-sensitive; Emby clients rely on ASP.NET's
        // case-insensitive routing for these streaming control endpoints.
        assert_ne!(
            status(&emby, "/emby/playback/bitratetest").await,
            StatusCode::NOT_FOUND
        );
        assert_ne!(
            status(&emby, "/emby/livestreams/open").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn emby_legacy_delete_aliases_reach_existing_handlers() {
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let emby = router(state);
        let id = "00000000-0000-0000-0000-000000000000";
        for uri in [
            format!("/emby/Items/{id}/Images/Primary/Delete"),
            format!("/emby/items/{id}/images/primary/delete"),
            format!("/emby/Items/{id}/Images/Primary/0/Delete"),
            format!("/emby/items/{id}/images/primary/0/delete"),
            format!("/emby/Items/{id}/Delete"),
            format!("/emby/items/{id}/delete"),
            format!("/emby/Users/{id}/Images/Profile/Delete"),
            format!("/emby/users/{id}/images/profile/delete"),
            format!("/emby/Users/{id}/Images/Profile/0/Delete"),
            format!("/emby/users/{id}/images/profile/0/delete"),
            format!("/emby/Collections/{id}/Items/Delete"),
            format!("/emby/collections/{id}/items/delete"),
            format!("/emby/Playlists/{id}/Items/Delete"),
            format!("/emby/playlists/{id}/items/delete"),
        ] {
            assert_ne!(
                status_method(&emby, axum::http::Method::POST, &uri).await,
                StatusCode::NOT_FOUND,
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn mixed_case_dedicated_static_routes_keep_literal_precedence() {
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let dedicated = dedicated_routes()
            .layer(middleware::from_fn(short_circuit_matched_route))
            .with_state(Arc::new(state));
        let app = case_insensitive_dedicated_routes(dedicated);

        for (path, expected) in [
            ("/pAcKaGeS/uPdAtEs", "/Packages/Updates"),
            ("/uSeRs/qUeRy", "/Users/Query"),
            ("/UsErS/iTeMaCcEsS", "/Users/ItemAccess"),
            ("/uSeRs/cOpYdAtAoPtIoNs", "/Users/CopyDataOptions"),
            ("/uSeRs/pReFiXeS", "/Users/Prefixes"),
            ("/iTeMs/iNtRoS", "/Items/Intros"),
            ("/iTeMs/pReFiXeS", "/Items/Prefixes"),
            ("/aRtIsTs/pReFiXeS", "/Artists/Prefixes"),
            ("/nOtIfIcAtIoNs/tYpEs", "/Notifications/Types"),
            (
                "/aUdIo/00000000-0000-0000-0000-000000000000/lIvE.m3U8",
                "/Audio/{item_id}/live.m3u8",
            ),
            (
                "/vIdEoS/00000000-0000-0000-0000-000000000000/sUbTiTlEs.M3u8",
                "/Videos/{item_id}/subtitles.m3u8",
            ),
            (
                "/vIdEoS/00000000-0000-0000-0000-000000000000/lIvE_sUbTiTlEs.M3u8",
                "/Videos/{item_id}/live_subtitles.m3u8",
            ),
            (
                "/sYsTeM/rElEaSeNoTeS/vErSiOnS",
                "/System/ReleaseNotes/Versions",
            ),
            (
                "/vIdEoS/00000000-0000-0000-0000-000000000000/InDeX.BiF",
                "/Videos/{item_id}/index.bif",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
            assert_eq!(
                response.headers().get(&MATCHED_PATH_HEADER).unwrap(),
                expected,
                "{path} must reach the literal dedicated route"
            );
        }

        let path = "/iTeMs/aCcEsS";
        let response = case_insensitive_dedicated_routes(dedicated_routes().with_state(Arc::new(
            AppState::new(
                DatabaseConnection::Disconnected,
                "API Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            ),
        )))
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");

        for method in [Method::GET, Method::POST] {
            let path = "/dIsPlAyPrEfErEnCeS/MiXeD%2BId?Client=Emby";
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{method} {path}");
            assert_eq!(
                response.headers().get(&MATCHED_PATH_HEADER).unwrap(),
                "/DisplayPreferences/{display_preferences_id}",
                "{method} {path} must use the same dedicated route"
            );
        }
    }

    #[tokio::test]
    async fn dedicated_route_normalization_preserves_dynamic_values_query_and_original_uri() {
        async fn observe_request(
            OriginalUri(original): OriginalUri,
            uri: Uri,
            Path(name): Path<String>,
        ) -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "OriginalUri": original.to_string(),
                "RoutedUri": uri.to_string(),
                "Name": name,
            }))
        }

        let app = Router::new().nest(
            EMBY_API_PREFIX,
            case_insensitive_dedicated_routes(
                Router::new().route("/Packages/{name}", get(observe_request)),
            ),
        );
        let requested = "/emby/pAcKaGeS/MiXeD%20Name?Client=Emby%20Swift&Token=AaBb";
        let response = app
            .clone()
            .oneshot(Request::get(requested).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(value["OriginalUri"], requested);
        assert_eq!(
            value["RoutedUri"],
            "/Packages/MiXeD%20Name?Client=Emby%20Swift&Token=AaBb"
        );
        assert_eq!(value["Name"], "MiXeD Name");

        assert_eq!(
            status(&app, "/pAcKaGeS/MiXeD%20Name").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(normalized_dedicated_path("/iTeMs/NotAPrefix"), None);
    }

    #[tokio::test]
    async fn generated_dedicated_operations_have_mixed_case_dispatch() {
        let contract: ClientContract =
            serde_json::from_str(include_str!("../tests/fixtures/emby_operations.json"))
                .expect("checked-in Emby operation inventory must be valid");
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let dedicated = dedicated_routes()
            .with_state(Arc::new(state))
            .layer(middleware::from_fn(short_circuit_matched_route));
        let mixed_case = case_insensitive_dedicated_routes(dedicated.clone());

        let mut checked = 0;
        for operation in contract.operations {
            if matches!(operation.tag.as_str(), "LiveTvService" | "PluginService")
                || operation.path == "/LiveTv"
                || operation.path.starts_with("/LiveTv/")
            {
                continue;
            }
            let method = Method::from_bytes(operation.method.as_bytes()).unwrap();
            let canonical = materialize_path(&operation.path);
            let Some(expected_route) = matched_route(&dedicated, method.clone(), &canonical).await
            else {
                continue;
            };

            checked += 1;
            let path = alternating_ascii_case(&canonical);
            assert_eq!(
                matched_route(&mixed_case, method, &path).await.as_deref(),
                Some(expected_route.as_str()),
                "dedicated Emby operation lost mixed-case dispatch: {} {} ({path})",
                operation.method,
                operation.path
            );
        }
        assert!(
            checked > 20,
            "expected to audit the dedicated route surface"
        );
    }

    #[tokio::test]
    async fn generated_emby_client_route_inventory_audit() {
        let contract: ClientContract =
            serde_json::from_str(include_str!("../tests/fixtures/emby_operations.json"))
                .expect("checked-in Emby operation inventory must be valid");
        assert_eq!(contract.version, EMBY_API_VERSION);

        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let dedicated = dedicated_routes()
            .with_state(Arc::new(state.clone()))
            .layer(middleware::from_fn(short_circuit_matched_route));
        let shared = jellyfin_api::unprefixed_router(state)
            .layer(middleware::from_fn(short_circuit_matched_route));
        let mut missing = BTreeSet::new();
        let mut checked = 0;
        for operation in contract.operations {
            if matches!(operation.tag.as_str(), "LiveTvService" | "PluginService")
                || operation.path == "/LiveTv"
                || operation.path.starts_with("/LiveTv/")
            {
                continue;
            }
            checked += 1;
            let method = Method::from_bytes(operation.method.as_bytes()).unwrap();
            let path = materialize_path(&operation.path);
            if !route_matches(&dedicated, method.clone(), &path).await
                && !route_matches(&shared, method, &path).await
            {
                missing.insert(format!("{} {}", operation.method, operation.path));
            }
        }
        let gap_ledger = include_str!("../tests/fixtures/emby_missing_routes.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();

        let unexpectedly_missing = missing.difference(&gap_ledger).collect::<Vec<_>>();
        let now_supported = gap_ledger.difference(&missing).collect::<Vec<_>>();
        assert!(
            unexpectedly_missing.is_empty() && now_supported.is_empty(),
            "Emby route gap ledger is stale (checked {checked} in-scope operations): \
             unexpectedly missing={unexpectedly_missing:?}; now supported={now_supported:?}"
        );
    }

    async fn short_circuit_matched_route(request: Request, next: Next) -> Response {
        let Some(path) = request.extensions().get::<MatchedPath>() else {
            return next.run(request).await;
        };
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            MATCHED_PATH_HEADER,
            HeaderValue::from_str(path.as_str()).expect("matched paths are valid header values"),
        );
        response
    }

    async fn route_matches(app: &Router, method: Method, path: &str) -> bool {
        matched_route(app, method, path).await.is_some()
    }

    async fn matched_route(app: &Router, method: Method, path: &str) -> Option<String> {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap()
            .headers()
            .get(&MATCHED_PATH_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    }

    fn materialize_path(template: &str) -> String {
        let mut path = String::with_capacity(template.len());
        let mut placeholder = false;
        for character in template.chars() {
            match character {
                '{' => {
                    placeholder = true;
                    path.push('0');
                }
                '}' => placeholder = false,
                _ if !placeholder => path.push(character),
                _ => {}
            }
        }
        path
    }

    fn alternating_ascii_case(path: &str) -> String {
        path.char_indices()
            .map(|(index, character)| {
                if index % 2 == 0 {
                    character.to_ascii_lowercase()
                } else {
                    character.to_ascii_uppercase()
                }
            })
            .collect()
    }

    async fn status(app: &Router, uri: &str) -> StatusCode {
        status_method(app, axum::http::Method::GET, uri).await
    }

    async fn status_method(app: &Router, method: axum::http::Method, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    async fn body(app: &Router, uri: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
    }
}
