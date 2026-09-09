use std::{sync::Arc, time::SystemTime};

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::Query;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use jellyfin_data::BaseItemError;
use jellyfin_drawing::{ImageProcessingError, ImageSource, original_image};
use jellyfin_model::{ImageFormat, ImageInfo, ImageType, MimeTypes};
use serde::Deserialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication};

const TRANSFER_MODE: HeaderName = HeaderName::from_static("transfermode.dlna.org");
const REAL_TIME_INFO: HeaderName = HeaderName::from_static("realtimeinfo.dlna.org");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct GetItemImageQuery {
    #[serde(default, rename = "maxWidth", alias = "MaxWidth", alias = "maxwidth")]
    max_width: Option<i32>,
    #[serde(
        default,
        rename = "maxHeight",
        alias = "MaxHeight",
        alias = "maxheight"
    )]
    max_height: Option<i32>,
    #[serde(default, rename = "width", alias = "Width")]
    width: Option<i32>,
    #[serde(default, rename = "height", alias = "Height")]
    height: Option<i32>,
    #[serde(default, rename = "quality", alias = "Quality")]
    quality: Option<i32>,
    #[serde(
        default,
        rename = "fillWidth",
        alias = "FillWidth",
        alias = "fillwidth"
    )]
    fill_width: Option<i32>,
    #[serde(
        default,
        rename = "fillHeight",
        alias = "FillHeight",
        alias = "fillheight"
    )]
    fill_height: Option<i32>,
    #[serde(default, rename = "tag", alias = "Tag")]
    tag: Option<String>,
    #[serde(default, rename = "format", alias = "Format")]
    format: Option<String>,
    #[serde(
        default,
        rename = "percentPlayed",
        alias = "PercentPlayed",
        alias = "percentplayed"
    )]
    percent_played: Option<f64>,
    #[serde(
        default,
        rename = "unplayedCount",
        alias = "UnplayedCount",
        alias = "unplayedcount"
    )]
    unplayed_count: Option<i32>,
    #[serde(default, rename = "blur", alias = "Blur")]
    blur: Option<i32>,
    #[serde(
        default,
        rename = "backgroundColor",
        alias = "BackgroundColor",
        alias = "backgroundcolor"
    )]
    background_color: Option<String>,
    #[serde(
        default,
        rename = "foregroundLayer",
        alias = "ForegroundLayer",
        alias = "foregroundlayer"
    )]
    foreground_layer: Option<String>,
    #[serde(
        default,
        rename = "imageIndex",
        alias = "ImageIndex",
        alias = "imageindex"
    )]
    pub(crate) image_index: Option<i32>,
    #[serde(default, rename = "accept", alias = "Accept")]
    accept: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DeleteItemImageQuery {
    #[serde(
        default,
        rename = "imageIndex",
        alias = "ImageIndex",
        alias = "imageindex"
    )]
    image_index: Option<i32>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdateItemImageIndexQuery {
    #[serde(default, rename = "newIndex", alias = "NewIndex", alias = "newindex")]
    new_index: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImageUrlQuery {
    #[serde(rename = "url", alias = "Url", alias = "URL")]
    url: String,
}

pub(crate) async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ImageInfo>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let item = state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    Ok(Json(state.item_images.list(&item).await?))
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type)): Path<(Uuid, String)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    let image_type = parse_image_type(&image_type)?;
    let image_index = query.image_index.unwrap_or(0);
    get_internal(state, uri, headers, item_id, image_type, image_index, query).await
}

pub(crate) async fn get_by_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type, image_index)): Path<(Uuid, String, i32)>,
    Query(query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    let image_type = parse_image_type(&image_type)?;
    get_internal(state, uri, headers, item_id, image_type, image_index, query).await
}

