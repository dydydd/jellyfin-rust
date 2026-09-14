//! Emby API surface for Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme and use Emby's `/emby` API base path.

use std::sync::{Arc, OnceLock};

use axum::{
    Json, Router,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, Uri, uri::PathAndQuery},
    middleware::Next,
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;
use tower::{ServiceExt, service_fn};

mod alternate_sources;
mod audiobooks;
mod auth_user;
mod backup;
mod bif;
mod camera_uploads;
mod collection_provider;
mod connect;
mod dlna;
mod dlna_server;
mod encoding;
mod environment;
mod generic_ui;
mod hide_from_resume;
mod home_sections;
mod legacy_item_metadata;
mod library;
mod live_stream_media_info;
mod metadata_reset;
mod notifications;
mod package_updates;
mod packages;
mod parties;
mod person_credits;
mod plugins;
mod query_contracts;
mod recent_searches;
mod remote_images;
mod section_items;
mod sync;
mod system_misc;
mod track_selections;
mod tv_shows;
mod typed_settings;
mod users;
mod web_strings;

/// Emby's Android and iOS API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Version advertised by the checked-in Emby client contract.
const EMBY_API_VERSION: &str = "4.10.0.40";

/// Builds the independent Emby route tree.
pub fn router(state: AppState) -> Router {
    let state = Arc::new(state);
    let fallback = jellyfin_api::unprefixed_router(state.as_ref().clone());
    let routes = dedicated_routes()
        .merge(swagger_alias_routes(fallback.clone()))
        .fallback_service(fallback)
        .layer(axum::middleware::from_fn(query_contracts::normalize))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            jellyfin_api::protocol_route_auth,
        ))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            users::adapt_user_responses,
        ))
        .layer(axum::middleware::from_fn(normalize_empty_success))
        .with_state(state);

    Router::new().nest(EMBY_API_PREFIX, case_insensitive_dedicated_routes(routes))
}

/// Emby's generated 4.10.0.40 document declares HTTP 200 as the sole success
/// status for every operation. Shared Jellyfin mutation handlers intentionally
/// retain their modern 204 contract; only the nested `/emby` tree normalizes
/// those empty successful responses.
async fn normalize_empty_success(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response.status() == StatusCode::NO_CONTENT {
        *response.status_mut() = StatusCode::OK;
    }
    response
}

