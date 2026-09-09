use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_data::BaseItemError;
use jellyfin_model::{ImageProviderInfo, RemoteImageResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, authorization, item_images::parse_image_type};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RemoteImagesQuery {
    #[serde(default, rename = "type", alias = "Type")]
    image_type: Option<String>,
    #[serde(
        default,
        rename = "startIndex",
        alias = "StartIndex",
        alias = "startindex"
    )]
    start_index: Option<i32>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<i32>,
    #[serde(
        default,
        rename = "providerName",
        alias = "ProviderName",
        alias = "providername"
    )]
    provider_name: Option<String>,
    #[serde(
        default,
        rename = "includeAllLanguages",
        alias = "IncludeAllLanguages",
        alias = "includealllanguages"
    )]
    include_all_languages: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DownloadRemoteImageQuery {
    #[serde(default, rename = "type", alias = "Type")]
    image_type: Option<String>,
    #[serde(default, rename = "imageUrl", alias = "ImageUrl", alias = "imageurl")]
    image_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FetchRemoteImageQuery {
    #[serde(rename = "imageUrl", alias = "ImageUrl", alias = "imageurl")]
    image_url: Option<String>,
    #[serde(
        rename = "providerName",
        alias = "ProviderName",
        alias = "providername"
    )]
    provider_name: Option<String>,
}

/// Streams an administrator-authorized remote image without decoding or
/// caching it. This is the wire contract used by Emby's remote-image lookup.
pub(crate) async fn fetch(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<FetchRemoteImageQuery>,
) -> Result<Response, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    let image_url = query
        .image_url
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::InvalidRequest)?;
    if uri
        .path()
        .to_ascii_lowercase()
        .ends_with("/remotesearch/image")
        && query.provider_name.as_deref().is_none_or(str::is_empty)
    {
        return Err(ApiError::InvalidRequest);
    }
    let response = state
        .remote_stream_client
        .get(image_url)
        .send()
        .await
        .map_err(|_| ApiError::UpstreamUnavailable)?;
    if !response.status().is_success() {
        return Err(if response.status() == StatusCode::NOT_FOUND {
            ApiError::NotFound
        } else {
            ApiError::UpstreamUnavailable
        });
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.to_ascii_lowercase().starts_with("image/"))
        .ok_or(ApiError::UnsupportedMediaType)?;
    let content_type = HeaderValue::from_str(content_type).map_err(|_| ApiError::Internal)?;
    let mut output = Response::new(Body::from_stream(response.bytes_stream()));
    output
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    Ok(output)
}

pub(crate) async fn images(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<RemoteImagesQuery>,
) -> Result<Json<RemoteImageResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let image_type = query
        .image_type
        .as_deref()
        .map(parse_image_type)
        .transpose()?;
    // RemoteImageController receives nullable Int32 values. Enumerable.Skip
    // ignores a negative offset, while Take returns an empty sequence for a
    // non-positive limit.
    let start_index = usize::try_from(query.start_index.unwrap_or_default()).unwrap_or_default();
    let limit = query.limit.map(|limit| {
        if limit <= 0 {
            0
        } else {
            usize::try_from(limit).unwrap_or_default()
        }
    });
    state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;

    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let configuration = state.server_configuration.load().await?;
    let result = state
        .item_lookup
        .remote_images(
            item_id,
            image_type,
            query.provider_name.as_deref(),
            query.include_all_languages,
            start_index,
            limit,
            &api_key,
            &configuration.preferred_metadata_language,
            &configuration.metadata_country_code,
        )
        .await?;
    Ok(Json(result))
}

pub(crate) async fn providers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<Json<Vec<ImageProviderInfo>>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .user_library
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    let api_key = Arc::clone(&*state.tmdb_api_key.read().await);
    let metadata_options = metadata_options_for(&state);
    Ok(Json(
        state
            .item_lookup
            .remote_image_providers(item_id, &api_key, &metadata_options)
            .await?,
    ))
}

fn metadata_options_for(state: &AppState) -> jellyfin_model::MetadataOptions {
    let _ = state;
    jellyfin_model::MetadataOptions::official_defaults()
        .into_iter()
        .find(|options| options.item_type == "Movie")
        .unwrap_or_default()
}

pub(crate) async fn download(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<DownloadRemoteImageQuery>,
) -> Result<StatusCode, ApiError> {
    authorization::require_default(&state, &headers, &uri)
        .await?
        .require_administrator()?;
    let image_type = query
        .image_type
        .as_deref()
        .ok_or(ApiError::InvalidRequest)
        .and_then(parse_image_type)?;
    let image_url = query.image_url.ok_or(ApiError::NotFound)?;
    ensure_item_exists(&state, item_id).await?;

    if state.tmdb_api_key.read().await.is_empty() {
        return Err(BaseItemError::NotFound.into());
    }
    state
        .item_images
        .download_remote_image(item_id, image_type, &image_url)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_item_exists(state: &AppState, item_id: Uuid) -> Result<(), ApiError> {
    state
        .base_items
        .get(item_id)
        .await?
        .ok_or(BaseItemError::NotFound)?;
    Ok(())
}

#[cfg(test)]
mod query_tests {
    use axum_extra::extract::Query;

    use super::{DownloadRemoteImageQuery, FetchRemoteImageQuery, RemoteImagesQuery};

    #[test]
    fn remote_image_queries_bind_all_lowercase_compound_names() {
        let uri =
            "http://localhost/?type=2&startindex=10&providername=Example&includealllanguages=true"
                .parse()
                .unwrap();
        let query = Query::<RemoteImagesQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.image_type.as_deref(), Some("2"));
        assert_eq!(query.start_index, Some(10));
        assert_eq!(query.provider_name.as_deref(), Some("Example"));
        assert!(query.include_all_languages);

        let uri = "http://localhost/?imageurl=https%3A%2F%2Fexample.invalid%2Fposter.jpg"
            .parse()
            .unwrap();
        let query = Query::<DownloadRemoteImageQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        assert_eq!(
            query.image_url.as_deref(),
            Some("https://example.invalid/poster.jpg")
        );

        let uri = "http://localhost/?ImageUrl=https%3A%2F%2Fexample.invalid%2Fposter.jpg&ProviderName=tmdb"
            .parse()
            .unwrap();
        let query = Query::<FetchRemoteImageQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        assert_eq!(
            query.image_url.as_deref(),
            Some("https://example.invalid/poster.jpg")
        );
        assert_eq!(query.provider_name.as_deref(), Some("tmdb"));
    }
}