#[allow(clippy::type_complexity)]
pub(crate) async fn get_legacy_path(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((
        item_id,
        image_type,
        image_index,
        tag,
        format,
        max_width,
        max_height,
        percent_played,
        unplayed_count,
    )): Path<(Uuid, String, i32, String, String, i32, i32, f64, i32)>,
    Query(mut query): Query<GetItemImageQuery>,
) -> Result<Response, ApiError> {
    query.tag = Some(tag);
    query.format = Some(format);
    query.max_width = Some(max_width);
    query.max_height = Some(max_height);
    query.percent_played = Some(percent_played);
    query.unplayed_count = Some(unplayed_count);
    get_internal(
        state,
        uri,
        headers,
        item_id,
        parse_image_type(&image_type)?,
        image_index,
        query,
    )
    .await
}

pub(crate) async fn delete(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type)): Path<(Uuid, String)>,
    Query(query): Query<DeleteItemImageQuery>,
) -> Result<StatusCode, ApiError> {
    delete_internal(
        &state,
        &uri,
        &headers,
        item_id,
        &image_type,
        query.image_index.unwrap_or(0),
    )
    .await
}

pub(crate) async fn delete_by_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type, image_index)): Path<(Uuid, String, i32)>,
) -> Result<StatusCode, ApiError> {
    delete_internal(&state, &uri, &headers, item_id, &image_type, image_index).await
}

pub(crate) async fn upload(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type)): Path<(Uuid, String)>,
    request: Request<Body>,
) -> Result<StatusCode, ApiError> {
    upload_internal(state, uri, headers, item_id, image_type, request).await
}

pub(crate) async fn upload_by_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type, _image_index)): Path<(Uuid, String, i32)>,
    request: Request<Body>,
) -> Result<StatusCode, ApiError> {
    upload_internal(state, uri, headers, item_id, image_type, request).await
}