// The shared Jellyfin document owns its OpenAPI bytes and response headers.
// Dispatch Emby's extensionless alias to that existing route without adding
// `/swagger` to Jellyfin's unprefixed or `/api` trees.
fn swagger_alias_routes(fallback: Router) -> Router<Arc<AppState>> {
    Router::new().route_service(
        "/swagger",
        service_fn(move |mut request: Request| {
            let fallback = fallback.clone();
            async move {
                *request.uri_mut() = Uri::from_static("/swagger.json");
                fallback.oneshot(request).await
            }
        }),
    )
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
    "/Collections/{collection_id}/Missing",
    "/Collections/{collection_id}/ProviderItems",
    "/Connect/Exchange",
    "/Connect/Pending",
    "/Devices/CameraUploads",
    "/Dlna/ProfileInfos",
    "/DisplayPreferences/{display_preferences_id}",
    "/Encoding/CodecConfiguration/Defaults",
    "/Encoding/CodecParameters",
    "/Encoding/CodecInformation/Video",
    "/Encoding/FfmpegOptions",
    "/Encoding/FullToneMapOptions",
    "/Encoding/PublicToneMapOptions",
    "/Encoding/SubtitleOptions",
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
    "/GameGenres",
    "/GameGenres/{name}",
    "/GameGenres/{name}/Images/{image_type}",
    "/GameGenres/{name}/Images/{image_type}/{image_index}",
    "/Games/{item_id}/Similar",
    "/Items/Access",
    "/Items/Shared/Leave",
    "/Items/Metadata/Reset",
    "/Items/RemoteSearch/Game",
    "/Items/{item_id}/Images",
    "/Items/{item_id}/RemoteImages/Download",
    "/Items/{item_id}/CriticReviews",
    "/Items/{item_id}/ThumbnailSet",
    "/Items/Intros",
    "/Items/Prefixes",
    "/ItemTypes",
    "/LiveStreams/Close",
    "/LiveStreams/MediaInfo",
    "/Notifications/Admin",
    "/Notifications/Services/Defaults",
    "/Notifications/Services/Test",
    "/Notifications/Types",
    // Keep the literal route before the dynamic package-name route. This is
    // the same precedence ASP.NET gives literal segments.
    "/Packages/Updates",
    "/Packages",
    "/Packages/{name}",
    "/Parties",
    "/Parties/Info",
    "/Parties/Leave",
    "/Parties/Messages",
    "/Parties/{party_id}/Join",
    "/OfficialRatings",
    "/Shows/Missing",
    "/Shows/NextUp",
    "/StreamLanguages",
    "/swagger",
    "/SubtitleCodecs",
    "/Sync/JobItems",
    "/Sync/JobItems/{id}/AdditionalFiles",
    "/Sync/JobItems/{id}/File",
    "/Sync/Items/Ready",
    "/Sync/Jobs",
    "/Sync/Jobs/{id}",
    "/Sync/Options",
    "/Sync/Targets",
    "/System/Info/Public",
    "/System/Info",
    "/System/Logs/{name}/Lines",
    "/System/Ping",
    "/System/ReleaseNotes/Versions",
    "/System/ReleaseNotes",
    "/System/WakeOnLanInfo",
    "/Tags",
    "/UI/Command",
    "/UI/View",
    "/UserSettings/{user_id}/Partial",
    "/UserSettings/{user_id}",
    "/Users/{user_id}/Items/{item_id}/HideFromResume",
    "/Users/{user_id}/HomeSections/Delete",
    "/Users/{user_id}/HomeSections/Move",
    "/Users/{user_id}/HomeSections",
    "/Users/{user_id}/Configuration",
    "/Users/{user_id}/Connect/Link/Delete",
    "/Users/{user_id}/Connect/Link",
    "/Users/{user_id}/Authenticate",
    "/Users/{user_id}/CopyData",
    "/Users/{user_id}/Policy",
    "/Users/{user_id}/RecentlySearched",
    "/Users/{user_id}/RecentlySearched/Delete",
    "/Users/{user_id}/SearchedItems",
    "/Users/{user_id}/SearchedItems/",
    "/Users/{user_id}/Sections/{section_id}/Items",
    "/Users/{user_id}/TrackSelections/{track_type}/Delete",
    "/Users/{user_id}/TrackSelections/{track_type}",
    "/Users/{user_id}/TypedSettings/{key}",
    "/Users/ItemAccess",
    "/Users/CopyDataOptions",
    "/Users/AuthenticateByName",
    "/Users/Me",
    "/Users/New",
    "/Users/Prefixes",
    // The shared handler is intentionally reused, but mixed-case Emby login
    // bootstrap requests must be normalized before the shared fallback and
    // route-policy matcher run.
    "/Users/Public",
    "/Users/Query",
    "/Users/{user_id}",
    "/Users",
    "/Videos/{item_id}/index.bif",
    "/Videos/{item_id}/Subtitles/{index}",
    "/Videos/{item_id}/Subtitles/{index}/Delete",
    "/Videos/{item_id}/AlternateSources/Delete",
    "/VideoCodecs",
    "/Videos/{item_id}/live_subtitles.m3u8",
    "/Videos/{item_id}/subtitles.m3u8",
    "/web/strings",
    "/web/stringset",
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
    for template_segments in emby_route_templates() {
        if request_segments.len() != template_segments.len() {
            continue;
        }
        let mut normalized = String::with_capacity(path.len());
        let mut matches = true;
        for (request_segment, template_segment) in request_segments.iter().zip(template_segments) {
            normalized.push('/');
            if let Some(segment) = normalized_template_segment(request_segment, template_segment) {
                normalized.push_str(&segment);
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

fn normalized_template_segment(request: &str, template: &str) -> Option<String> {
    if !template.contains('{') {
        return request
            .eq_ignore_ascii_case(template)
            .then(|| template.to_owned());
    }
    if template.starts_with('{') && template.ends_with('}') && template.matches('{').count() == 1 {
        return Some(request.to_owned());
    }

    let mut normalized = String::with_capacity(request.len());
    let mut request_offset = 0;
    let mut template_offset = 0;
    while let Some(open_relative) = template[template_offset..].find('{') {
        let open = template_offset + open_relative;
        let static_prefix = &template[template_offset..open];
        let request_prefix = request.get(request_offset..request_offset + static_prefix.len())?;
        if !request_prefix.eq_ignore_ascii_case(static_prefix) {
            return None;
        }
        normalized.push_str(static_prefix);
        request_offset += static_prefix.len();

        let close = open + template[open..].find('}')?;
        template_offset = close + 1;
        let next_open = template[template_offset..]
            .find('{')
            .map(|offset| template_offset + offset)
            .unwrap_or(template.len());
        let next_static = &template[template_offset..next_open];
        if next_static.is_empty() {
            if next_open != template.len() {
                return None;
            }
            normalized.push_str(&request[request_offset..]);
            request_offset = request.len();
            template_offset = template.len();
            break;
        }
        let remaining = &request[request_offset..];
        let matched_static = remaining
            .to_ascii_lowercase()
            .find(&next_static.to_ascii_lowercase())?;
        normalized.push_str(&remaining[..matched_static]);
        normalized.push_str(next_static);
        request_offset += matched_static + next_static.len();
        template_offset = next_open;
    }
    if template_offset < template.len() {
        let suffix = &template[template_offset..];
        let request_suffix = request.get(request_offset..)?;
        if !request_suffix.eq_ignore_ascii_case(suffix) {
            return None;
        }
        normalized.push_str(suffix);
        request_offset = request.len();
    }
    (request_offset == request.len()).then_some(normalized)
}

fn emby_route_templates() -> &'static [Vec<String>] {
    static TEMPLATES: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut templates = DEDICATED_ROUTE_TEMPLATES
            .iter()
            .map(|template| route_template_segments(template))
            .collect::<Vec<_>>();
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/emby_operations.json"))
                .expect("checked-in Emby operation inventory must be valid");
        for operation in contract["operations"]
            .as_array()
            .expect("Emby operation inventory must contain an operations array")
        {
            let path = operation["path"]
                .as_str()
                .expect("Emby operation path must be a string");
            let tag = operation["tag"]
                .as_str()
                .expect("Emby operation tag must be a string");
            if matches!(tag, "LiveTvService" | "PluginService")
                || path == "/LiveTv"
                || path.starts_with("/LiveTv/")
                || path.contains("QuickConnect")
            {
                continue;
            }
            let segments = route_template_segments(path);
            if !templates.contains(&segments) {
                templates.push(segments);
            }
        }
        // Literal routes must win before dynamic templates with the same
        // segment count, matching ASP.NET endpoint precedence.
        templates.sort_by_key(|segments| {
            segments
                .iter()
                .filter(|segment| segment.starts_with('{') && segment.ends_with('}'))
                .count()
        });
        templates
    })
}

fn route_template_segments(template: &str) -> Vec<String> {
    template
        .strip_prefix('/')
        .expect("Emby route templates are absolute")
        .split('/')
        .map(ToOwned::to_owned)
        .collect()
}

fn dedicated_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(jellyfin_api::emby_legacy_audio_hls_routes())
        .merge(jellyfin_api::emby_legacy_subtitle_delete_routes())
        .merge(jellyfin_api::emby_legacy_subtitle_hls_routes())
        .merge(jellyfin_api::emby_game_genre_routes())
        .merge(jellyfin_api::emby_game_routes())
        .merge(jellyfin_api::emby_item_image_routes())
        .merge(auth_user::routes())
        .merge(alternate_sources::routes())
        .merge(audiobooks::routes())
        .merge(backup::routes())
        .merge(bif::routes())
        .merge(camera_uploads::routes())
        .merge(collection_provider::routes())
        .merge(connect::routes())
        .merge(dlna::routes())
        .merge(dlna_server::routes())
        .merge(encoding::routes())
        .merge(environment::routes())
        .merge(generic_ui::routes())
        .merge(hide_from_resume::routes())
        .merge(home_sections::routes())
        .merge(library::routes())
        .merge(legacy_item_metadata::routes())
        .merge(live_stream_media_info::routes())
        .merge(metadata_reset::routes())
        .merge(notifications::routes())
        .merge(package_updates::routes())
        .merge(packages::routes())
        .merge(parties::routes())
        .merge(person_credits::routes())
        .merge(plugins::routes())
        .merge(recent_searches::routes())
        .merge(remote_images::routes())
        .merge(section_items::routes())
        .merge(sync::routes())
        .merge(system_misc::routes())
        .merge(tv_shows::routes())
        .merge(track_selections::routes())
        .merge(typed_settings::routes())
        .merge(users::routes())
        .merge(web_strings::routes())
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
        http::{HeaderName, HeaderValue, Method, StatusCode, header},
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
        let emby_handlers = dedicated_routes().with_state(Arc::new(state.clone()));
        let emby = router(state);

        assert_eq!(status(&jellyfin, "/GetUtcTime").await, StatusCode::OK);
        assert_eq!(status(&jellyfin, "/api/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&jellyfin, "/emby/GetUtcTime").await, StatusCode::OK);

        assert_eq!(status(&emby, "/emby/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/GetUtcTime").await, StatusCode::OK);
        assert_ne!(status(&emby, "/api/GetUtcTime").await, StatusCode::OK);

        let jellyfin_info = body(&jellyfin, "/System/Info/Public").await;
        let emby_info = body(&emby_handlers, "/System/Info/Public").await;
        assert!(jellyfin_info.get("ProductName").is_some());
        assert!(emby_info.get("ProductName").is_none());
        assert_eq!(emby_info["Version"], EMBY_API_VERSION);
        assert_eq!(emby_info["LocalAddresses"][0], "http://127.0.0.1:8096");
        assert!(emby_info["RemoteAddresses"].is_array());

        let jellyfin_info = body(&jellyfin, "/System/Info").await;
        let emby_info = body(&emby_handlers, "/System/Info").await;
        assert!(jellyfin_info.get("WebPath").is_some());
        assert!(emby_info.get("WebPath").is_none());
        assert!(emby_info.get("LocalAddresses").is_some());
        assert!(emby_info.get("CompletedInstallations").is_some());

        let jellyfin_branding = body(&jellyfin, "/Branding/Configuration").await;
        let emby_branding = body(&emby_handlers, "/Branding/Configuration").await;
        assert!(jellyfin_branding.get("SplashscreenEnabled").is_some());
        assert!(emby_branding.get("SplashscreenEnabled").is_none());
        assert_eq!(
            status(&emby, "/emby/branding/css.css").await,
            StatusCode::UNAUTHORIZED
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
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");

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
        assert_eq!(normalized_dedicated_path("/NotAnApi/NotAPrefix"), None);
    }

    #[tokio::test]
    async fn generated_supported_operations_have_mixed_case_dispatch() {
        let contract: ClientContract =
            serde_json::from_str(include_str!("../tests/fixtures/emby_operations.json"))
                .expect("checked-in Emby operation inventory must be valid");
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let shared_routes = jellyfin_api::unprefixed_router(state.clone());
        let dedicated_routes = dedicated_routes()
            .merge(swagger_alias_routes(shared_routes.clone()))
            .with_state(Arc::new(state));
        let dedicated = dedicated_routes
            .clone()
            .route_layer(middleware::from_fn(short_circuit_matched_route));
        let shared = shared_routes
            .clone()
            .route_layer(middleware::from_fn(short_circuit_matched_route));
        let all_routes = dedicated.clone().fallback_service(shared.clone());
        let mixed_case = case_insensitive_dedicated_routes(all_routes.clone());
        let mixed_case_routes = case_insensitive_dedicated_routes(
            dedicated_routes
                .clone()
                .fallback_service(shared_routes.clone()),
        );

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
            let expected_route =
                if method_is_allowed(&dedicated_routes, method.clone(), &canonical).await {
                    matched_route(&dedicated, method.clone(), &canonical).await
                } else if method_is_allowed(&shared_routes, method.clone(), &canonical).await {
                    matched_route(&shared, method.clone(), &canonical).await
                } else {
                    None
                };
            let Some(expected_route) = expected_route else {
                continue;
            };

            checked += 1;
            let path = alternating_ascii_case(&canonical);
            assert!(
                method_is_allowed(&mixed_case_routes, method.clone(), &path).await,
                "supported Emby operation lost its method dispatch: {} {} ({path})",
                operation.method,
                operation.path
            );
            assert_eq!(
                matched_route(&mixed_case, method, &path).await.as_deref(),
                Some(expected_route.as_str()),
                "supported Emby operation lost mixed-case dispatch: {} {} ({path})",
                operation.method,
                operation.path
            );
        }
        assert!(
            checked > 300,
            "expected to audit the supported Emby route surface"
        );
    }

    #[tokio::test]
    async fn generated_dedicated_operations_remain_owned_only_by_emby_tree() {
        let contract: ClientContract =
            serde_json::from_str(include_str!("../tests/fixtures/emby_operations.json"))
                .expect("checked-in Emby operation inventory must be valid");
        let state = AppState::new(
            DatabaseConnection::Disconnected,
            "API Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let shared = jellyfin_api::unprefixed_router(state.clone());
        let dedicated = dedicated_routes()
            .merge(swagger_alias_routes(shared))
            .with_state(Arc::new(state.clone()));
        let marked_dedicated = dedicated
            .clone()
            .route_layer(middleware::from_fn(short_circuit_matched_route));
        let emby = Router::new().nest(
            EMBY_API_PREFIX,
            case_insensitive_dedicated_routes(marked_dedicated),
        );
        let combined = jellyfin_api::router(state).merge(emby);

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
            if !method_is_allowed(&dedicated, method.clone(), &canonical).await {
                continue;
            }
            checked += 1;

            for path in [canonical.clone(), alternating_ascii_case(&canonical)] {
                let emby_path = format!("{EMBY_API_PREFIX}{path}");
                assert!(
                    matched_route(&combined, method.clone(), &emby_path)
                        .await
                        .is_some(),
                    "dedicated Emby operation did not reach its owned tree: {} {} ({emby_path})",
                    operation.method,
                    operation.path,
                );
            }

            for path in [
                canonical.clone(),
                format!("/api{canonical}"),
                format!("/api{EMBY_API_PREFIX}{canonical}"),
            ] {
                assert!(
                    matched_route(&combined, method.clone(), &path)
                        .await
                        .is_none(),
                    "dedicated Emby operation leaked outside /emby: {} {} ({path})",
                    operation.method,
                    operation.path,
                );
            }
        }
        assert!(
            checked > 50,
            "expected to audit every dedicated Emby module"
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
        let shared_routes = jellyfin_api::unprefixed_router(state.clone());
        let dedicated_routes = dedicated_routes()
            .merge(swagger_alias_routes(shared_routes.clone()))
            .with_state(Arc::new(state));
        let dedicated = dedicated_routes
            .clone()
            .route_layer(middleware::from_fn(short_circuit_matched_route));
        let shared = shared_routes
            .clone()
            .route_layer(middleware::from_fn(short_circuit_matched_route));
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
            if !(route_matches(&dedicated, method.clone(), &path).await
                && method_is_allowed(&dedicated_routes, method.clone(), &path).await)
                && !(route_matches(&shared, method.clone(), &path).await
                    && method_is_allowed(&shared_routes, method, &path).await)
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

    async fn method_is_allowed(app: &Router, method: Method, path: &str) -> bool {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        response
            .headers()
            .get(header::ALLOW)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|allowed| {
                allowed
                    .split(',')
                    .any(|candidate| candidate.trim().eq_ignore_ascii_case(method.as_str()))
            })
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
