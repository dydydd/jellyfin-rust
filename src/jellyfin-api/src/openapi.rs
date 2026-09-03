use std::sync::Arc;

use aide::{
    axum::{ApiRouter, routing::get_with},
    openapi::{
        ApiKeyLocation, Components, Info, OpenApi, Operation, PathItem, ReferenceOr,
        Response as OpenApiResponse, Responses, SchemaObject, SecurityScheme,
        StatusCode as OpenApiStatusCode,
    },
    transform::TransformResponse,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Extension, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use indexmap::IndexMap;
use jellyfin_model::PublicSystemInfo;
use schemars::json_schema;
use serde_json::{Value, json};

use crate::AppState;

const CUSTOM_AUTHENTICATION: &str = "CustomAuthentication";
const OPENAPI_CONTENT_TYPE: &str = "application/json; charset=utf-8";

#[derive(Clone)]
struct OpenApiDocument(Bytes);

#[derive(schemars::JsonSchema)]
struct ApiErrorResponse {
    #[schemars(rename = "Message")]
    _message: String,
}

pub(crate) fn documented_routes() -> Router<Arc<AppState>> {
    aide::generate::infer_responses(false);

    let mut document = base_document();
    let router = ApiRouter::<Arc<AppState>>::new()
        .api_route(
            "/health",
            get_with(super::health, |operation| {
                operation
                    .id("GetHealth")
                    .summary("Gets server health.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server and database are healthy.")
                    })
                    .response_with::<503, String, _>(|response| {
                        plain_text_response(response, "The server or database is unavailable.")
                    })
            }),
        )
        .api_route(
            "/System/Info/Public",
            get_with(public_system_info, |operation| {
                operation
                    .id("GetPublicSystemInfo")
                    .summary("Gets public information about the server.")
                    .response_with::<200, Json<PublicSystemInfo>, _>(|response| {
                        response.description("Public server information was retrieved.")
                    })
                    .response_with::<500, Json<ApiErrorResponse>, _>(|response| {
                        response.description("Server configuration could not be read.")
                    })
            }),
        )
        .api_route(
            "/System/Ping",
            get_with(super::ping, |operation| {
                operation
                    .id("GetPingSystem")
                    .summary("Pings the system.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server name was retrieved.")
                    })
            })
            .post_with(super::ping, |operation| {
                operation
                    .id("PostPingSystem")
                    .summary("Pings the system.")
                    .response_with::<200, String, _>(|response| {
                        plain_text_response(response, "The server name was retrieved.")
                    })
            }),
        )
        .api_route(
            "/api-docs/openapi.json",
            get_with(serve_document, |operation| {
                operation
                    .id("GetOpenApiSpec")
                    .summary("Gets the OpenAPI specification.")
                    .response_with::<200, Json<Value>, _>(|response| {
                        openapi_document_response(response)
                    })
            }),
        )
        .finish_api(&mut document);

    add_route_inventory(&mut document);

    let document = OpenApiDocument(Bytes::from(
        serde_json::to_vec(&document).expect("the generated OpenAPI document must serialize"),
    ));

    router.layer(Extension(document))
}

/// Add every route exposed by [`crate::router`] to the document.  Most handlers
/// are not yet annotated with aide, so these inventory entries intentionally
/// describe only the operation and a generic success response; they do not
/// invent request or response schemas for endpoints that have no Rust schema.
fn add_route_inventory(document: &mut OpenApi) {
    let paths = document.paths.get_or_insert_with(Default::default);
    for &(path, methods) in ROUTE_METHODS {
        let path_item = paths
            .paths
            .entry(path.to_owned())
            .or_insert_with(|| ReferenceOr::Item(PathItem::default()));
        let Some(path_item) = path_item.as_item_mut() else {
            continue;
        };
        for &method in methods {
            let operation = Operation {
                operation_id: Some(inventory_operation_id(method, path)),
                responses: Some(Responses {
                    responses: std::iter::once((
                        OpenApiStatusCode::Code(200),
                        ReferenceOr::Item(OpenApiResponse {
                            description: "The request was handled by the Jellyfin API.".to_owned(),
                            ..OpenApiResponse::default()
                        }),
                    ))
                    .collect(),
                    ..Responses::default()
                }),
                ..Operation::default()
            };
            match method {
                "get" => path_item.get.get_or_insert(operation),
                "put" => path_item.put.get_or_insert(operation),
                "post" => path_item.post.get_or_insert(operation),
                "delete" => path_item.delete.get_or_insert(operation),
                "patch" => path_item.patch.get_or_insert(operation),
                "head" => path_item.head.get_or_insert(operation),
                "options" => path_item.options.get_or_insert(operation),
                _ => continue,
            };
        }
    }
}

