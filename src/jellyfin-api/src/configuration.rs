use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{OriginalUri, Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use jellyfin_data::{
    NamedConfigurationStoreError, ServerConfigurationUpdate, entities::server_configuration,
};
use jellyfin_model::{
    ImageSavingConvention, MetadataOptions, NameValuePair, RepositoryInfo, ServerConfiguration,
    TrickplayOptions,
};
use serde_json::Value;

use crate::{ApiError, AppState, authentication, authorization};

/// The encoding settings that affect request-time FFmpeg command generation.
///
/// Jellyfin keeps these values in the named `encoding` configuration rather
/// than in `ServerConfiguration`.  Loading the small subset here keeps the
/// progressive routes on the same configuration contract without coupling
/// the API crate to the much larger dashboard model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EncodingRuntimeOptions {
    pub(crate) encoding_thread_count: i32,
    pub(crate) enable_audio_vbr: bool,
}

impl Default for EncodingRuntimeOptions {
    fn default() -> Self {
        Self {
            // Official EncodingOptions uses -1 to request FFmpeg's automatic
            // thread selection. EncodingHelper consequently emits 0.
            encoding_thread_count: -1,
            enable_audio_vbr: false,
        }
    }
}

pub(crate) async fn encoding_runtime_options(
    state: &AppState,
) -> Result<EncodingRuntimeOptions, ApiError> {
    let Some(repository) = state.named_configurations.as_ref() else {
        return Ok(EncodingRuntimeOptions::default());
    };
    let configuration = match repository.load("encoding").await {
        Ok(configuration) => configuration.configuration,
        Err(NamedConfigurationStoreError::NotFound(_)) => {
            return Ok(EncodingRuntimeOptions::default());
        }
        Err(error) => return Err(error.into()),
    };
    parse_encoding_runtime_options(&configuration)
}

fn parse_encoding_runtime_options(value: &Value) -> Result<EncodingRuntimeOptions, ApiError> {
    let object = value.as_object().ok_or(ApiError::Internal)?;
    let mut options = EncodingRuntimeOptions::default();
    for (name, value) in object {
        if name.eq_ignore_ascii_case("EncodingThreadCount") {
            options.encoding_thread_count = json_i32(value).ok_or(ApiError::Internal)?;
        } else if name.eq_ignore_ascii_case("EnableAudioVbr") {
            options.enable_audio_vbr = json_bool(value).ok_or(ApiError::Internal)?;
        }
    }
    Ok(options)
}

fn json_i32(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn json_bool(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        let value = value.as_str()?.trim();
        if value.eq_ignore_ascii_case("true") {
            Some(true)
        } else if value.eq_ignore_ascii_case("false") {
            Some(false)
        } else {
            None
        }
    })
}

pub(crate) async fn get(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<ServerConfiguration>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let configuration = state.server_configuration.load().await?;
    Ok(Json(server_configuration(configuration)?))
}