pub(crate) async fn upload_url(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type, _image_index)): Path<(Uuid, String, i32)>,
    Query(query): Query<ImageUrlQuery>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let image_type = parse_image_type(&image_type)?;
    if query.url.is_empty() {
        return Err(ApiError::InvalidRequest);
    }
    state
        .item_images
        .download_remote_image(item_id, image_type, &query.url)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn update_index(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path((item_id, image_type, image_index)): Path<(Uuid, String, i32)>,
    Query(query): Query<UpdateItemImageIndexQuery>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let image_type = parse_image_type(&image_type)?;
    let new_index = query.new_index.ok_or(ApiError::InvalidRequest)?;
    state
        .item_images
        .swap(
            item_id,
            image_type,
            i64::from(image_index),
            i64::from(new_index),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn upload_internal(
    state: Arc<AppState>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    item_id: Uuid,
    image_type: String,
    request: Request<Body>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let image_type = parse_image_type(&image_type)?;
    state
        .base_items
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;
    let extension = MimeTypes::try_get_image_extension(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    )
    .ok_or(ApiError::InvalidRequest)?;
    if image_type == ImageType::Chapter {
        return Err(jellyfin_controller::ItemImageError::UnsupportedImageType.into());
    }
    let image = decode_base64_image_body(request.into_body()).await?;
    state
        .item_images
        .upload(item_id, image_type, &extension, &image)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn decode_base64_image_body(body: Body) -> Result<Vec<u8>, ApiError> {
    let encoded = to_bytes(body, usize::MAX)
        .await
        .map_err(|_| ApiError::Internal)?;
    let mut encoded = encoded
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    encoded.truncate(encoded.len() / 4 * 4);
    BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| ApiError::Internal)
}

async fn delete_internal(
    state: &AppState,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    item_id: Uuid,
    image_type: &str,
    image_index: i32,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(state, headers, Some(uri))
        .await?
        .require_administrator()?;
    let image_type = parse_image_type(image_type)?;
    if let Ok(image_index) = u32::try_from(image_index) {
        state
            .item_images
            .delete(item_id, image_type, image_index)
            .await?;
    } else {
        state
            .base_items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn parse_image_type(value: &str) -> Result<ImageType, ApiError> {
    let numeric = value.parse::<i32>().ok();
    [
        ImageType::Primary,
        ImageType::Art,
        ImageType::Backdrop,
        ImageType::Banner,
        ImageType::Logo,
        ImageType::Thumb,
        ImageType::Disc,
        ImageType::Box,
        ImageType::Screenshot,
        ImageType::Menu,
        ImageType::Chapter,
        ImageType::BoxRear,
        ImageType::Profile,
    ]
    .into_iter()
    .find(|image_type| {
        image_type_name(*image_type).eq_ignore_ascii_case(value)
            || numeric == Some(*image_type as i32)
    })
    .ok_or(ApiError::InvalidRequest)
}

const fn image_type_name(image_type: ImageType) -> &'static str {
    match image_type {
        ImageType::Primary => "primary",
        ImageType::Art => "art",
        ImageType::Backdrop => "backdrop",
        ImageType::Banner => "banner",
        ImageType::Logo => "logo",
        ImageType::Thumb => "thumb",
        ImageType::Disc => "disc",
        ImageType::Box => "box",
        ImageType::Screenshot => "screenshot",
        ImageType::Menu => "menu",
        ImageType::Chapter => "chapter",
        ImageType::BoxRear => "boxrear",
        ImageType::Profile => "profile",
    }
}

async fn get_internal(
    state: Arc<AppState>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    item_id: Uuid,
    image_type: ImageType,
    image_index: i32,
    query: GetItemImageQuery,
) -> Result<Response, ApiError> {
    ensure_visible_item(&state, &headers, &uri, item_id).await?;
    render_item_image(&state, &headers, item_id, image_type, image_index, query).await
}

pub(crate) async fn render_item_image(
    state: &AppState,
    headers: &HeaderMap,
    item_id: Uuid,
    image_type: ImageType,
    image_index: i32,
    query: GetItemImageQuery,
) -> Result<Response, ApiError> {
    let image_index = u32::try_from(image_index).map_err(|_| BaseItemError::NotFound)?;
    let source = if image_type == ImageType::Chapter {
        let chapter = state
            .chapters
            .get(
                item_id,
                i32::try_from(image_index).map_err(|_| BaseItemError::NotFound)?,
            )
            .await
            .map_err(|_| ApiError::Internal)?
            .ok_or(jellyfin_controller::ItemImageError::NotFound)?;
        let path = chapter
            .image_path
            .filter(|path| !path.is_empty())
            .ok_or(jellyfin_controller::ItemImageError::NotFound)?;
        ImageSource {
            path: path.into(),
            date_modified: SystemTime::from(
                chapter
                    .image_date_modified
                    .unwrap_or_else(|| jellyfin_model::ChapterInfo::default().image_date_modified),
            ),
            width: None,
            height: None,
        }
    } else {
        let resource = state
            .item_images
            .resource(item_id, image_type, image_index)
            .await?;
        ImageSource {
            path: resource.path,
            date_modified: SystemTime::from(resource.date_modified),
            width: resource.width,
            height: resource.height,
        }
    };
    validate_direct_image_request(&query)?;
    // This server intentionally ignores image transformation parameters. Media-library clients
    // request many differently sized derivatives while browsing; serving the source bytes avoids
    // decoder-sized memory spikes and an unbounded family of cached variants.
    let processed = original_image(source).await?;
    build_response(headers, query.tag.as_deref(), processed).await
}

pub(crate) async fn render_simple_image(
    _state: &AppState,
    headers: &HeaderMap,
    path: std::path::PathBuf,
    date_modified: DateTime<Utc>,
    tag: Option<&str>,
    format: Option<&str>,
) -> Result<Response, ApiError> {
    if let Some(format) = format {
        parse_image_format(format)?;
    }
    let source = ImageSource {
        path,
        date_modified: SystemTime::from(date_modified),
        width: None,
        height: None,
    };
    let processed = original_image(source).await?;
    build_response(headers, tag, processed).await
}

fn validate_direct_image_request(query: &GetItemImageQuery) -> Result<(), ApiError> {
    if let Some(format) = query.format.as_deref() {
        parse_image_format(format)?;
    }
    if query.percent_played.is_some_and(|value| !value.is_finite()) {
        return Err(ImageProcessingError::InvalidPercentPlayed.into());
    }

    // Keep accepting the official transformation surface even though this deployment policy
    // intentionally ignores it and returns the source file.
    let _ignored_transformations = (
        query.width,
        query.height,
        query.max_width,
        query.max_height,
        query.fill_width,
        query.fill_height,
        query.quality,
        query.blur,
        query.percent_played,
        query.unplayed_count,
        query.background_color.as_deref(),
        query.foreground_layer.as_deref(),
        query.accept.as_deref(),
    );
    Ok(())
}

async fn ensure_visible_item(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    item_id: Uuid,
) -> Result<(), ApiError> {
    if let Some(user_id) =
        authentication::optional_authenticated_user_id(state, headers, uri).await?
    {
        state.user_data.visible_item(user_id, item_id).await?;
    } else {
        state
            .base_items
            .get(item_id)
            .await?
            .ok_or(BaseItemError::NotFound)?;
    }
    Ok(())
}

fn parse_image_format(value: &str) -> Result<ImageFormat, ApiError> {
    let numeric = value.parse::<i32>().ok();
    ImageFormat::ALL
        .into_iter()
        .find(|format| {
            format_name(*format).eq_ignore_ascii_case(value) || numeric == Some(*format as i32)
        })
        .ok_or(ApiError::InvalidRequest)
}

const fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Bmp => "bmp",
        ImageFormat::Gif => "gif",
        ImageFormat::Jpg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Webp => "webp",
        ImageFormat::Svg => "svg",
    }
}

async fn build_response(
    request_headers: &HeaderMap,
    tag: Option<&str>,
    processed: jellyfin_drawing::ProcessedImage,
) -> Result<Response, ApiError> {
    let disable_caching = cache_control_has_no_cache(request_headers);
    let tag = tag.filter(|tag| !tag.is_empty());
    let modified = DateTime::<Utc>::from(processed.date_modified);
    if !disable_caching
        && (tag.is_some_and(|tag| if_none_match(request_headers, tag))
            || if_modified_since(request_headers, modified))
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        apply_image_headers(
            response.headers_mut(),
            processed.mime_type,
            modified,
            tag,
            false,
        )?;
        return Ok(response);
    }

    let request = Request::get("/")
        .body(Body::empty())
        .map_err(|_| ApiError::Internal)?;
    let response = match ServeFile::new(processed.path).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let mut response = response.map(Body::new);
    response.headers_mut().remove(header::ACCEPT_RANGES);
    apply_image_headers(
        response.headers_mut(),
        processed.mime_type,
        modified,
        tag,
        disable_caching,
    )?;
    Ok(response)
}