fn inventory_operation_id(method: &str, path: &str) -> String {
    let mut id = String::with_capacity(method.len() + path.len());
    id.push_str(method);
    for part in path.split('/') {
        if part.is_empty() {
            continue;
        }
        id.push_str(&part.replace(['{', '}', '*', '.', '-'], "_"));
    }
    id
}

const ROUTE_METHODS: &[(&str, &[&str])] = &[
    ("/metrics", &["get"]),
    ("/websocket", &["get"]),
    ("/socket", &["get"]),
    ("/Branding/Configuration", &["get"]),
    ("/Branding/Css", &["get"]),
    ("/Branding/Css.css", &["get"]),
    ("/Branding/Splashscreen", &["get", "post", "delete"]),
    ("/Channels", &["get"]),
    ("/Channels/Features", &["get"]),
    ("/Channels/Items/Latest", &["get"]),
    ("/Channels/{channel_id}/Features", &["get"]),
    ("/Channels/{channel_id}/Items", &["get"]),
    (
        "/Artists/{name}/Images/{image_type}/{image_index}",
        &["get"],
    ),
    ("/Search/Hints", &["get"]),
    ("/Backup", &["get"]),
    ("/Backup/Create", &["post"]),
    ("/Backup/Manifest", &["get"]),
    ("/Backup/Restore", &["post"]),
    ("/Items/{item_id}/Images", &["get"]),
    (
        "/Items/{item_id}/Images/{image_type}",
        &["get", "post", "delete"],
    ),
    (
        "/Items/{item_id}/Images/{image_type}/{image_index}",
        &["get", "post", "delete"],
    ),
    (
        "/Items/{item_id}/Images/{image_type}/{image_index}/Index",
        &["post"],
    ),
    (
        "/Items/{item_id}/Images/{image_type}/{image_index}/{tag}/{format}/{max_width}/{max_height}/{percent_played}/{unplayed_count}",
        &["get"],
    ),
    ("/Items/{item_id}/RemoteImages", &["get"]),
    ("/Items/{item_id}/RemoteImages/Providers", &["get"]),
    ("/Items/{item_id}/RemoteImages/Download", &["post"]),
    ("/System/Configuration", &["get", "post"]),
    ("/System/Configuration/MetadataOptions/Default", &["get"]),
    ("/System/Configuration/Branding", &["post"]),
    ("/System/Configuration/{key}", &["get", "post"]),
    ("/web/ConfigurationPage", &["get"]),
    ("/web/ConfigurationPages", &["get"]),
    ("/Playback/BitrateTest", &["get"]),
    ("/Items/{item_id}/PlaybackInfo", &["get", "post"]),
    ("/LiveStreams/Open", &["post"]),
    ("/LiveStreams/Close", &["post"]),
    ("/MediaSegments/{item_id}", &["get"]),
    ("/FallbackFont/Fonts", &["get"]),
    ("/FallbackFont/Fonts/{name}", &["get"]),
    ("/Audio/{item_id}/hls/{*legacy_path}", &["get"]),
    ("/Audio/{item_id}/master.m3u8", &["get", "head"]),
    ("/Audio/{item_id}/main.m3u8", &["get"]),
    (
        "/Audio/{item_id}/hls1/{playlist_id}/{segment_file}",
        &["get"],
    ),
    ("/Audio/{item_id}/stream", &["get", "head"]),
    ("/Audio/{item_id}/stream.{container}", &["get", "head"]),
    ("/Audio/{item_id}/universal", &["get", "head"]),
    ("/Videos/{item_id}/hls/{*legacy_path}", &["get"]),
    ("/Videos/{item_id}/live.m3u8", &["get"]),
    ("/Videos/{item_id}/master.m3u8", &["get", "head"]),
    ("/Videos/{item_id}/main.m3u8", &["get"]),
    (
        "/Videos/{item_id}/hls1/{playlist_id}/{segment_file}",
        &["get"],
    ),
    ("/Videos/ActiveEncodings", &["delete"]),
    ("/Videos/{item_id}/stream", &["get", "head"]),
    ("/Videos/{item_id}/stream.{container}", &["get", "head"]),
    ("/Plugins", &["get"]),
    ("/Plugins/{plugin_id}/{version}/Enable", &["post"]),
    ("/Plugins/{plugin_id}/{version}/Disable", &["post"]),
    ("/Plugins/{plugin_id}/{version}", &["delete"]),
    ("/Plugins/{plugin_id}", &["delete"]),
    ("/Plugins/{plugin_id}/Configuration", &["get", "post"]),
    ("/Plugins/{plugin_id}/Manifest", &["post"]),
    ("/Plugins/{plugin_id}/{version}/Image", &["get"]),
    ("/Users/{user_id}/Items/Root", &["get"]),
    ("/Users/{user_id}/Items/{item_id}", &["get"]),
    ("/Users/{user_id}/Items/{item_id}/Intros", &["get"]),
    ("/Users/{user_id}/Items/{item_id}/LocalTrailers", &["get"]),
    ("/Users/{user_id}/Items/{item_id}/SpecialFeatures", &["get"]),
    ("/Users/{user_id}/Items/{item_id}/Lyrics", &["get"]),
    ("/Items/Filters", &["get"]),
    ("/Items/Filters2", &["get"]),
    ("/Artists", &["get"]),
    ("/Artists/AlbumArtists", &["get"]),
    ("/Artists/{name}", &["get"]),
    ("/Years", &["get"]),
    ("/Years/{year}", &["get"]),
    ("/Genres", &["get"]),
    ("/Genres/{genre_name}", &["get"]),
    ("/Genres/{name}/Images/{image_type}", &["get"]),
    ("/Genres/{name}/Images/{image_type}/{image_index}", &["get"]),
    ("/Studios", &["get"]),
    ("/Studios/{name}", &["get"]),
    ("/Studios/{name}/Images/{image_type}", &["get"]),
    (
        "/Studios/{name}/Images/{image_type}/{image_index}",
        &["get"],
    ),
    ("/Trailers", &["get"]),
    ("/MusicGenres", &["get"]),
    ("/MusicGenres/{genre_name}", &["get"]),
    ("/MusicGenres/{name}/Images/{image_type}", &["get"]),
    (
        "/MusicGenres/{name}/Images/{image_type}/{image_index}",
        &["get"],
    ),
    ("/Persons", &["get"]),
    ("/Persons/{name}", &["get"]),
    ("/Persons/{name}/Images/{image_type}", &["get"]),
    (
        "/Persons/{name}/Images/{image_type}/{image_index}",
        &["get"],
    ),
    ("/Library/VirtualFolders", &["get", "post", "delete"]),
    ("/Library/VirtualFolders/Name", &["post"]),
    ("/Library/VirtualFolders/Paths", &["post", "delete"]),
    ("/Library/VirtualFolders/Paths/Update", &["post"]),
    ("/Library/VirtualFolders/LibraryOptions", &["post"]),
    ("/System/ActivityLog/Entries", &["get"]),
    ("/System/Logs", &["get"]),
    ("/System/Logs/Log", &["get"]),
    ("/System/Info", &["get"]),
    ("/System/Info/Storage", &["get"]),
    ("/System/Endpoint", &["get"]),
    ("/System/Restart", &["post"]),
    ("/System/Shutdown", &["post"]),
    ("/Document", &["post"]),
    ("/ClientLog/Document", &["post"]),
    ("/GetUtcTime", &["get"]),
    ("/ScheduledTasks", &["get"]),
    ("/ScheduledTasks/Running/{task_id}", &["post", "delete"]),
    ("/ScheduledTasks/{task_id}/Triggers", &["post"]),
    ("/ScheduledTasks/{task_id}", &["get"]),
    ("/SyncPlay/New", &["post"]),
    ("/SyncPlay/Join", &["post"]),
    ("/SyncPlay/Leave", &["post"]),
    ("/SyncPlay/List", &["get"]),
    ("/SyncPlay/SetNewQueue", &["post"]),
    ("/SyncPlay/SetPlaylistItem", &["post"]),
    ("/SyncPlay/RemoveFromPlaylist", &["post"]),
    ("/SyncPlay/MovePlaylistItem", &["post"]),
    ("/SyncPlay/Queue", &["post"]),
    ("/SyncPlay/Unpause", &["post"]),
    ("/SyncPlay/Pause", &["post"]),
    ("/SyncPlay/Stop", &["post"]),
    ("/SyncPlay/Seek", &["post"]),
    ("/SyncPlay/Buffering", &["post"]),
    ("/SyncPlay/Ready", &["post"]),
    ("/SyncPlay/SetIgnoreWait", &["post"]),
    ("/SyncPlay/NextItem", &["post"]),
    ("/SyncPlay/PreviousItem", &["post"]),
    ("/SyncPlay/SetRepeatMode", &["post"]),
    ("/SyncPlay/SetShuffleMode", &["post"]),
    ("/SyncPlay/Ping", &["post"]),
    ("/SyncPlay/{id}", &["get"]),
    ("/Environment/DirectoryContents", &["get"]),
    ("/Environment/ValidatePath", &["post"]),
    ("/Environment/Drives", &["get"]),
    ("/Environment/ParentPath", &["get"]),
    ("/Environment/DefaultDirectoryBrowser", &["get"]),
    ("/Localization/Cultures", &["get"]),
    ("/Localization/Countries", &["get"]),
    ("/Localization/ParentalRatings", &["get"]),
    ("/Localization/Options", &["get"]),
    ("/Auth/Keys", &["get", "post"]),
    ("/Auth/Keys/{key}", &["delete"]),
    ("/Packages", &["get"]),
    ("/Packages/Installed/{name}", &["post"]),
    ("/Packages/Installing/{package_id}", &["delete"]),
    ("/Packages/{name}", &["get"]),
    ("/Repositories", &["get", "post"]),
    ("/Startup/Configuration", &["get", "post"]),
    ("/Startup/RemoteAccess", &["post"]),
    ("/Startup/User", &["get", "post"]),
    ("/Startup/FirstUser", &["get"]),
    ("/Startup/Complete", &["post"]),
    ("/Users/AuthenticateByName", &["post"]),
    ("/Users/authenticatebyname", &["post"]),
    ("/Users/AuthenticateWithQuickConnect", &["post"]),
    ("/Users/{user_id}/Authenticate", &["post"]),
    ("/Users/Me", &["get"]),
    ("/QuickConnect/Enabled", &["get"]),
    ("/QuickConnect/Initiate", &["post"]),
    ("/QuickConnect/Connect", &["get"]),
    ("/QuickConnect/Authorize", &["post"]),
    ("/Devices", &["get", "delete"]),
    ("/Devices/Info", &["get"]),
    ("/Devices/Options", &["get", "post"]),
    (
        "/DisplayPreferences/{display_preferences_id}",
        &["get", "post"],
    ),
    ("/UserImage", &["get", "post", "delete"]),
    ("/Users", &["get", "post"]),
    ("/Users/Public", &["get"]),
    ("/Users/New", &["post"]),
    ("/Users/ForgotPassword", &["post"]),
    ("/Users/ForgotPassword/Pin", &["post"]),
    ("/Users/Configuration", &["post"]),
    ("/Users/{id}", &["get", "post", "delete"]),
    ("/User/{id}", &["delete"]),
    ("/Users/Password", &["post"]),
    ("/Users/{id}/Configuration", &["post"]),
    (
        "/Users/{id}/Images/{image_type}",
        &["get", "post", "delete"],
    ),
    (
        "/Users/{id}/Images/{image_type}/{index}",
        &["get", "post", "delete"],
    ),
    ("/Users/{id}/Password", &["post"]),
    ("/Users/{id}/Policy", &["post"]),
    ("/UserViews", &["get"]),
    ("/UserViews/GroupingOptions", &["get"]),
    ("/Users/{user_id}/Views", &["get"]),
    ("/Users/{user_id}/GroupingOptions", &["get"]),
    ("/Sessions", &["get"]),
    ("/Sessions/{session_id}/System/{command}", &["post"]),
    ("/Sessions/{session_id}/Viewing", &["post"]),
    ("/Sessions/{session_id}/Playing", &["post"]),
    ("/Sessions/{session_id}/Playing/{command}", &["post"]),
    ("/Sessions/{session_id}/Command/{command}", &["post"]),
    ("/Sessions/{session_id}/Command", &["post"]),
    ("/Sessions/{session_id}/Message", &["post"]),
    ("/Sessions/{session_id}/User/{user_id}", &["post", "delete"]),
    ("/Sessions/Viewing", &["post"]),
    ("/Sessions/Capabilities", &["post"]),
    ("/Sessions/Capabilities/Full", &["post"]),
    ("/Sessions/Logout", &["post"]),
    ("/Auth/Providers", &["get"]),
    ("/Auth/PasswordResetProviders", &["get"]),
    ("/Sessions/Playing", &["post"]),
    ("/Sessions/Playing/Progress", &["post"]),
    ("/Sessions/Playing/Ping", &["post"]),
    ("/Sessions/Playing/Stopped", &["post"]),
    ("/PlayingItems/{item_id}", &["post", "delete"]),
    ("/PlayingItems/{item_id}/Progress", &["post"]),
    (
        "/Users/{user_id}/PlayingItems/{item_id}",
        &["post", "delete"],
    ),
    (
        "/Users/{user_id}/PlayingItems/{item_id}/Progress",
        &["post"],
    ),
    ("/UserPlayedItems/{item_id}", &["post", "delete"]),
    (
        "/Users/{user_id}/PlayedItems/{item_id}",
        &["post", "delete"],
    ),
    ("/UserItems/{item_id}/UserData", &["get", "post"]),
    (
        "/Users/{user_id}/Items/{item_id}/UserData",
        &["get", "post"],
    ),
    ("/UserFavoriteItems/{item_id}", &["post", "delete"]),
    (
        "/Users/{user_id}/FavoriteItems/{item_id}",
        &["post", "delete"],
    ),
    ("/UserItems/{item_id}/Rating", &["post", "delete"]),
    (
        "/Users/{user_id}/Items/{item_id}/Rating",
        &["post", "delete"],
    ),
    ("/Items", &["get", "delete"]),
    ("/Items/Suggestions", &["get"]),
    ("/Items/Latest", &["get"]),
    ("/UserItems/Resume", &["get"]),
    ("/Users/{user_id}/Items", &["get"]),
    ("/Users/{user_id}/Suggestions", &["get"]),
    ("/Users/{user_id}/Items/Latest", &["get"]),
    ("/Users/{user_id}/Items/Resume", &["get"]),
    ("/Collections", &["post"]),
    ("/Collections/{collection_id}/Items", &["post", "delete"]),
    ("/Playlists", &["post"]),
    ("/Playlists/{playlist_id}", &["get", "post"]),
    ("/Playlists/{playlist_id}/Users", &["get"]),
    (
        "/Playlists/{playlist_id}/Users/{user_id}",
        &["get", "post", "delete"],
    ),
    ("/Playlists/{playlist_id}/Items", &["get", "post", "delete"]),
    (
        "/Playlists/{playlist_id}/Items/{item_id}/Move/{new_index}",
        &["post"],
    ),
    ("/Songs/{item_id}/InstantMix", &["get"]),
    ("/Albums/{item_id}/InstantMix", &["get"]),
    ("/Playlists/{item_id}/InstantMix", &["get"]),
    ("/Artists/{item_id}/InstantMix", &["get"]),
    ("/Items/{item_id}/InstantMix", &["get"]),
    ("/MusicGenres/InstantMix", &["get"]),
    ("/Artists/InstantMix", &["get"]),
    ("/MusicGenres/{name}/InstantMix", &["get"]),
    ("/Items/Counts", &["get"]),
    ("/Items/{item_id}/File", &["get"]),
    ("/Items/{item_id}/ThemeSongs", &["get"]),
    ("/Items/{item_id}/ThemeVideos", &["get"]),
    ("/Items/{item_id}/ThemeMedia", &["get"]),
    ("/Items/{item_id}/Ancestors", &["get"]),
    ("/Items/{item_id}/Download", &["get"]),
    ("/Items/{item_id}/Collections", &["get"]),
    ("/Library/Refresh", &["post"]),
    ("/Library/PhysicalPaths", &["get"]),
    ("/Library/MediaFolders", &["get"]),
    ("/Library/Series/Added", &["post"]),
    ("/Library/Series/Updated", &["post"]),
    ("/Library/Movies/Added", &["post"]),
    ("/Library/Movies/Updated", &["post"]),
    ("/Library/Media/Updated", &["post"]),
    ("/Libraries/AvailableOptions", &["get"]),
    ("/Artists/{item_id}/Similar", &["get"]),
    ("/Items/{item_id}/Similar", &["get"]),
    ("/Albums/{item_id}/Similar", &["get"]),
    ("/Shows/{item_id}/Similar", &["get"]),
    ("/Movies/Recommendations", &["get"]),
    ("/Movies/{item_id}/Similar", &["get"]),
    ("/Shows/NextUp", &["get"]),
    ("/Shows/Upcoming", &["get"]),
    ("/Shows/{series_id}/Episodes", &["get"]),
    ("/Shows/{series_id}/Seasons", &["get"]),
    ("/Trailers/{item_id}/Similar", &["get"]),
    ("/Items/Root", &["get"]),
    ("/Items/{item_id}", &["get", "post", "delete"]),
    ("/Items/{item_id}/ContentType", &["post"]),
    ("/Items/{item_id}/Refresh", &["post"]),
    ("/Items/{item_id}/MetadataEditor", &["get"]),
    ("/Items/{item_id}/ExternalIdInfos", &["get"]),
    ("/Items/RemoteSearch/Movie", &["post"]),
    ("/Items/RemoteSearch/Trailer", &["post"]),
    ("/Items/RemoteSearch/MusicVideo", &["post"]),
    ("/Items/RemoteSearch/Series", &["post"]),
    ("/Items/RemoteSearch/BoxSet", &["post"]),
    ("/Items/RemoteSearch/MusicArtist", &["post"]),
    ("/Items/RemoteSearch/MusicAlbum", &["post"]),
    ("/Items/RemoteSearch/Person", &["post"]),
    ("/Items/RemoteSearch/Book", &["post"]),
    ("/Items/RemoteSearch/Apply/{item_id}", &["post"]),
    ("/Items/{item_id}/Intros", &["get"]),
    ("/Items/{item_id}/LocalTrailers", &["get"]),
    ("/Items/{item_id}/SpecialFeatures", &["get"]),
    ("/Audio/{item_id}/RemoteSearch/Lyrics", &["get"]),
    (
        "/Items/{item_id}/RemoteSearch/Subtitles/{id}",
        &["get", "post"],
    ),
    ("/Audio/{item_id}/RemoteSearch/Lyrics/{lyric_id}", &["post"]),
    ("/Audio/{item_id}/Lyrics", &["get", "post", "delete"]),
    ("/Providers/Lyrics/{lyric_id}", &["get"]),
    ("/Providers/Subtitles/Subtitles/{subtitle_id}", &["get"]),
    ("/Videos/MergeVersions", &["post"]),
    ("/Videos/{item_id}/AlternateSources", &["delete"]),
    ("/Videos/{item_id}/AdditionalParts", &["get"]),
    ("/Videos/{item_id}/Subtitles/{index}", &["delete"]),
    ("/Videos/{item_id}/Subtitles", &["post"]),
    (
        "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/Stream.{format}",
        &["get"],
    ),
    (
        "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/{start_position_ticks}/Stream.{format}",
        &["get"],
    ),
    (
        "/Videos/{item_id}/{media_source_id}/Subtitles/{index}/subtitles.m3u8",
        &["get"],
    ),
    (
        "/Videos/{item_id}/{media_source_id}/Attachments/{index}",
        &["get"],
    ),
    ("/Videos/{item_id}/Trickplay/{width}/tiles.m3u8", &["get"]),
    ("/Videos/{item_id}/Trickplay/{width}/{*tile}", &["get"]),
    ("/LiveTv/Info", &["get"]),
    ("/LiveTv/Channels", &["get"]),
    ("/LiveTv/Channels/{channel_id}", &["get"]),
    ("/LiveTv/Recordings", &["get"]),
    ("/LiveTv/Recordings/Series", &["get"]),
    ("/LiveTv/Recordings/Groups", &["get"]),
    ("/LiveTv/Recordings/Folders", &["get"]),
    ("/LiveTv/Recordings/{recording_id}", &["get", "delete"]),
    ("/LiveTv/Tuners/{tuner_id}/Reset", &["post"]),
    ("/LiveTv/Timers", &["get", "post"]),
    ("/LiveTv/Timers/Defaults", &["get"]),
    ("/LiveTv/Timers/{timer_id}", &["get", "post", "delete"]),
    ("/LiveTv/Programs", &["get", "post"]),
    ("/LiveTv/Programs/Recommended", &["get"]),
    ("/LiveTv/Programs/{program_id}", &["get"]),
    ("/LiveTv/SeriesTimers", &["get", "post"]),
    (
        "/LiveTv/SeriesTimers/{timer_id}",
        &["get", "post", "delete"],
    ),
    ("/LiveTv/ListingProviders/Default", &["get"]),
    ("/LiveTv/ListingProviders", &["post", "delete"]),
    (
        "/LiveTv/ListingProviders/SchedulesDirect/Countries",
        &["get"],
    ),
    ("/LiveTv/ChannelMappingOptions", &["get"]),
    ("/LiveTv/ChannelMappings", &["post"]),
    ("/LiveTv/TunerHosts/Types", &["get"]),
    ("/LiveTv/Tuners/Discover", &["get"]),
    ("/LiveTv/Tuners/Discvover", &["get"]),
    ("/LiveTv/LiveRecordings/{recording_id}/stream", &["get"]),
    (
        "/LiveTv/LiveStreamFiles/{stream_id}/stream.{container}",
        &["get"],
    ),
    ("/LiveTv/TunerHosts", &["post", "delete"]),
    (
        "/LiveTv/ListingProviders/SchedulesDirect/Refresh",
        &["post"],
    ),
    ("/LiveTv/ListingProviders/Lineups", &["get"]),
    ("/LiveTv/GuideInfo", &["get"]),
];

