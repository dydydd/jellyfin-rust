use std::{path::Path as FilePath, sync::Arc};

use axum::{
    Json,
    body::Body,
    extract::{
        OriginalUri, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::Response,
};
use jellyfin_controller::RelatedItemKind;
use jellyfin_data::{BaseItemCounts, BaseItemPage};
use jellyfin_model::{
    CollectionType, ImageOption, ImageType, ItemCounts, LibraryOptionsResultDto,
    LibraryTypeOptionsDto,
};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization, user_library};

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct LibraryQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "startIndex", alias = "StartIndex")]
    start_index: u64,
    limit: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstantMixByIdQuery {
    id: Option<Uuid>,
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    limit: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemCountsQuery {
    #[serde(default, rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, rename = "isFavorite", alias = "IsFavorite")]
    is_favorite: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct MediaFoldersQuery {
    #[serde(rename = "isHidden", alias = "IsHidden")]
    is_hidden: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct AvailableOptionsQuery {
    #[serde(default, rename = "libraryContentType", alias = "LibraryContentType")]
    library_content_type: Option<CollectionType>,
    #[serde(default, rename = "isNewLibrary", alias = "IsNewLibrary")]
    is_new_library: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeleteItemsQuery {
    ids: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdatedSeriesQuery {
    #[serde(default, rename = "tvdbId", alias = "TvdbId")]
    tvdb_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdatedMoviesQuery {
    #[serde(default, rename = "tmdbId", alias = "TmdbId")]
    tmdb_id: Option<String>,
    #[serde(default, rename = "imdbId", alias = "ImdbId")]
    imdb_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct MediaUpdateInfoDto {
    updates: Vec<MediaUpdateInfoPathDto>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub(crate) struct MediaUpdateInfoPathDto {
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ThemeMediaResult {
    items: Vec<user_library::BaseItemDto>,
    total_record_count: usize,
    owner_id: Uuid,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllThemeMediaResult {
    #[serde(rename = "ThemeSongsResult")]
    theme_songs: Arc<ThemeMediaResult>,
    #[serde(rename = "ThemeVideosResult")]
    theme_videos: ThemeMediaResult,
    #[serde(rename = "SoundtrackSongsResult")]
    soundtrack_songs: Arc<ThemeMediaResult>,
}

pub(crate) async fn file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    file_response(state, headers, item_id, false).await
}

pub(crate) async fn download(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    file_response(state, headers, item_id, true).await
}

pub(crate) async fn theme_songs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<ThemeMediaResult>, ApiError> {
    Ok(Json(
        theme_result(
            &state,
            &headers,
            item_id,
            query.user_id,
            RelatedItemKind::ThemeSong,
        )
        .await?,
    ))
}

pub(crate) async fn theme_videos(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<ThemeMediaResult>, ApiError> {
    Ok(Json(
        theme_result(
            &state,
            &headers,
            item_id,
            query.user_id,
            RelatedItemKind::ThemeVideo,
        )
        .await?,
    ))
}

pub(crate) async fn theme_media(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<AllThemeMediaResult>, ApiError> {
    let theme_songs = Arc::new(
        theme_result(
            &state,
            &headers,
            item_id,
            query.user_id,
            RelatedItemKind::ThemeSong,
        )
        .await?,
    );
    let theme_videos = theme_result(
        &state,
        &headers,
        item_id,
        query.user_id,
        RelatedItemKind::ThemeVideo,
    )
    .await?;
    Ok(Json(AllThemeMediaResult {
        // ALLOW: both response properties intentionally expose the same read-only result.
        theme_songs: Arc::clone(&theme_songs),
        theme_videos,
        soundtrack_songs: theme_songs,
    }))
}

pub(crate) async fn ancestors(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<Vec<user_library::BaseItemDto>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let items = state
        .library_controller
        .ancestors(&authenticated.user, target_user_id, item_id)
        .await?;
    Ok(Json(
        items
            .into_iter()
            .map(|item| user_library::item_to_dto(item, state.server_id()))
            .collect(),
    ))
}

pub(crate) async fn collections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let page = state
        .library_controller
        .collections_containing_item(
            &authenticated.user,
            target_user_id,
            item_id,
            query.start_index,
            query.limit,
        )
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn similar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let page = state
        .library_controller
        .similar_items(&authenticated.user, target_user_id, item_id, query.limit)
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn instant_mix(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let page = state
        .library_controller
        .instant_mix(&authenticated.user, target_user_id, item_id, query.limit)
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn instant_mix_genre_by_id(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<InstantMixByIdQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let genre_id = query.id.ok_or(ApiError::InvalidRequest)?;
    let page = state
        .library_controller
        .instant_mix_for_genre(&authenticated.user, target_user_id, genre_id, query.limit)
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn instant_mix_by_id(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<InstantMixByIdQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let item_id = query.id.ok_or(ApiError::InvalidRequest)?;
    let page = state
        .library_controller
        .instant_mix(&authenticated.user, target_user_id, item_id, query.limit)
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn instant_mix_genre_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(query): Query<LibraryQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
    let page = state
        .library_controller
        .instant_mix_for_genre_name(&authenticated.user, target_user_id, &name, query.limit)
        .await?;
    Ok(Json(page_to_dto(page, state.server_id())))
}

pub(crate) async fn item_counts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ItemCountsQuery>,
) -> Result<Json<ItemCounts>, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, None).await?;
    let target_user_id = identity.target_user_id(query.user_id)?;
    let user_id = if target_user_id.is_nil() {
        None
    } else {
        state.users.get(target_user_id).await?;
        Some(target_user_id)
    };
    Ok(Json(counts_to_dto(
        state
            .library_controller
            .item_counts(user_id, query.is_favorite)
            .await?,
    )))
}

pub(crate) async fn media_folders(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<MediaFoldersQuery>, QueryRejection>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let items = state
        .virtual_folders
        .list()
        .await?
        .into_iter()
        .filter(is_enabled_media_folder)
        .filter(|folder| {
            query
                .is_hidden
                .is_none_or(|hidden| is_hidden(folder) == hidden)
        })
        .map(|folder| crate::user_views::view_to_dto(folder, state.server_id()))
        .collect::<Vec<_>>();
    Ok(Json(user_library::BaseItemQueryResult {
        total_record_count: items.len(),
        start_index: 0,
        items,
    }))
}

pub(crate) async fn physical_paths(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(
        state
            .virtual_folders
            .list()
            .await?
            .into_iter()
            .flat_map(|folder| folder.locations)
            .collect(),
    ))
}

pub(crate) async fn available_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<AvailableOptionsQuery>, QueryRejection>,
) -> Result<Json<LibraryOptionsResultDto>, ApiError> {
    authorization::require_first_time_setup_or_elevated(&state, &headers, &uri).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let is_new_library = query.is_new_library;
    let metadata_savers = ["Nfo"]
        .into_iter()
        .map(|name| jellyfin_model::LibraryOptionInfoDto {
            name: Some(name.to_owned()),
            default_enabled: !is_new_library,
        })
        .collect();
    let metadata_readers = ["Nfo"].into_iter().map(option_info).collect();
    let subtitle_fetchers: Vec<jellyfin_model::LibraryOptionInfoDto> = Vec::new();
    let lyric_fetchers: Vec<jellyfin_model::LibraryOptionInfoDto> = Vec::new();
    let media_segment_providers = Vec::new();
    Ok(Json(LibraryOptionsResultDto {
        metadata_savers,
        metadata_readers,
        subtitle_fetchers,
        lyric_fetchers,
        media_segment_providers,
        type_options: representative_item_types(query.library_content_type)
            .into_iter()
            .map(|item_type| LibraryTypeOptionsDto {
                item_type: Some(item_type.to_owned()),
                default_image_options: default_image_options(item_type),
                metadata_fetchers: metadata_fetchers(item_type, is_new_library),
                image_fetchers: image_fetchers(item_type, is_new_library),
                similar_item_providers: similar_item_providers(item_type),
                ..LibraryTypeOptionsDto::default()
            })
            .collect(),
        ..LibraryOptionsResultDto::default()
    }))
}

pub(crate) async fn refresh(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let summary = state.library_scan.scan_all().await?;
    crate::websocket::broadcast_library_changed(
        &state,
        &summary.added_ids,
        &summary.removed_ids,
        &summary.changed_ids,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn updated_series(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<UpdatedSeriesQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let _ = query.tvdb_id.as_deref();
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn updated_movies(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    query: Result<Query<UpdatedMoviesQuery>, QueryRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Query(query) = query.map_err(|_| ApiError::InvalidRequest)?;
    let _ = (query.imdb_id.as_deref(), query.tmdb_id.as_deref());
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn updated_media(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<MediaUpdateInfoDto>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    let Json(request) = request.map_err(|_| ApiError::InvalidRequest)?;
    if request.updates.iter().any(|update| update.path.is_none()) {
        return Err(ApiError::InvalidRequest);
    }
    // The official endpoint reports each path to the library monitor.  This
    // implementation has no asynchronous file-system monitor yet, so trigger
    // an equivalent library scan immediately rather than returning a no-op.
    if !request.updates.is_empty() {
        let summary = state.library_scan.scan_all().await?;
        crate::websocket::broadcast_library_changed(
            &state,
            &summary.added_ids,
            &summary.removed_ids,
            &summary.changed_ids,
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn option_info(name: &str) -> jellyfin_model::LibraryOptionInfoDto {
    jellyfin_model::LibraryOptionInfoDto {
        name: Some(name.to_owned()),
        default_enabled: true,
    }
}

fn metadata_fetchers(item_type: &str, is_new: bool) -> Vec<jellyfin_model::LibraryOptionInfoDto> {
    let names: &[&str] = match item_type {
        "Movie" | "Series" | "Season" | "Episode" => &["TheMovieDb", "TheTVDB", "OMDb"],
        "MusicArtist" | "MusicAlbum" | "Audio" | "MusicVideo" => &["TheAudioDB", "MusicBrainz"],
        "Book" | "AudioBook" => &["GoogleBooks"],
        _ => &[],
    };
    names
        .iter()
        .map(|name| jellyfin_model::LibraryOptionInfoDto {
            name: Some((*name).to_owned()),
            // New libraries enable the canonical provider for each media
            // family by default; optional fallbacks remain opt-in. Existing
            // libraries preserve the historical enabled default.
            default_enabled: !is_new
                || match item_type {
                    "Movie" | "Series" | "Season" | "Episode" => {
                        matches!(*name, "TheMovieDb" | "TheTVDB")
                    }
                    "MusicArtist" | "MusicAlbum" | "Audio" | "MusicVideo" => {
                        matches!(*name, "TheAudioDB" | "MusicBrainz")
                    }
                    "Book" | "AudioBook" => *name == "GoogleBooks",
                    _ => false,
                },
        })
        .collect()
}

fn image_fetchers(item_type: &str, is_new: bool) -> Vec<jellyfin_model::LibraryOptionInfoDto> {
    if matches!(
        item_type,
        "Movie" | "Series" | "Season" | "Episode" | "MusicArtist" | "MusicAlbum"
    ) {
        {
            let mut providers = vec![jellyfin_model::LibraryOptionInfoDto {
                name: Some("TheMovieDb".to_owned()),
                default_enabled: true,
            }];
            if matches!(item_type, "Series" | "Season" | "Episode") {
                providers.push(jellyfin_model::LibraryOptionInfoDto {
                    name: Some("TheTVDB".to_owned()),
                    default_enabled: true,
                });
            }
            if matches!(item_type, "MusicArtist" | "MusicAlbum") {
                providers.extend([
                    jellyfin_model::LibraryOptionInfoDto {
                        name: Some("TheAudioDB".to_owned()),
                        default_enabled: true,
                    },
                    jellyfin_model::LibraryOptionInfoDto {
                        name: Some("Image Extractor".to_owned()),
                        default_enabled: !is_new,
                    },
                ]);
            }
            providers
        }
    } else {
        Vec::new()
    }
}

fn similar_item_providers(item_type: &str) -> Vec<jellyfin_model::LibraryOptionInfoDto> {
    if matches!(item_type, "Movie" | "Series" | "MusicAlbum") {
        vec![option_info("TheMovieDb")]
    } else {
        Vec::new()
    }
}

pub(crate) async fn delete_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    delete_for(state, headers, vec![item_id]).await
}

pub(crate) async fn delete_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeleteItemsQuery>,
) -> Result<StatusCode, ApiError> {
    let ids = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.parse().map_err(|_| ApiError::InvalidRequest))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    delete_for(state, headers, ids).await
}

async fn theme_result(
    state: &AppState,
    headers: &HeaderMap,
    item_id: Uuid,
    target_user_id_hint: Option<Uuid>,
    kind: RelatedItemKind,
) -> Result<ThemeMediaResult, ApiError> {
    let authenticated = authentication::authenticated_session(state, headers).await?;
    let target_user_id = target_user_id_hint.unwrap_or(authenticated.user.id);
    let _owner = state
        .library_controller
        .item(&authenticated.user, target_user_id, item_id)
        .await?;
    let items = state
        .user_library
        .related_items(&authenticated.user, target_user_id, item_id, kind)
        .await?
        .into_iter()
        .map(|item| user_library::item_to_dto(item, state.server_id()))
        .collect::<Vec<_>>();
    let total_record_count = items.len();
    Ok(ThemeMediaResult {
        items,
        total_record_count,
        owner_id: item_id,
    })
}

async fn file_response(
    state: Arc<AppState>,
    mut headers: HeaderMap,
    item_id: Uuid,
    attachment: bool,
) -> Result<Response, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let path = state
        .library_controller
        .download_path(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    let mut file_request = Request::builder()
        .method("GET")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    for name in [header::RANGE, header::IF_RANGE] {
        if let Some(value) = headers.remove(&name) {
            file_request.headers_mut().insert(name, value);
        }
    }
    let response = match ServeFile::new(&path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(file_request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let mut response = response.map(Body::new);
    if attachment && response.status().is_success() {
        let filename = safe_filename(&path);
        let value = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| ApiError::Internal)?;
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

async fn delete_for(
    state: Arc<AppState>,
    headers: HeaderMap,
    ids: Vec<Uuid>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .library_controller
        .delete_items(&authenticated.user, &ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn page_to_dto(page: BaseItemPage, server_id: &str) -> user_library::BaseItemQueryResult {
    user_library::BaseItemQueryResult {
        items: page
            .items
            .into_iter()
            .map(|item| user_library::item_to_dto(item, server_id))
            .collect(),
        total_record_count: usize::try_from(page.total_record_count).unwrap_or(usize::MAX),
        start_index: usize::try_from(page.start_index).unwrap_or(usize::MAX),
    }
}

fn counts_to_dto(counts: BaseItemCounts) -> ItemCounts {
    ItemCounts {
        movie_count: saturating_i32(counts.movie_count),
        series_count: saturating_i32(counts.series_count),
        episode_count: saturating_i32(counts.episode_count),
        artist_count: saturating_i32(counts.artist_count),
        program_count: saturating_i32(counts.program_count),
        trailer_count: saturating_i32(counts.trailer_count),
        song_count: saturating_i32(counts.song_count),
        album_count: saturating_i32(counts.album_count),
        music_video_count: saturating_i32(counts.music_video_count),
        box_set_count: saturating_i32(counts.box_set_count),
        book_count: saturating_i32(counts.book_count),
        item_count: saturating_i32(counts.item_count),
    }
}

fn is_enabled_media_folder(folder: &jellyfin_controller::VirtualFolder) -> bool {
    crate::user_views::bool_option(&folder.library_options, &["Enabled", "enabled"]).unwrap_or(true)
}

fn is_hidden(folder: &jellyfin_controller::VirtualFolder) -> bool {
    crate::user_views::bool_option(
        &folder.library_options,
        &["IsHidden", "isHidden", "Hidden", "hidden"],
    )
    .unwrap_or(false)
}

fn representative_item_types(content_type: Option<CollectionType>) -> Vec<&'static str> {
    match content_type {
        Some(CollectionType::BoxSets) => vec!["BoxSet"],
        Some(CollectionType::Playlists) => vec!["Playlist"],
        Some(CollectionType::Movies) => vec!["Movie"],
        Some(CollectionType::TvShows) => vec!["Series", "Season", "Episode"],
        Some(CollectionType::Books) => vec!["Book", "AudioBook"],
        Some(CollectionType::Music) => {
            vec!["MusicArtist", "MusicAlbum", "Audio", "MusicVideo"]
        }
        Some(CollectionType::HomeVideos | CollectionType::Photos) => vec!["Video", "Photo"],
        Some(CollectionType::MusicVideos) => vec!["MusicVideo"],
        Some(
            CollectionType::Unknown
            | CollectionType::Trailers
            | CollectionType::LiveTv
            | CollectionType::Folders,
        )
        | None => vec!["Series", "Season", "Episode", "Movie"],
    }
}

fn default_image_options(item_type: &str) -> Vec<ImageOption> {
    match item_type {
        "Movie" | "MusicVideo" => vec![
            image_option(ImageType::Backdrop, 1, 1280),
            image_option(ImageType::Art, 0, 0),
            image_option(ImageType::Disc, 0, 0),
            image_option(ImageType::Primary, 1, 0),
            image_option(ImageType::Banner, 0, 0),
            image_option(ImageType::Thumb, 1, 0),
            image_option(ImageType::Logo, 1, 0),
        ],
        "Series" => vec![
            image_option(ImageType::Backdrop, 1, 1280),
            image_option(ImageType::Art, 0, 0),
            image_option(ImageType::Primary, 1, 0),
            image_option(ImageType::Banner, 1, 0),
            image_option(ImageType::Thumb, 1, 0),
            image_option(ImageType::Logo, 1, 0),
        ],
        "MusicAlbum" => vec![
            image_option(ImageType::Backdrop, 0, 1280),
            image_option(ImageType::Disc, 0, 0),
        ],
        "MusicArtist" => vec![
            image_option(ImageType::Backdrop, 1, 1280),
            image_option(ImageType::Banner, 0, 0),
            image_option(ImageType::Art, 0, 0),
            image_option(ImageType::Logo, 1, 0),
        ],
        "BoxSet" => vec![
            image_option(ImageType::Backdrop, 1, 1280),
            image_option(ImageType::Primary, 1, 0),
            image_option(ImageType::Thumb, 1, 0),
            image_option(ImageType::Logo, 1, 0),
            image_option(ImageType::Art, 0, 0),
            image_option(ImageType::Disc, 0, 0),
            image_option(ImageType::Banner, 0, 0),
        ],
        "Season" => vec![
            image_option(ImageType::Backdrop, 0, 1280),
            image_option(ImageType::Primary, 1, 0),
            image_option(ImageType::Banner, 0, 0),
            image_option(ImageType::Thumb, 0, 0),
        ],
        "Episode" => vec![
            image_option(ImageType::Backdrop, 0, 1280),
            image_option(ImageType::Primary, 1, 0),
        ],
        _ => Vec::new(),
    }
}

const fn image_option(image_type: ImageType, limit: i32, min_width: i32) -> ImageOption {
    ImageOption {
        image_type,
        limit,
        min_width,
    }
}

fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn safe_filename(path: &str) -> String {
    FilePath::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download")
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                Some(character)
            } else if character.is_ascii_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect()
}