fn apply_image_headers(
    headers: &mut HeaderMap,
    mime_type: &str,
    modified: DateTime<Utc>,
    tag: Option<&str>,
    disable_caching: bool,
) -> Result<(), ApiError> {
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime_type).map_err(|_| ApiError::Internal)?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Accept"));
    headers.insert(TRANSFER_MODE, HeaderValue::from_static("Interactive"));
    headers.insert(REAL_TIME_INFO, HeaderValue::from_static("DLNA.ORG_TLAG=*"));
    let age = round_milliseconds_to_seconds(
        Utc::now()
            .signed_duration_since(modified)
            .num_milliseconds(),
    );
    headers.insert(
        header::AGE,
        HeaderValue::from_str(&age.to_string()).map_err(|_| ApiError::Internal)?,
    );
    if disable_caching {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
        headers.insert(
            header::PRAGMA,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
        headers.remove(header::LAST_MODIFIED);
        headers.remove(header::ETAG);
        return Ok(());
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if tag.is_some() {
            "public, max-age=31536000, immutable"
        } else {
            "public"
        }),
    );
    headers.insert(
        header::LAST_MODIFIED,
        HeaderValue::from_str(&modified.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
            .map_err(|_| ApiError::Internal)?,
    );
    if let Some(tag) = tag {
        headers.insert(
            header::ETAG,
            HeaderValue::from_str(&format!("\"{tag}\"")).map_err(|_| ApiError::InvalidRequest)?,
        );
    }
    Ok(())
}