fn base_document() -> OpenApi {
    let version = env!("CARGO_PKG_VERSION").to_owned();
    let mut info = Info {
        title: "Jellyfin API".to_owned(),
        version: version.clone(),
        ..Info::default()
    };
    info.extensions
        .insert("x-jellyfin-version".to_owned(), json!(version));

    let mut components = Components::default();
    components.security_schemes.insert(
        CUSTOM_AUTHENTICATION.to_owned(),
        ReferenceOr::Item(SecurityScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "Authorization".to_owned(),
            description: Some("API key header parameter".to_owned()),
            extensions: IndexMap::default(),
        }),
    );

    let mut document = OpenApi {
        info,
        components: Some(components),
        ..OpenApi::default()
    };
    document
        .extensions
        .insert("x-jellyfin-partial".to_owned(), json!(true));
    document
}

async fn public_system_info(state: State<Arc<AppState>>) -> Response {
    super::public_system_info(state).await.into_response()
}

async fn serve_document(Extension(document): Extension<OpenApiDocument>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static(OPENAPI_CONTENT_TYPE),
        )
        .body(Body::from(document.0))
        .expect("the static OpenAPI response must be valid")
}

fn plain_text_response<'response>(
    mut response: TransformResponse<'response, String>,
    description: &str,
) -> TransformResponse<'response, String> {
    response
        .inner()
        .content
        .get_mut("text/plain; charset=utf-8")
        .expect("Aide String responses must contain plain text")
        .schema = Some(SchemaObject {
        json_schema: json_schema!({ "type": "string" }),
        external_docs: None,
        example: None,
    });
    response.description(description)
}

fn openapi_document_response(
    mut response: TransformResponse<'_, Value>,
) -> TransformResponse<'_, Value> {
    response
        .inner()
        .content
        .get_mut("application/json")
        .expect("Aide JSON responses must contain application/json")
        .schema = Some(SchemaObject {
        json_schema: json_schema!({ "type": "object" }),
        external_docs: None,
        example: None,
    });
    response.description("The OpenAPI 3.1 document was retrieved.")
}