pub(crate) async fn update(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    request: Result<Json<ServerConfiguration>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    apply_configuration(&state, configuration).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn partial(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let patch: Value = serde_json::from_slice(&body).map_err(|_| ApiError::InvalidRequest)?;
    let Some(patch) = patch.as_object() else {
        return Err(ApiError::InvalidRequest);
    };
    let current = state.server_configuration.load().await?;
    let mut merged =
        serde_json::to_value(server_configuration(current)?).map_err(|_| ApiError::Internal)?;
    let object = merged.as_object_mut().ok_or(ApiError::Internal)?;
    for (key, value) in patch {
        let destination = object
            .keys()
            .find(|existing| existing.eq_ignore_ascii_case(key))
            .cloned()
            .unwrap_or_else(|| key.clone());
        object.insert(destination, value.clone());
    }
    let configuration: ServerConfiguration =
        serde_json::from_value(merged).map_err(|_| ApiError::InvalidRequest)?;
    apply_configuration(&state, configuration).await?;
    Ok(StatusCode::OK)
}

async fn apply_configuration(
    state: &AppState,
    configuration: ServerConfiguration,
) -> Result<(), ApiError> {
    let updated = state
        .server_configuration
        .update_server_configuration(server_configuration_update(configuration)?)
        .await?;
    *state.tmdb_api_key.write().await = Arc::from(updated.tmdb_api_key);
    *state.omdb_api_key.write().await = Arc::from(updated.omdb_api_key);
    state.metadata_refresh.set_preferred_locale(
        updated.preferred_metadata_language,
        updated.metadata_country_code,
    );
    state
        .quick_connect_capability
        .set_enabled(updated.quick_connect_available);
    state
        .metrics_enabled
        .store(updated.enable_metrics, std::sync::atomic::Ordering::Release);
    state
        .scheduled_tasks
        .set_log_file_retention_days(updated.log_file_retention_days);
    state
        .scheduled_tasks
        .set_activity_log_retention_days(updated.activity_log_retention_days.unwrap_or(30));
    state.scheduled_tasks.set_trickplay_options(
        serde_json::from_value(updated.trickplay_options).map_err(|_| ApiError::Internal)?,
    );
    state
        .library_scan
        .set_fanout_concurrency(updated.library_scan_fanout_concurrency.max(0) as usize);
    Ok(())
}

pub(crate) async fn default_metadata_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<MetadataOptions>, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    Ok(Json(MetadataOptions::default()))
}

pub(crate) fn metadata_options_for_item_type(
    configuration: &server_configuration::Model,
    item_type: &str,
) -> Result<MetadataOptions, ApiError> {
    let options = metadata_options(configuration)?;
    Ok(options
        .into_iter()
        .find(|options| options.item_type.eq_ignore_ascii_case(item_type))
        .unwrap_or_default())
}

pub(crate) fn metadata_options(
    configuration: &server_configuration::Model,
) -> Result<Vec<MetadataOptions>, ApiError> {
    serde_json::from_value(configuration.metadata_options.clone()).map_err(|_| ApiError::Internal)
}