fn round_milliseconds_to_seconds(milliseconds: i64) -> i64 {
    let seconds = milliseconds / 1_000;
    let remainder = milliseconds % 1_000;
    match remainder.abs().cmp(&500) {
        std::cmp::Ordering::Less => seconds,
        std::cmp::Ordering::Equal if seconds % 2 == 0 => seconds,
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => seconds + remainder.signum(),
    }
}

fn cache_control_has_no_cache(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case("no-cache"))
}

fn if_none_match(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == tag || value == format!("\"{tag}\""))
}

fn if_modified_since(headers: &HeaderMap, modified: DateTime<Utc>) -> bool {
    headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
        .is_some_and(|cached| modified <= cached)
}

#[cfg(test)]
mod query_tests {
    use axum_extra::extract::Query;

    use super::{DeleteItemImageQuery, GetItemImageQuery, UpdateItemImageIndexQuery};

    #[test]
    fn image_enum_parsers_accept_official_names_and_integer_values() {
        for (number, name) in [
            (0, "Primary"),
            (1, "Art"),
            (2, "Backdrop"),
            (3, "Banner"),
            (4, "Logo"),
            (5, "Thumb"),
            (6, "Disc"),
            (7, "Box"),
            (8, "Screenshot"),
            (9, "Menu"),
            (10, "Chapter"),
            (11, "BoxRear"),
            (12, "Profile"),
        ] {
            assert_eq!(
                super::parse_image_type(name).unwrap() as i32,
                number,
                "name {name}"
            );
            assert_eq!(
                super::parse_image_type(&name.to_ascii_lowercase()).unwrap() as i32,
                number,
                "lowercase name {name}"
            );
            assert_eq!(
                super::parse_image_type(&number.to_string()).unwrap() as i32,
                number,
                "integer {number}"
            );
        }
        for value in ["", "13", "-1", "not-an-image"] {
            assert!(super::parse_image_type(value).is_err(), "value {value}");
        }

        for (number, name) in [
            (0, "Bmp"),
            (1, "Gif"),
            (2, "Jpg"),
            (3, "Png"),
            (4, "Webp"),
            (5, "Svg"),
        ] {
            assert_eq!(
                super::parse_image_format(name).unwrap() as i32,
                number,
                "name {name}"
            );
            assert_eq!(
                super::parse_image_format(&number.to_string()).unwrap() as i32,
                number,
                "integer {number}"
            );
        }
        for value in ["", "6", "-1", "not-an-image"] {
            assert!(super::parse_image_format(value).is_err(), "value {value}");
        }
    }

    #[test]
    fn image_queries_bind_all_lowercase_compound_names() {
        let uri = "http://localhost/?maxwidth=1&maxheight=2&fillwidth=3&fillheight=4&percentplayed=5.5&unplayedcount=6&backgroundcolor=black&foregroundlayer=shadow&imageindex=7"
            .parse()
            .unwrap();
        let query = Query::<GetItemImageQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.max_width, Some(1));
        assert_eq!(query.max_height, Some(2));
        assert_eq!(query.fill_width, Some(3));
        assert_eq!(query.fill_height, Some(4));
        assert_eq!(query.percent_played, Some(5.5));
        assert_eq!(query.unplayed_count, Some(6));
        assert_eq!(query.background_color.as_deref(), Some("black"));
        assert_eq!(query.foreground_layer.as_deref(), Some("shadow"));
        assert_eq!(query.image_index, Some(7));

        let delete = Query::<DeleteItemImageQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(delete.image_index, Some(7));

        let uri = "http://localhost/?newindex=8".parse().unwrap();
        let update = Query::<UpdateItemImageIndexQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        assert_eq!(update.new_index, Some(8));
    }
}
