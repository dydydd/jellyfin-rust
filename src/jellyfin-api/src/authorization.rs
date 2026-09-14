use std::{
    borrow::Cow,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{ConnectInfo, OriginalUri, State},
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
    Download,
    CameraUpload,
    SubtitleManagement,
    LyricManagement,
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
    // `Router::nest` strips `/emby` from the request URI before this shared
    // middleware runs. Axum retains the client-facing URI in `OriginalUri`,
    // while the Emby adapter has already normalized the routed URI's literal
    // segments. Use the former only to select the protocol and the latter for
    // policy matching. This gives mixed-case Emby operations their canonical
    // policy without making those case-insensitive rules affect root or
    // `/api` requests.
    let policy_path = policy_path(&request);
    let policy = route_policy(request.method(), &policy_path);
    match policy {
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
        RoutePolicy::Download
        | RoutePolicy::CameraUpload
        | RoutePolicy::SubtitleManagement
        | RoutePolicy::LyricManagement => {
            let identity =
                require_default_with_remote(&state, request.headers(), request.uri(), remote_ip)
                    .await?;
            let allowed = match (&identity, policy) {
                (AuthenticatedIdentity::ApiKey(_), _) => true,
                (AuthenticatedIdentity::Device(session), RoutePolicy::Download) => {
                    session.can_download_content()
                }
                (AuthenticatedIdentity::Device(session), RoutePolicy::CameraUpload) => {
                    session.can_upload_camera()
                }
                (AuthenticatedIdentity::Device(session), RoutePolicy::SubtitleManagement) => {
                    session.can_manage_subtitles()
                }
                (AuthenticatedIdentity::Device(session), RoutePolicy::LyricManagement) => {
                    session.can_manage_lyrics()
                }
                (AuthenticatedIdentity::Device(_), _) => false,
            };
            if !allowed {
                return Err(ApiError::Forbidden);
            }
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

fn policy_path(request: &Request<Body>) -> Cow<'_, str> {
    let is_emby_protocol = request
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| uri.0.path())
        .is_some_and(|path| path == "/emby" || path.starts_with("/emby/"));
    if is_emby_protocol {
        Cow::Owned(format!("/emby{}", request.uri().path()))
    } else {
        Cow::Borrowed(request.uri().path())
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
    // Protocol routers (currently `/emby`) run this shared middleware before
    // Axum's nested service strips their prefix. Apply the same policy to the
    // protocol path so public/setup routes do not become authenticated-only.
    let is_emby_protocol = path == "/emby" || path.starts_with("/emby/");
    let path = path
        .strip_prefix("/emby/")
        .or_else(|| (path == "/emby").then_some(""))
        .unwrap_or(path);
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    // Emby exposes the extensionless Swagger document publicly, while the
    // Jellyfin root intentionally keeps only its existing `.json` aliases.
    if is_emby_protocol
        && matches!(segments.as_slice(), [segment] if segment.eq_ignore_ascii_case("swagger"))
    {
        return RoutePolicy::Public;
    }
    // Emby's actual WebApp request DTOs mark these plugin-localization routes
    // unauthenticated even though the generated Swagger says user auth. Keep
    // that protocol-local correction case-insensitive.
    if is_emby_protocol
        && matches!(segments.as_slice(), [web, endpoint]
            if web.eq_ignore_ascii_case("web")
                && ["strings", "stringset"]
                    .iter()
                    .any(|candidate| endpoint.eq_ignore_ascii_case(candidate)))
    {
        return RoutePolicy::Public;
    }
    // Generic UI controllers expose plugin setup state and both operations are
    // protected by the service-level administrator role in Emby 4.10.
    if is_emby_protocol
        && matches!(segments.as_slice(), [ui, endpoint]
            if ui.eq_ignore_ascii_case("UI")
                && ["View", "Command"]
                    .iter()
                    .any(|candidate| endpoint.eq_ignore_ascii_case(candidate)))
    {
        return RoutePolicy::Elevated;
    }
    // Persisted Emby DLNA profiles are administrator configuration.  Keep the
    // rule protocol-local because Jellyfin has no matching unprefixed route.
    if is_emby_protocol
        && matches!(segments.as_slice(), [dlna, profiles]
            if dlna.eq_ignore_ascii_case("Dlna")
                && profiles.eq_ignore_ascii_case("ProfileInfos"))
    {
        return RoutePolicy::Elevated;
    }
    // Emby's UPnP transport controller is deliberately unauthenticated. Keep
    // this exception protocol-local so similarly shaped unknown requests in
    // the Jellyfin root and `/api` trees retain their normal fail-closed
    // authorization precedence. The neighbouring profile routes are elevated.
    if is_emby_protocol && is_public_emby_dlna_server_route(&segments) {
        return RoutePolicy::Public;
    }
    // Emby's camera upload action uses its named `cameraupload` role. Apply
    // the protocol-private user-policy flag before the handler extracts the
    // required query or starts consuming a potentially large request body.
    if is_emby_protocol
        && matches!(segments.as_slice(), [devices, camera_uploads]
            if devices.eq_ignore_ascii_case("Devices")
                && camera_uploads.eq_ignore_ascii_case("CameraUploads"))
    {
        return if method == Method::POST {
            RoutePolicy::CameraUpload
        } else {
            RoutePolicy::Default
        };
    }
    // Package update discovery is administrator-only in the generated Emby
    // contract. Match both static segments case-insensitively because the
    // protocol router normalizes dispatch without rewriting OriginalUri,
    // which is the source used for authorization policy selection.
    if is_emby_protocol
        && matches!(segments.as_slice(), [packages, updates]
            if packages.eq_ignore_ascii_case("Packages")
                && updates.eq_ignore_ascii_case("Updates"))
    {
        return RoutePolicy::Elevated;
    }
    // Emby's generated contract requires an authenticated user for discovery
    // and branding routes that Jellyfin deliberately exposes publicly. Keep
    // these overrides protocol-local so the root and `/api` trees retain
    // Jellyfin's existing anonymous bootstrap behavior.
    if is_emby_protocol
        && (matches!(segments.as_slice(), [system, ping]
            if system.eq_ignore_ascii_case("System") && ping.eq_ignore_ascii_case("Ping"))
            || matches!(segments.as_slice(), [system, info, public]
                if system.eq_ignore_ascii_case("System")
                    && info.eq_ignore_ascii_case("Info")
                    && public.eq_ignore_ascii_case("Public"))
            || matches!(segments.as_slice(), [branding, endpoint]
                if branding.eq_ignore_ascii_case("Branding")
                    && ["Configuration", "Css", "Css.css"]
                        .iter()
                        .any(|candidate| endpoint.eq_ignore_ascii_case(candidate))))
    {
        return RoutePolicy::Default;
    }
    // Feature discovery reveals installed server capabilities in Emby and is
    // explicitly administrator-only in its generated wire contract.
    if is_emby_protocol
        && matches!(segments.as_slice(), [features] if features.eq_ignore_ascii_case("Features"))
    {
        return RoutePolicy::Elevated;
    }
    // Legacy Emby Connect is separate from Jellyfin QuickConnect. The
    // generated Emby contract marks pending-link discovery and every user
    // link mutation as administrator-only. Keep these rules protocol-local
    // and match static segments case-insensitively just like Emby's router.
    if is_emby_protocol
        && (matches!(segments.as_slice(), [connect, pending]
            if connect.eq_ignore_ascii_case("Connect")
                && pending.eq_ignore_ascii_case("Pending"))
            || matches!(segments.as_slice(), [users, _, connect, link]
                if users.eq_ignore_ascii_case("Users")
                    && connect.eq_ignore_ascii_case("Connect")
                    && link.eq_ignore_ascii_case("Link"))
            || matches!(segments.as_slice(), [users, _, connect, link, delete]
                if users.eq_ignore_ascii_case("Users")
                    && connect.eq_ignore_ascii_case("Connect")
                    && link.eq_ignore_ascii_case("Link")
                    && delete.eq_ignore_ascii_case("Delete")))
    {
        return RoutePolicy::Elevated;
    }
    // Emby's generated Android/iOS contract exposes this legacy DELETE to any
    // authenticated user. Keep Jellyfin's unprefixed endpoint on the current
    // RequiresElevation policy.
    if is_emby_protocol
        && method == Method::DELETE
        && matches!(
            segments.as_slice(),
            ["Videos", _, "Subtitles", _] | ["videos", _, "subtitles", _]
        )
    {
        return RoutePolicy::Default;
    }
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
    // OriginalUri intentionally retains the client-facing casing even after
    // the Emby adapter normalizes its dispatch URI. Apply ASP.NET's
    // case-insensitive static-segment behavior here as well so mixed-case
    // login routes do not accidentally fall back to authenticated-only.
    if is_emby_protocol
        && segments
            .first()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("Users"))
        && (matches!(segments.as_slice(), [_, action] if [
            "Public",
            "AuthenticateByName",
            "ForgotPassword",
        ]
        .iter()
        .any(|candidate| action.eq_ignore_ascii_case(candidate)))
            || matches!(segments.as_slice(), [_, action, pin]
                if action.eq_ignore_ascii_case("ForgotPassword")
                    && pin.eq_ignore_ascii_case("Pin"))
            || matches!(segments.as_slice(), [_, _, action]
                if action.eq_ignore_ascii_case("Authenticate")))
    {
        return RoutePolicy::Public;
    }
    if is_emby_protocol
        && matches!(segments.as_slice(), [users, _, action]
        if users.eq_ignore_ascii_case("Users") && action.eq_ignore_ascii_case("Policy"))
    {
        return RoutePolicy::Elevated;
    }
    if is_emby_protocol
        && matches!(segments.as_slice(), [users, _, action]
        if users.eq_ignore_ascii_case("Users") && action.eq_ignore_ascii_case("Configuration"))
    {
        return RoutePolicy::Default;
    }

    match segments.as_slice() {
        ["health" | "GetUtcTime" | "getutctime" | "metrics"]
        | ["api-docs", "openapi.json"]
        | ["openapi" | "openapi.json" | "swagger.json"] => RoutePolicy::Public,
        ["System", "Info", "Public"]
        | ["system", "info", "public"]
        | ["System", "Ping"]
        | ["system", "ping"] => RoutePolicy::Public,
        ["Branding", "Configuration"] | ["branding", "configuration"] => RoutePolicy::Public,
        ["Branding", "Css" | "Css.css"] | ["branding", "css" | "css.css"] => RoutePolicy::Public,
        ["Branding", "Splashscreen"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Branding", "Splashscreen"] if is_write(method) => RoutePolicy::Elevated,
        ["branding", "splashscreen"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["branding", "splashscreen"] if is_write(method) => RoutePolicy::Elevated,
        [
            "Users",
            "Public"
            | "AuthenticateByName"
            | "authenticatebyname"
            | "AuthenticateWithQuickConnect"
            | "ForgotPassword",
        ]
        | ["Users", "ForgotPassword", "Pin"]
        | [
            "users",
            "public" | "authenticatebyname" | "authenticatewithquickconnect" | "forgotpassword",
        ]
        | ["users", "forgotpassword", "pin"] => RoutePolicy::Public,
        ["Users" | "users", _, "Authenticate" | "authenticate"] => RoutePolicy::Public,
        [
            "QuickConnect" | "quickconnect",
            "Enabled" | "enabled" | "Initiate" | "initiate" | "Connect" | "connect",
        ] => RoutePolicy::Public,
        ["Startup" | "startup" | "Environment" | "environment", ..]
        | [
            "Library" | "library",
            "VirtualFolders" | "virtualfolders",
            ..,
        ]
        | [
            "Libraries" | "libraries",
            "AvailableOptions" | "availableoptions",
        ] => RoutePolicy::FirstTimeSetupOrElevated,
        ["Localization" | "localization", ..] => RoutePolicy::FirstTimeSetupOrDefault,
        ["System", "Info"] | ["system", "info"] => {
            RoutePolicy::FirstTimeSetupOrIgnoreParentalControl
        }
        ["System", "Restart"] | ["system", "restart"] => RoutePolicy::LocalOrElevated,
        ["System", "ActivityLog", "Entries"]
        | ["system", "activitylog", "entries"]
        | ["System", "Logs", ..]
        | ["system", "logs", ..]
        | ["System", "Info", "Storage"]
        | ["system", "info", "storage"]
        | ["System", "Shutdown"]
        | ["system", "shutdown"] => RoutePolicy::Elevated,
        ["ScheduledTasks" | "scheduledtasks", ..] => RoutePolicy::Elevated,
        ["Auth" | "auth", "Keys" | "keys", ..]
        | [
            "Auth" | "auth",
            "Providers" | "providers" | "PasswordResetProviders" | "passwordresetproviders",
        ] => RoutePolicy::Elevated,
        ["Devices" | "devices" | "Packages" | "Backup" | "backup", ..] | ["Repositories"] => {
            RoutePolicy::Elevated
        }
        ["web", "ConfigurationPages"] | ["web", "configurationpages"] => RoutePolicy::Elevated,
        ["web", "ConfigurationPage"] | ["web", "configurationpage"] | ["web", ..] => {
            RoutePolicy::Public
        }
        ["System", "Configuration", "MetadataOptions", "Default"]
        | ["system", "configuration", "metadataoptions", "default"] => RoutePolicy::Elevated,
        ["System", "Configuration", ..] | ["system", "configuration", ..] if is_write(method) => {
            RoutePolicy::Elevated
        }
        ["System", "Configuration"]
        | ["system", "configuration"]
        | ["System", "Configuration", _]
        | ["system", "configuration", _] => RoutePolicy::Default,
        ["Users", "New"] | ["users", "new"] => RoutePolicy::Elevated,
        ["Users", _, "Policy"] | ["users", _, "policy"] => RoutePolicy::Elevated,
        ["Users", "Me"] | ["users", "me"] => RoutePolicy::Default,
        ["Users" | "users", _] if method == Method::DELETE => RoutePolicy::Elevated,
        ["User", _] if method == Method::DELETE => RoutePolicy::Elevated,
        ["Users" | "users", _] if method == Method::GET => RoutePolicy::IgnoreParentalControl,
        ["Users", _] => RoutePolicy::Default,
        ["LiveTv", "TunerHosts"] => RoutePolicy::Elevated,
        ["LiveTv", "ListingProviders", ..] => RoutePolicy::Elevated,
        [
            "Library" | "library",
            "MediaFolders"
            | "PhysicalPaths"
            | "Refresh"
            | "SelectableMediaFolders"
            | "mediafolders"
            | "physicalpaths"
            | "refresh"
            | "selectablemediafolders",
        ] => RoutePolicy::Elevated,
        [
            "Items" | "items",
            _,
            "Refresh" | "MetadataEditor" | "ExternalIdInfos" | "refresh" | "metadataeditor"
            | "externalidinfos",
        ] => RoutePolicy::Elevated,
        ["Items", "RemoteSearch", "Person"] | ["Items", "RemoteSearch", "Apply", _] => {
            RoutePolicy::Elevated
        }
        ["items", "remotesearch", "person"] | ["items", "remotesearch", "apply", _] => {
            RoutePolicy::Elevated
        }
        ["Items", _, "ContentType"] | ["Items", _, "RemoteImages", "Download"] => {
            RoutePolicy::Elevated
        }
        ["items", _, "remoteimages", "download"] => RoutePolicy::Elevated,
        ["Items", _, "Download"] | ["items", _, "download"] if is_get_or_head(method) => {
            RoutePolicy::Download
        }
        ["Items", _, "RemoteSearch", "Subtitles", _]
        | ["items", _, "remotesearch", "subtitles", _]
            if is_get_or_head(method) || method == Method::POST =>
        {
            RoutePolicy::SubtitleManagement
        }
        ["Providers", "Subtitles", "Subtitles", _] | ["providers", "subtitles", "subtitles", _]
            if is_get_or_head(method) =>
        {
            RoutePolicy::SubtitleManagement
        }
        ["Videos", _, "Subtitles"] | ["videos", _, "subtitles"] if method == Method::POST => {
            RoutePolicy::SubtitleManagement
        }
        ["Audio", _, "Lyrics"] | ["audio", _, "lyrics"]
            if matches!(*method, Method::POST | Method::DELETE) =>
        {
            RoutePolicy::LyricManagement
        }
        ["Audio", _, "RemoteSearch", "Lyrics"] | ["audio", _, "remotesearch", "lyrics"]
            if is_get_or_head(method) =>
        {
            RoutePolicy::LyricManagement
        }
        ["Audio", _, "RemoteSearch", "Lyrics", _] | ["audio", _, "remotesearch", "lyrics", _]
            if method == Method::POST =>
        {
            RoutePolicy::LyricManagement
        }
        ["Providers", "Lyrics", _] | ["providers", "lyrics", _] if is_get_or_head(method) => {
            RoutePolicy::LyricManagement
        }
        ["Videos", "MergeVersions"] | ["videos", "mergeversions"] if method == Method::POST => {
            RoutePolicy::Elevated
        }
        ["Videos", _, "AlternateSources"] | ["videos", _, "alternatesources"]
            if method == Method::DELETE =>
        {
            RoutePolicy::Elevated
        }
        ["Videos", _, _, "Subtitles", _, ..]
            if is_get_or_head(method) && subtitle_stream_segment(&segments) =>
        {
            RoutePolicy::Public
        }
        ["videos", _, _, "subtitles", _, ..]
            if is_get_or_head(method) && subtitle_stream_segment(&segments) =>
        {
            RoutePolicy::Public
        }
        ["Videos", _, "Subtitles", _] | ["videos", _, "subtitles", _]
            if method == Method::DELETE =>
        {
            RoutePolicy::Elevated
        }
        ["Videos", _, _, "Attachments", _] | ["Videos", _, _, "Attachments", _, "Stream"]
            if is_get_or_head(method) =>
        {
            RoutePolicy::Public
        }
        ["videos", _, _, "attachments", _] | ["videos", _, _, "attachments", _, "stream"]
            if is_get_or_head(method) =>
        {
            RoutePolicy::Public
        }
        ["Audio", _, "hls", ..] => RoutePolicy::Public,
        ["Videos", _, "hls", ..] if hls_path_is_playlist(&segments) => RoutePolicy::Default,
        ["Videos", _, "hls", ..] => RoutePolicy::Public,
        ["Items", _, "Images", ..] if is_write(method) => RoutePolicy::Elevated,
        ["Items", _, "Images", ..] if is_get_or_head(method) && segments.len() > 3 => {
            RoutePolicy::Optional
        }
        ["items", _, "images", ..] if is_write(method) => RoutePolicy::Elevated,
        ["items", _, "images", ..] if is_get_or_head(method) && segments.len() > 3 => {
            RoutePolicy::Optional
        }
        ["UserImage"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["userimage"] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Users", _, "Images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        ["users", _, "images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        [
            "Artists" | "Genres" | "Studios" | "MusicGenres" | "Persons",
            _,
            "Images",
            ..,
        ] if is_get_or_head(method) => RoutePolicy::Optional,
        ["studios", _, "images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        ["artists" | "genres", _, "images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        ["musicgenres", _, "images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        ["persons", _, "images", ..] if is_get_or_head(method) => RoutePolicy::Optional,
        ["Plugins", _, _, "Image"] => RoutePolicy::Optional,
        ["plugins", _, _, "image"] => RoutePolicy::Optional,
        ["Plugins" | "plugins", ..] => RoutePolicy::Elevated,
        _ => RoutePolicy::Default,
    }
}

fn is_public_emby_dlna_server_route(segments: &[&str]) -> bool {
    matches!(segments, [dlna, icons, _]
        if dlna.eq_ignore_ascii_case("Dlna") && icons.eq_ignore_ascii_case("icons"))
        || matches!(segments, [dlna, _, icons, _]
            if dlna.eq_ignore_ascii_case("Dlna") && icons.eq_ignore_ascii_case("icons"))
        || matches!(segments, [dlna, _, endpoint]
            if dlna.eq_ignore_ascii_case("Dlna")
                && ["description", "description.xml"]
                    .iter()
                    .any(|candidate| endpoint.eq_ignore_ascii_case(candidate)))
        || matches!(segments, [dlna, _, service, endpoint]
            if dlna.eq_ignore_ascii_case("Dlna")
                && ((service.eq_ignore_ascii_case("contentdirectory")
                    && ["contentdirectory", "contentdirectory.xml", "control"]
                        .iter()
                        .any(|candidate| endpoint.eq_ignore_ascii_case(candidate)))
                    || (service.eq_ignore_ascii_case("connectionmanager")
                        && ["connectionmanager", "connectionmanager.xml", "control"]
                            .iter()
                            .any(|candidate| endpoint.eq_ignore_ascii_case(candidate)))))
}

fn is_known_api_path(segments: &[&str]) -> bool {
    let Some(first) = segments.first() else {
        return false;
    };
    matches!(
        first.to_ascii_lowercase().as_str(),
        "health"
            | "metrics"
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
            | "providers"
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
            | "openapi"
            | "openapi.json"
            | "swagger.json"
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
        .is_some_and(|segment| starts_with_ignore_ascii_case(segment, "Stream."))
        || segments
            .get(6)
            .is_some_and(|segment| starts_with_ignore_ascii_case(segment, "Stream."))
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
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
    fn emby_policy_uses_normalized_routed_path_without_affecting_other_trees() {
        let mut emby = Request::get("/Items/not-a-uuid/MetadataEditor")
            .body(Body::empty())
            .expect("request");
        emby.extensions_mut().insert(OriginalUri(
            "/emby/iTeMs/not-a-uuid/mEtAdAtAeDiToR"
                .parse()
                .expect("original URI"),
        ));
        let emby_path = policy_path(&emby);
        assert_eq!(emby_path, "/emby/Items/not-a-uuid/MetadataEditor");
        assert_eq!(
            route_policy(emby.method(), &emby_path),
            RoutePolicy::Elevated
        );

        for original in [
            "/iTeMs/not-a-uuid/mEtAdAtAeDiToR",
            "/api/iTeMs/not-a-uuid/mEtAdAtAeDiToR",
        ] {
            let mut jellyfin = Request::get("/iTeMs/not-a-uuid/mEtAdAtAeDiToR")
                .body(Body::empty())
                .expect("request");
            jellyfin.extensions_mut().insert(OriginalUri(
                original.parse().expect("Jellyfin original URI"),
            ));
            let jellyfin_path = policy_path(&jellyfin);
            assert_eq!(jellyfin_path, "/iTeMs/not-a-uuid/mEtAdAtAeDiToR");
            assert_eq!(
                route_policy(jellyfin.method(), &jellyfin_path),
                RoutePolicy::Default,
                "Emby mixed-case authorization leaked into {original}",
            );
        }
    }

    #[test]
    fn route_policy_defaults_to_authenticated_for_known_api_routes() {
        assert_eq!(route_policy(&Method::GET, "/Items"), RoutePolicy::Default);
        assert_eq!(
            route_policy(&Method::GET, "/Users/Me"),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(&Method::GET, "/users/me"),
            RoutePolicy::Default
        );
        assert_eq!(
            route_policy(&Method::GET, "/System/Logs"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/system/activitylog/entries"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::POST, "/system/configuration/branding"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::POST, "/system/restart"),
            RoutePolicy::LocalOrElevated
        );
        assert_eq!(
            route_policy(&Method::POST, "/emby/uSeRs/not-a-uuid/PoLiCy"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::POST, "/emby/uSeRs/not-a-uuid/CoNfIgUrAtIoN"),
            RoutePolicy::Default
        );
    }

    #[test]
    fn route_policy_preserves_anonymous_and_optional_endpoints() {
        assert_eq!(
            route_policy(&Method::GET, "/System/Info/Public"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/System/Ping"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/system/info/public"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/branding/configuration"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/system/info/storage"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/api-docs/openapi.json"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::GET, "/emby/Localization/Cultures"),
            RoutePolicy::FirstTimeSetupOrDefault
        );
        assert_eq!(
            route_policy(&Method::GET, "/api-docs/missing.json"),
            RoutePolicy::Public
        );
        assert_eq!(
            route_policy(&Method::POST, "/Users/AuthenticateByName"),
            RoutePolicy::Public
        );
        for route in [
            "/emby/uSeRs/aUtHeNtIcAtEbYnAmE",
            "/emby/uSeRs/user-id/aUtHeNtIcAtE",
        ] {
            assert_eq!(
                route_policy(&Method::POST, route),
                RoutePolicy::Public,
                "mixed-case Emby login route {route}"
            );
        }
        for route in [
            "/Users/fORGOTpASSWORD",
            "/api/Users/fORGOTpASSWORD",
            "/Users/user-id/pOLICY",
            "/api/Users/user-id/cONFIGURATION",
        ] {
            assert_eq!(
                route_policy(&Method::POST, route),
                RoutePolicy::Default,
                "Emby mixed-case policy must not leak into Jellyfin route {route}"
            );
        }
        for route in ["/users/forgotpassword", "/users/forgotpassword/pin"] {
            assert_eq!(
                route_policy(&Method::POST, route),
                RoutePolicy::Public,
                "route {route}"
            );
        }
        for route in [
            "/auth/keys",
            "/auth/keys/token",
            "/auth/providers",
            "/auth/passwordresetproviders",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Elevated,
                "route {route}"
            );
        }
        assert_eq!(
            route_policy(&Method::POST, "/System/Restart"),
            RoutePolicy::LocalOrElevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/auth/providers"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/auth/keys"),
            RoutePolicy::Elevated
        );
        assert_eq!(
            route_policy(&Method::GET, "/Items/{item_id}/Images/Primary"),
            RoutePolicy::Optional
        );
        for route in [
            "/items/{item_id}/images/Primary",
            "/artists/name/images/Primary/0",
            "/genres/name/images/Primary/0",
            "/branding/splashscreen",
            "/userimage",
            "/users/{user_id}/images/Primary",
            "/plugins/{plugin_id}/1.0/image",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Optional,
                "route {route}"
            );
        }
        assert_eq!(
            route_policy(&Method::POST, "/items/{item_id}/remoteimages/download"),
            RoutePolicy::Elevated
        );
        for route in [
            "/Videos/{item_id}/Subtitles/0",
            "/videos/{item_id}/subtitles/0",
        ] {
            assert_eq!(
                route_policy(&Method::DELETE, route),
                RoutePolicy::Elevated,
                "route {route}",
            );
        }
        for (method, route) in [
            (Method::POST, "/Videos/MergeVersions"),
            (Method::POST, "/videos/mergeversions"),
            (Method::DELETE, "/Videos/{item_id}/AlternateSources"),
            (Method::DELETE, "/videos/{item_id}/alternatesources"),
        ] {
            assert_eq!(
                route_policy(&method, route),
                RoutePolicy::Elevated,
                "route {route}",
            );
        }
        for route in [
            "/Items/RemoteSearch/Person",
            "/items/remotesearch/person",
            "/Items/RemoteSearch/Apply/{item_id}",
            "/items/remotesearch/apply/{item_id}",
        ] {
            assert_eq!(
                route_policy(&Method::POST, route),
                RoutePolicy::Elevated,
                "route {route}",
            );
        }
        assert_eq!(
            route_policy(&Method::GET, "/Localization/Options"),
            RoutePolicy::FirstTimeSetupOrDefault
        );
        assert_eq!(
            route_policy(&Method::GET, "/Localization/Cultures"),
            RoutePolicy::FirstTimeSetupOrDefault
        );
        for route in [
            "/localization/cultures",
            "/localization/countries",
            "/localization/parentalratings",
            "/localization/options",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::FirstTimeSetupOrDefault,
                "{route}"
            );
        }
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

    #[test]
    fn emby_discovery_authorization_matches_generated_contract() {
        for method in [Method::GET, Method::POST, Method::HEAD] {
            for route in [
                "/emby/System/Ping",
                "/emby/system/ping",
                "/emby/sYsTeM/pInG",
            ] {
                assert_eq!(
                    route_policy(&method, route),
                    RoutePolicy::Default,
                    "Emby ping must authenticate for {method} {route}",
                );
            }
        }

        for route in [
            "/emby/System/Info/Public",
            "/emby/system/info/public",
            "/emby/sYsTeM/iNfO/pUbLiC",
            "/emby/Branding/Configuration",
            "/emby/branding/configuration",
            "/emby/bRaNdInG/cOnFiGuRaTiOn",
            "/emby/Branding/Css",
            "/emby/branding/css",
            "/emby/bRaNdInG/cSs",
            "/emby/Branding/Css.css",
            "/emby/branding/css.css",
            "/emby/bRaNdInG/cSs.CsS",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Default,
                "Emby user-authenticated route {route}",
            );
        }

        for route in [
            "/emby/Features",
            "/emby/features",
            "/emby/fEaTuReS",
            "/emby/Packages/Updates",
            "/emby/packages/updates",
            "/emby/pAcKaGeS/uPdAtEs",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Elevated,
                "Emby administrator route {route}",
            );
        }
    }

    #[test]
    fn emby_discovery_overrides_preserve_public_protocol_exceptions() {
        for route in [
            "/emby/swagger",
            "/emby/sWaGgEr",
            "/emby/Users/Public",
            "/emby/uSeRs/pUbLiC",
            "/emby/Users/AuthenticateByName",
            "/emby/uSeRs/aUtHeNtIcAtEbYnAmE",
            "/emby/Users/ForgotPassword",
            "/emby/uSeRs/fOrGoTpAsSwOrD",
            "/emby/Users/ForgotPassword/Pin",
            "/emby/uSeRs/fOrGoTpAsSwOrD/pIn",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Public,
                "existing Emby public exception {route}",
            );
        }

        for route in [
            "/System/Info/Public",
            "/system/info/public",
            "/System/Ping",
            "/system/ping",
            "/Branding/Configuration",
            "/branding/configuration",
            "/Branding/Css",
            "/branding/css",
            "/Branding/Css.css",
            "/branding/css.css",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Public,
                "Jellyfin public route {route}",
            );
        }
    }

    #[test]
    fn named_user_permission_routes_match_official_controller_policies() {
        for (method, canonical, lowercase, expected) in [
            (
                Method::GET,
                "/Items/item-id/Download",
                "/items/item-id/download",
                RoutePolicy::Download,
            ),
            (
                Method::GET,
                "/Items/item-id/RemoteSearch/Subtitles/eng",
                "/items/item-id/remotesearch/subtitles/eng",
                RoutePolicy::SubtitleManagement,
            ),
            (
                Method::POST,
                "/Items/item-id/RemoteSearch/Subtitles/provider-id",
                "/items/item-id/remotesearch/subtitles/provider-id",
                RoutePolicy::SubtitleManagement,
            ),
            (
                Method::GET,
                "/Providers/Subtitles/Subtitles/provider-id",
                "/providers/subtitles/subtitles/provider-id",
                RoutePolicy::SubtitleManagement,
            ),
            (
                Method::POST,
                "/Videos/item-id/Subtitles",
                "/videos/item-id/subtitles",
                RoutePolicy::SubtitleManagement,
            ),
            (
                Method::POST,
                "/Audio/item-id/Lyrics",
                "/audio/item-id/lyrics",
                RoutePolicy::LyricManagement,
            ),
            (
                Method::DELETE,
                "/Audio/item-id/Lyrics",
                "/audio/item-id/lyrics",
                RoutePolicy::LyricManagement,
            ),
            (
                Method::GET,
                "/Audio/item-id/RemoteSearch/Lyrics",
                "/audio/item-id/remotesearch/lyrics",
                RoutePolicy::LyricManagement,
            ),
            (
                Method::POST,
                "/Audio/item-id/RemoteSearch/Lyrics/provider-id",
                "/audio/item-id/remotesearch/lyrics/provider-id",
                RoutePolicy::LyricManagement,
            ),
            (
                Method::GET,
                "/Providers/Lyrics/provider-id",
                "/providers/lyrics/provider-id",
                RoutePolicy::LyricManagement,
            ),
        ] {
            assert_eq!(route_policy(&method, canonical), expected, "{canonical}");
            assert_eq!(route_policy(&method, lowercase), expected, "{lowercase}");
        }

        assert_eq!(
            route_policy(&Method::GET, "/Audio/item-id/Lyrics"),
            RoutePolicy::Default,
        );
        assert_eq!(
            route_policy(&Method::DELETE, "/Videos/item-id/Subtitles/0"),
            RoutePolicy::Elevated,
        );
        assert_eq!(
            route_policy(&Method::POST, "/emby/Videos/item-id/Subtitles/-1/Delete"),
            RoutePolicy::Default,
        );
        assert_eq!(
            route_policy(&Method::POST, "/emby/videos/item-id/subtitles/-1/delete"),
            RoutePolicy::Default,
        );
        assert_eq!(
            route_policy(&Method::DELETE, "/emby/Videos/item-id/Subtitles/-1"),
            RoutePolicy::Default,
        );
        assert_eq!(
            route_policy(&Method::DELETE, "/emby/videos/item-id/subtitles/-1"),
            RoutePolicy::Default,
        );
    }

    #[test]
    fn lowercase_system_routes_preserve_canonical_authorization() {
        for (method, canonical, lowercase) in [
            (Method::GET, "/GetUtcTime", "/getutctime"),
            (Method::GET, "/System/Ping", "/system/ping"),
            (
                Method::GET,
                "/System/ActivityLog/Entries",
                "/system/activitylog/entries",
            ),
            (Method::GET, "/System/Logs", "/system/logs"),
            (Method::GET, "/System/Logs/Log", "/system/logs/log"),
            (Method::GET, "/System/Endpoint", "/system/endpoint"),
            (Method::POST, "/System/Restart", "/system/restart"),
            (Method::POST, "/System/Shutdown", "/system/shutdown"),
            (Method::GET, "/ScheduledTasks", "/scheduledtasks"),
            (
                Method::GET,
                "/ScheduledTasks/task-id",
                "/scheduledtasks/task-id",
            ),
            (
                Method::POST,
                "/ScheduledTasks/Running/task-id",
                "/scheduledtasks/running/task-id",
            ),
            (
                Method::DELETE,
                "/ScheduledTasks/Running/task-id",
                "/scheduledtasks/running/task-id",
            ),
            (
                Method::POST,
                "/ScheduledTasks/task-id/Triggers",
                "/scheduledtasks/task-id/triggers",
            ),
            (
                Method::GET,
                "/System/Configuration",
                "/system/configuration",
            ),
            (
                Method::POST,
                "/System/Configuration",
                "/system/configuration",
            ),
            (
                Method::GET,
                "/System/Configuration/MetadataOptions/Default",
                "/system/configuration/metadataoptions/default",
            ),
            (
                Method::POST,
                "/System/Configuration/Branding",
                "/system/configuration/branding",
            ),
            (
                Method::GET,
                "/System/Configuration/Branding",
                "/system/configuration/branding",
            ),
            (
                Method::GET,
                "/System/Configuration/encoding",
                "/system/configuration/encoding",
            ),
            (
                Method::POST,
                "/System/Configuration/encoding",
                "/system/configuration/encoding",
            ),
            (Method::POST, "/ClientLog/Document", "/clientlog/document"),
        ] {
            assert_eq!(
                route_policy(&method, canonical),
                route_policy(&method, lowercase),
                "lowercase route {lowercase} must preserve {canonical} authorization",
            );
        }
    }

    #[test]
    fn lowercase_backup_routes_remain_elevated() {
        for (method, canonical, lowercase) in [
            (Method::GET, "/Backup", "/backup"),
            (Method::POST, "/Backup/Create", "/backup/create"),
            (Method::GET, "/Backup/Manifest", "/backup/manifest"),
            (Method::POST, "/Backup/Restore", "/backup/restore"),
        ] {
            assert_eq!(route_policy(&method, canonical), RoutePolicy::Elevated);
            assert_eq!(route_policy(&method, lowercase), RoutePolicy::Elevated);
        }
    }

    #[test]
    fn emby_generic_ui_and_web_strings_use_official_runtime_policies() {
        for route in [
            "/emby/UI/View",
            "/emby/ui/view",
            "/emby/uI/vIeW",
            "/emby/UI/Command",
            "/emby/ui/command",
            "/emby/uI/cOmMaNd",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Elevated,
                "Generic UI route must remain administrator-only: {route}",
            );
        }

        for route in [
            "/emby/web/strings",
            "/emby/WeB/StRiNgS",
            "/emby/web/stringset",
            "/emby/WeB/StRiNgSeT",
        ] {
            assert_eq!(
                route_policy(&Method::GET, route),
                RoutePolicy::Public,
                "official WebApp request DTO is unauthenticated: {route}",
            );
        }

        for route in ["/UI/View", "/api/UI/View", "/UI/Command", "/api/UI/Command"] {
            assert_ne!(
                route_policy(&Method::GET, route),
                RoutePolicy::Elevated,
                "Emby-only Generic UI policy leaked into Jellyfin: {route}",
            );
        }
    }
}