pub(crate) async fn get_named(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<Value>, ApiError> {
    get_named_by_key(&state, &uri, &headers, &key).await
}

pub(crate) async fn get_branding_named(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    get_named_by_key(&state, &uri, &headers, "branding").await
}

async fn get_named_by_key(
    state: &AppState,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    key: &str,
) -> Result<Json<Value>, ApiError> {
    authorization::require_default(&state, &headers, &uri).await?;
    let repository = state
        .named_configurations
        .as_ref()
        .ok_or(ApiError::Internal)?;
    let configuration = repository.load(key).await?;
    Ok(Json(configuration.configuration))
}

pub(crate) async fn update_named(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(key): Path<String>,
    request: Result<Json<Value>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    authentication::authenticated_identity(&state, &headers, Some(&uri))
        .await?
        .require_administrator()?;
    let Json(configuration) = request.map_err(|_| ApiError::InvalidRequest)?;
    if !configuration.is_object() {
        return Err(ApiError::InvalidRequest);
    }
    let repository = state
        .named_configurations
        .as_ref()
        .ok_or(ApiError::Internal)?;
    repository.save(&key, configuration).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn server_configuration(
    model: server_configuration::Model,
) -> Result<ServerConfiguration, ApiError> {
    Ok(ServerConfiguration {
        is_startup_wizard_completed: model.is_startup_wizard_completed,
        preferred_metadata_language: model.preferred_metadata_language,
        metadata_country_code: model.metadata_country_code,
        min_resume_pct: model.min_resume_pct,
        max_resume_pct: model.max_resume_pct,
        min_resume_duration_seconds: model.min_resume_duration_seconds,
        min_audiobook_resume: model.min_audiobook_resume,
        max_audiobook_resume: model.max_audiobook_resume,
        server_name: model.server_name,
        ui_culture: model.ui_culture,
        content_types: serde_json::from_value::<Vec<NameValuePair>>(model.content_types)
            .map_err(|_| ApiError::Internal)?,
        plugin_repositories: serde_json::from_value::<Vec<RepositoryInfo>>(
            model.plugin_repositories,
        )
        .map_err(|_| ApiError::Internal)?,
        allow_client_log_upload: model.allow_client_log_upload,
        trickplay_options: serde_json::from_value::<TrickplayOptions>(model.trickplay_options)
            .map_err(|_| ApiError::Internal)?,
        cast_receiver_applications: serde_json::from_value(model.cast_receiver_applications)
            .map_err(|_| ApiError::Internal)?,
        tmdb_api_key: model.tmdb_api_key,
        omdb_api_key: model.omdb_api_key,
        quick_connect_available: model.quick_connect_available,
        log_file_retention_days: model.log_file_retention_days,
        enable_metrics: model.enable_metrics,
        enable_normalized_item_by_name_ids: model.enable_normalized_item_by_name_ids,
        enable_case_sensitive_item_ids: model.enable_case_sensitive_item_ids,
        metadata_path: model.metadata_path,
        metadata_options: serde_json::from_value::<Vec<MetadataOptions>>(model.metadata_options)
            .map_err(|_| ApiError::Internal)?,
        sort_replace_characters: serde_json::from_value(model.sort_replace_characters)
            .map_err(|_| ApiError::Internal)?,
        sort_remove_characters: serde_json::from_value(model.sort_remove_characters)
            .map_err(|_| ApiError::Internal)?,
        sort_remove_words: serde_json::from_value(model.sort_remove_words)
            .map_err(|_| ApiError::Internal)?,
        inactive_session_threshold: model.inactive_session_threshold,
        library_monitor_delay: model.library_monitor_delay,
        library_update_duration: model.library_update_duration,
        cache_size: model
            .cache_size
            .unwrap_or_else(|| ServerConfiguration::default().cache_size),
        image_saving_convention: image_saving_convention(model.image_saving_convention)?,
        save_metadata_hidden: model.save_metadata_hidden,
        remote_client_bitrate_limit: model.remote_client_bitrate_limit,
        enable_folder_view: model.enable_folder_view,
        enable_grouping_movies_into_collections: model.enable_grouping_movies_into_collections,
        enable_grouping_shows_into_collections: model.enable_grouping_shows_into_collections,
        display_specials_within_seasons: model.display_specials_within_seasons,
        enable_external_content_in_suggestions: model.enable_external_content_in_suggestions,
        cors_hosts: serde_json::from_value(model.cors_hosts).map_err(|_| ApiError::Internal)?,
        activity_log_retention_days: model.activity_log_retention_days,
        library_scan_fanout_concurrency: model.library_scan_fanout_concurrency,
        library_metadata_refresh_concurrency: model.library_metadata_refresh_concurrency,
        ..ServerConfiguration::default()
    })
}

fn server_configuration_update(
    configuration: ServerConfiguration,
) -> Result<ServerConfigurationUpdate, ApiError> {
    Ok(ServerConfigurationUpdate {
        server_name: configuration.server_name,
        ui_culture: configuration.ui_culture,
        metadata_country_code: configuration.metadata_country_code,
        preferred_metadata_language: configuration.preferred_metadata_language,
        is_startup_wizard_completed: configuration.is_startup_wizard_completed,
        content_types: serde_json::to_value(configuration.content_types)
            .map_err(|_| ApiError::Internal)?,
        plugin_repositories: serde_json::to_value(configuration.plugin_repositories)
            .map_err(|_| ApiError::Internal)?,
        min_resume_pct: configuration.min_resume_pct,
        max_resume_pct: configuration.max_resume_pct,
        min_resume_duration_seconds: configuration.min_resume_duration_seconds,
        min_audiobook_resume: configuration.min_audiobook_resume,
        max_audiobook_resume: configuration.max_audiobook_resume,
        allow_client_log_upload: configuration.allow_client_log_upload,
        trickplay_options: serde_json::to_value(configuration.trickplay_options)
            .map_err(|_| ApiError::Internal)?,
        cast_receiver_applications: serde_json::to_value(configuration.cast_receiver_applications)
            .map_err(|_| ApiError::Internal)?,
        tmdb_api_key: configuration.tmdb_api_key,
        quick_connect_available: configuration.quick_connect_available,
        omdb_api_key: configuration.omdb_api_key,
        log_file_retention_days: configuration.log_file_retention_days,
        enable_metrics: configuration.enable_metrics,
        enable_normalized_item_by_name_ids: configuration.enable_normalized_item_by_name_ids,
        enable_case_sensitive_item_ids: configuration.enable_case_sensitive_item_ids,
        metadata_path: configuration.metadata_path,
        metadata_options: serde_json::to_value(configuration.metadata_options)
            .map_err(|_| ApiError::Internal)?,
        sort_replace_characters: serde_json::to_value(configuration.sort_replace_characters)
            .map_err(|_| ApiError::Internal)?,
        sort_remove_characters: serde_json::to_value(configuration.sort_remove_characters)
            .map_err(|_| ApiError::Internal)?,
        sort_remove_words: serde_json::to_value(configuration.sort_remove_words)
            .map_err(|_| ApiError::Internal)?,
        inactive_session_threshold: configuration.inactive_session_threshold,
        library_monitor_delay: configuration.library_monitor_delay,
        library_update_duration: configuration.library_update_duration,
        cache_size: Some(configuration.cache_size),
        image_saving_convention: image_saving_convention_code(
            configuration.image_saving_convention,
        ),
        save_metadata_hidden: configuration.save_metadata_hidden,
        remote_client_bitrate_limit: configuration.remote_client_bitrate_limit,
        enable_folder_view: configuration.enable_folder_view,
        enable_grouping_movies_into_collections: configuration
            .enable_grouping_movies_into_collections,
        enable_grouping_shows_into_collections: configuration
            .enable_grouping_shows_into_collections,
        display_specials_within_seasons: configuration.display_specials_within_seasons,
        enable_external_content_in_suggestions: configuration
            .enable_external_content_in_suggestions,
        cors_hosts: serde_json::to_value(configuration.cors_hosts)
            .map_err(|_| ApiError::Internal)?,
        activity_log_retention_days: configuration.activity_log_retention_days,
        library_scan_fanout_concurrency: configuration.library_scan_fanout_concurrency,
        library_metadata_refresh_concurrency: configuration.library_metadata_refresh_concurrency,
    })
}

const IMAGE_SAVING_CONVENTION_COMPATIBLE: i16 = 1;

fn image_saving_convention_code(convention: ImageSavingConvention) -> i16 {
    match convention {
        ImageSavingConvention::Legacy => 0,
        ImageSavingConvention::Compatible => IMAGE_SAVING_CONVENTION_COMPATIBLE,
    }
}

fn image_saving_convention(code: i16) -> Result<ImageSavingConvention, ApiError> {
    match code {
        0 => Ok(ImageSavingConvention::Legacy),
        IMAGE_SAVING_CONVENTION_COMPATIBLE => Ok(ImageSavingConvention::Compatible),
        _ => Err(ApiError::Internal),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EncodingRuntimeOptions, parse_encoding_runtime_options};

    #[test]
    fn encoding_runtime_options_match_official_defaults() {
        assert_eq!(
            parse_encoding_runtime_options(&json!({})).unwrap(),
            EncodingRuntimeOptions {
                encoding_thread_count: -1,
                enable_audio_vbr: false,
            }
        );
    }

    #[test]
    fn encoding_runtime_options_follow_json_defaults_value_semantics() {
        assert_eq!(
            parse_encoding_runtime_options(&json!({
                "encodingthreadcount": "3",
                "ENABLEAUDIOVBR": "TrUe",
                "Unknown": "ignored",
            }))
            .unwrap(),
            EncodingRuntimeOptions {
                encoding_thread_count: 3,
                enable_audio_vbr: true,
            }
        );
        assert!(
            parse_encoding_runtime_options(&json!({"EncodingThreadCount": 2_i64.pow(40)})).is_err()
        );
        assert!(parse_encoding_runtime_options(&json!({"EnableAudioVbr": "yes"})).is_err());
    }
}
