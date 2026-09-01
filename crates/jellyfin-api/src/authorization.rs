use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, Request, Uri},
    middleware::Next,
    response::Response,
};
use chrono::Local;

use crate::{
    ApiError, AppState,
    authentication::{self, AuthenticatedIdentity},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RoutePolicy {
    Public,
    Optional,
    Default,
    IgnoreParentalControl,
    FirstTimeSetupOrDefault,
    FirstTimeSetupOrIgnoreParentalControl,
    FirstTimeSetupOrElevated,
    Elevated,
    LocalOrElevated,
}

/// Applies Jellyfin's default authenticated-user policy, including parental schedules.
pub(crate) async fn require_default(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<AuthenticatedIdentity, ApiError> {
    require_default_with_remote(state, headers, uri, IpAddr::V4(Ipv4Addr::LOCALHOST)).await
}

pub(crate) async fn require_default_with_remote(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    remote_ip: IpAddr,
) -> Result<AuthenticatedIdentity, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.require_remote_access(state, remote_ip)?;
    identity.require_parental_schedule(Local::now().fixed_offset())?;
    Ok(identity)
}

/// Authenticates while deliberately bypassing parental schedules.
pub(crate) async fn require_ignore_parental_control(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<AuthenticatedIdentity, ApiError> {
    require_ignore_parental_control_with_remote(
        state,
        headers,
        uri,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )
    .await
}

pub(crate) async fn require_ignore_parental_control_with_remote(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    remote_ip: IpAddr,
) -> Result<AuthenticatedIdentity, ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.require_remote_access(state, remote_ip)?;
    Ok(identity)
}

/// Applies Jellyfin's startup-wizard-or-elevated authorization policy.
pub(crate) async fn require_first_time_setup_or_elevated(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(), ApiError> {
    let startup_completed = crate::startup::is_completed(state).await?;
    if !startup_completed {
        return Ok(());
    }

    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()
}

/// Applies Jellyfin's startup-wizard-or-any-authenticated-user policy.
///
/// Official controllers such as `LocalizationController` use
/// `FirstTimeSetupOrDefault`, which allows every authenticated user once the
/// startup wizard has completed instead of requiring administrator rights.
pub(crate) async fn require_first_time_setup_or_default(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(), ApiError> {
    require_first_time_setup_or_default_with_remote(
        state,
        headers,
        uri,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    )
    .await
}

pub(crate) async fn require_first_time_setup_or_default_with_remote(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    remote_ip: IpAddr,
) -> Result<(), ApiError> {
    let startup_completed = crate::startup::is_completed(state).await?;
    if !startup_completed {
        return Ok(());
    }

    require_default_with_remote(state, headers, uri, remote_ip).await?;
    Ok(())
}

/// Applies Jellyfin's first-time-setup-or-ignore-parental-control policy.
///
/// Anonymous requests are allowed until the startup wizard completes. After
/// that point the request must authenticate, but parental schedules are
/// intentionally bypassed just like Jellyfin's
/// `FirstTimeSetupOrIgnoreParentalControl` policy.
pub(crate) async fn require_first_time_setup_or_ignore_parental_control(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(), ApiError> {
    let startup_completed = crate::startup::is_completed(state).await?;
    if !startup_completed {
        return Ok(());
    }

    authentication::authenticated_identity(state, headers, Some(uri)).await?;
    Ok(())
}

/// Applies the official per-route authorization policy before a handler runs.
pub(crate) async fn require_route_auth(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let remote_ip = remote_ip(request.extensions().get::<ConnectInfo<SocketAddr>>());
    match route_policy(request.method(), request.uri().path()) {
        RoutePolicy::Public => Ok(next.run(request).await),
        RoutePolicy::Optional => {
            authentication::optional_authenticated_identity(
                &state,
                request.headers(),
                request.uri(),
            )
            .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::Default => {
            require_default_with_remote(&state, request.headers(), request.uri(), remote_ip)
                .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::IgnoreParentalControl => {
            require_ignore_parental_control_with_remote(
                &state,
                request.headers(),
                request.uri(),
                remote_ip,
            )
            .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::FirstTimeSetupOrIgnoreParentalControl => {
            require_first_time_setup_or_ignore_parental_control(
                &state,
                request.headers(),
                request.uri(),
            )
            .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::FirstTimeSetupOrDefault => {
            require_first_time_setup_or_default_with_remote(
                &state,
                request.headers(),
                request.uri(),
                remote_ip,
            )
            .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::FirstTimeSetupOrElevated => {
            require_first_time_setup_or_elevated_with_remote(
                &state,
                request.headers(),
                request.uri(),
                remote_ip,
            )
            .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::Elevated => {
            require_elevated_with_remote(&state, request.headers(), request.uri(), remote_ip)
                .await?;
            Ok(next.run(request).await)
        }
        RoutePolicy::LocalOrElevated => {
            if state.network_manager.is_in_local_network(remote_ip) {
                return Ok(next.run(request).await);
            }

            require_elevated_with_remote(&state, request.headers(), request.uri(), remote_ip)
                .await?;
            Ok(next.run(request).await)
        }
    }
}

async fn require_first_time_setup_or_elevated_with_remote(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    remote_ip: IpAddr,
) -> Result<(), ApiError> {
    let startup_completed = crate::startup::is_completed(state).await?;
    if !startup_completed {
        return Ok(());
    }

    require_elevated_with_remote(state, headers, uri, remote_ip).await
}

async fn require_elevated_with_remote(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    remote_ip: IpAddr,
) -> Result<(), ApiError> {
    let identity = authentication::authenticated_identity(state, headers, Some(uri)).await?;
    identity.require_remote_access(state, remote_ip)?;
    identity.require_administrator()?;
    Ok(())
}

#[allow(clippy::match_same_arms)]
#[allow(clippy::too_many_lines)]
fn route_policy(method: &Method, path: &str) -> RoutePolicy {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    // Unknown paths still go through auth so the existence of a route is not
    // leaked through the response status code.
    if !is_known_api_path(&segments) {
        return RoutePolicy::Default;
    }
    if segments
        .first()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("robots.txt"))
    {
        return RoutePolicy::Public;
    }
    if segments
        .first()
        .is_some_and(|segment| *segment == "api-docs")
        && segments != ["api-docs", "openapi.json"]
    {
        return RoutePolicy::Public;
    }

    match segments.as_slice() {
        ["health" | "GetUtcTime"] | ["api-docs", "openapi.json"] => RoutePolicy::Public,
        ["System", "Info", "Public"] | ["System", "Ping"] => RoutePolicy::Public,
        ["Branding", "Configuration"] => RoutePolicy::Public,
        ["Branding", "Css" | "Css.css"] => RoutePolicy::Public,
        ["Branding", "Splashscreen"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Branding", "Splashscreen"] if is_write(method) => RoutePolicy::Elevated,
        [
            "Users",
            "Public"
            | "AuthenticateByName"
            | "authenticatebyname"
            | "AuthenticateWithQuickConnect"
            | "ForgotPassword",
        ]
        | ["Users", "ForgotPassword", "Pin"] => RoutePolicy::Public,
        ["Users", _, "Authenticate"] => RoutePolicy::Public,
        ["QuickConnect", "Enabled" | "Initiate" | "Connect"] => RoutePolicy::Public,
        ["Startup" | "Environment", ..]
        | ["Library", "VirtualFolders", ..]
        | ["Libraries", "AvailableOptions"] => RoutePolicy::FirstTimeSetupOrElevated,
        ["Localization", ..] => RoutePolicy::FirstTimeSetupOrDefault,
        ["System", "Info"] => RoutePolicy::FirstTimeSetupOrIgnoreParentalControl,
        ["System", "Restart"] => RoutePolicy::LocalOrElevated,
        ["System", "ActivityLog", "Entries"]
        | ["System", "Logs", ..]
        | ["System", "Info", "Storage"]
        | ["System", "Shutdown"] => RoutePolicy::Elevated,
        ["ScheduledTasks", ..] => RoutePolicy::Elevated,
        ["Auth", "Keys", ..] | ["Auth", "Providers" | "PasswordResetProviders"] => {
            RoutePolicy::Elevated
        }
        ["Devices" | "Packages" | "Backup", ..] | ["Repositories"] => RoutePolicy::Elevated,
        ["web", "ConfigurationPages"] => RoutePolicy::Elevated,
        ["web", "ConfigurationPage"] | ["web", ..] => RoutePolicy::Public,
        ["System", "Configuration", "MetadataOptions", "Default"]
        | ["System", "Configuration", "Branding"] => RoutePolicy::Elevated,
        ["System", "Configuration", ..] if is_write(method) => RoutePolicy::Elevated,
        ["System", "Configuration"] | ["System", "Configuration", _] => RoutePolicy::Default,
        ["Users", "New"] => RoutePolicy::Elevated,
        ["Users", _, "Policy"] => RoutePolicy::Elevated,
        ["Users", "Me"] => RoutePolicy::Default,
        ["Users", _] if method == Method::DELETE => RoutePolicy::Elevated,
        ["User", _] if method == Method::DELETE => RoutePolicy::Elevated,
        ["Users", _] if method == Method::GET => RoutePolicy::IgnoreParentalControl,
        ["Users", _] => RoutePolicy::Default,
        ["LiveTv", "TunerHosts"] => RoutePolicy::Elevated,
        ["LiveTv", "ListingProviders", ..] => RoutePolicy::Elevated,
        ["Library", "MediaFolders" | "PhysicalPaths" | "Refresh"] => RoutePolicy::Elevated,
        ["Items", _, "Refresh" | "MetadataEditor" | "ExternalIdInfos"] => RoutePolicy::Elevated,
        ["Items", "RemoteSearch", "Person"] | ["Items", "RemoteSearch", "Apply", _] => {
            RoutePolicy::Elevated
        }
        ["Items", _, "ContentType"] | ["Items", _, "RemoteImages", "Download"] => {
            RoutePolicy::Elevated
        }
        ["Videos", _, _, "Subtitles", _, ..]
            if is_get_or_head(method) && subtitle_stream_segment(&segments) =>
        {
            RoutePolicy::Public
        }
        ["Videos", _, "Subtitles", _] if method == Method::DELETE => RoutePolicy::Default,
        ["Videos", _, _, "Attachments", _] if is_get_or_head(method) => RoutePolicy::Public,
        ["Audio", _, "hls", ..] => RoutePolicy::Public,
        ["Videos", _, "hls", ..] if hls_path_is_playlist(&segments) => RoutePolicy::Default,
        ["Videos", _, "hls", ..] => RoutePolicy::Public,
        ["Items", _, "Images", ..] if is_write(method) => RoutePolicy::Elevated,
        ["Items", _, "Images", ..] if is_get_or_head(method) && segments.len() > 3 => {
            RoutePolicy::Optional
        }
        ["UserImage"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Users", _, "Images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        [
            "Artists" | "Genres" | "Studios" | "MusicGenres" | "Persons",
            _,
            "Images",
            ..,
        ] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Plugins", _, _, "Image"] => RoutePolicy::Optional,
        ["Plugins", ..] => RoutePolicy::Elevated,
        _ => RoutePolicy::Default,
    }
}

fn is_known_api_path(segments: &[&str]) -> bool {
    let Some(first) = segments.first() else {
        return false;
    };
    matches!(
        first.to_ascii_lowercase().as_str(),
        "health"
            | "system"
            | "branding"
            | "channels"
            | "artists"
            | "search"
            | "backup"
            | "items"
            | "web"
            | "playback"
            | "livestreams"
            | "mediasegments"
            | "fallbackfont"
            | "audio"
            | "videos"
            | "plugins"
            | "packages"
            | "environment"
            | "localization"
            | "auth"
            | "devices"
            | "displaypreferences"
            | "users"
            | "user"
            | "userimage"
            | "userviews"
            | "startup"
            | "quickconnect"
            | "sessions"
            | "playingitems"
            | "userplayeditems"
            | "useritems"
            | "userfavoriteitems"
            | "collections"
            | "playlists"
            | "songs"
            | "albums"
            | "musicgenres"
            | "genres"
            | "studios"
            | "trailers"
            | "persons"
            | "library"
            | "libraries"
            | "shows"
            | "movies"
            | "livetv"
            | "syncplay"
            | "getutctime"
            | "document"
            | "clientlog"
            | "scheduledtasks"
            | "api-docs"
            | "repositories"
            | "robots.txt"
    )
}

fn hls_path_is_playlist(segments: &[&str]) -> bool {
    segments
        .last()
        .is_some_and(|segment| segment.to_ascii_lowercase().starts_with("stream."))
}

fn is_get_or_head(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

fn subtitle_stream_segment(segments: &[&str]) -> bool {
    segments
        .get(5)
        .is_some_and(|segment| segment.starts_with("Stream."))
        || segments
            .get(6)
            .is_some_and(|segment| segment.starts_with("Stream."))
}

fn is_write(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn remote_ip(connect_info: Option<&ConnectInfo<SocketAddr>>) -> IpAddr {
    connect_info.map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |info| match info.0.ip() {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address @ IpAddr::V4(_) => address,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_policy_defaults_to_authenticated_for_known_api_routes() {
        assert_eq!(route_policy(&Method::GET, "/Items"), RoutePolicy::Default);
        assert_eq!(
            route_policy(&Method::GET, "/Users/Me"),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(&Method::GET, "/System/Logs"),
            RoutePolicy::Elevated
        );
    }

    #[test]
    fn route_policy_preserves_anonymous_and_optional_endpoints() {
        assert_eq!(
            route_policy(&Method::GET, "/System/Info/Public"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/api-docs/openapi.json"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/api-docs/missing.json"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::POST, "/Users/AuthenticateByName"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::POST, "/System/Restart"),
            RoutePolicy::LocalOrElevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/Items/{item_id}/Images/Primary"),
            RoutePolicy::Optional
        );
        assert_eq!(
            route_policy(&Method::GET, "/Localization/Options"),
            RoutePolicy::FirstTimeSetupOrDefault
        );
        assert_eq!(
            route_policy(&Method::GET, "/Localization/Cultures"),
            RoutePolicy::FirstTimeSetupOrDefault
        );
        assert_eq!(
            route_policy(&Method::GET, "/Videos/{item_id}/hls/playlist/seg1.ts"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/Videos/{item_id}/hls/playlist/stream.m3u8"),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(
                &Method::GET,
                "/Videos/{item_id}/{media_source_id}/Subtitles/0/Stream.srt"
            ),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(
                &Method::GET,
                "/Videos/{item_id}/{media_source_id}/Subtitles/0/10000000/Stream.vtt"
            ),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(
                &Method::GET,
                "/Videos/{item_id}/{media_source_id}/Subtitles/0/subtitles.m3u8"
            ),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(&Method::GET, "/not-a-route"),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(
                &Method::GET,
                "/Videos/{item_id}/{media_source_id}/Attachments/0"
            ),
            RoutePolicy::Public
        );
    }
}
