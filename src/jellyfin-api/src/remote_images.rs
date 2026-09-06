use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
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
    start_index: Option<usize>,
    #[serde(default, rename = "limit", alias = "Limit")]
    limit: Option<usize>,
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
            query.start_index.unwrap_or(0),
            query.limit,
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

    use super::{DownloadRemoteImageQuery, RemoteImagesQuery};

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
    }
}
