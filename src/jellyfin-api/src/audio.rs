use std::{io, path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request},
    response::{IntoResponse, Redirect, Response},
};
use bytes::Bytes;
use futures_util::stream;
use serde::Deserialize;
use tokio::{io::AsyncReadExt, process::Command};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use jellyfin_controller::{FfmpegCommand, audio_command};
use jellyfin_model::{MediaStream, MediaStreamType};

use crate::{ApiError, AppState, authentication};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamQuery {
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i64>,
    #[serde(
        rename = "audioSampleRate",
        alias = "AudioSampleRate",
        alias = "audiosamplerate"
    )]
    audio_sample_rate: Option<i32>,
    #[serde(
        rename = "audioChannels",
        alias = "AudioChannels",
        alias = "audiochannels"
    )]
    audio_channels: Option<i32>,
    #[serde(
        rename = "maxAudioChannels",
        alias = "MaxAudioChannels",
        alias = "maxaudiochannels"
    )]
    max_audio_channels: Option<i32>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        rename = "copyTimestamps",
        alias = "CopyTimestamps",
        alias = "copytimestamps"
    )]
    copy_timestamps: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UniversalQuery {
    #[serde(
        default,
        rename = "container",
        alias = "Container",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    container: Vec<String>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
    #[serde(rename = "userId", alias = "UserId", alias = "userid")]
    user_id: Option<Uuid>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "maxAudioChannels",
        alias = "MaxAudioChannels",
        alias = "maxaudiochannels"
    )]
    max_audio_channels: Option<i32>,
    #[serde(
        rename = "transcodingAudioChannels",
        alias = "TranscodingAudioChannels",
        alias = "transcodingaudiochannels"
    )]
    transcoding_audio_channels: Option<i32>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "maxStreamingBitrate",
        alias = "MaxStreamingBitrate",
        alias = "maxstreamingbitrate"
    )]
    max_streaming_bitrate: Option<i64>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i64>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks"
    )]
    start_time_ticks: Option<i64>,
    #[serde(
        rename = "transcodingContainer",
        alias = "TranscodingContainer",
        alias = "transcodingcontainer"
    )]
    transcoding_container: Option<String>,
    #[serde(
        rename = "transcodingProtocol",
        alias = "TranscodingProtocol",
        alias = "transcodingprotocol"
    )]
    transcoding_protocol: Option<jellyfin_model::MediaStreamProtocol>,
    #[serde(
        rename = "maxAudioSampleRate",
        alias = "MaxAudioSampleRate",
        alias = "maxaudiosamplerate"
    )]
    max_audio_sample_rate: Option<i32>,
    #[serde(
        rename = "maxAudioBitDepth",
        alias = "MaxAudioBitDepth",
        alias = "maxaudiobitdepth"
    )]
    _max_audio_bit_depth: Option<i32>,
    #[serde(
        rename = "enableRemoteMedia",
        alias = "EnableRemoteMedia",
        alias = "enableremotemedia"
    )]
    enable_remote_media: Option<bool>,
    #[serde(
        rename = "enableAudioVbrEncoding",
        alias = "EnableAudioVbrEncoding",
        alias = "enableaudiovbrencoding"
    )]
    _enable_audio_vbr_encoding: Option<bool>,
    #[serde(
        rename = "enableRedirection",
        alias = "EnableRedirection",
        alias = "enableredirection"
    )]
    enable_redirection: Option<bool>,
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, None, query, request).await
}

pub(crate) async fn stream_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, Some(&container), query, request).await
}

/// Compatibility endpoint used by older Emby clients (`/Audio/{id}/{name}`).
/// The filename is only the requested output suffix; authorization and serving
/// remain in the normal stream path.
pub(crate) async fn stream_with_file_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, stream_file_name)): Path<(Uuid, String)>,
    Query(query): Query<StreamQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let container = stream_file_name
        .rsplit_once('.')
        .map_or(stream_file_name.as_str(), |(_, suffix)| suffix);
    stream_file(state, headers, item_id, Some(container), query, request).await
}

pub(crate) async fn universal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<UniversalQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let requested_user_id = query.user_id.filter(|user_id| !user_id.is_nil());
    let (target_user_id, target_policy, item) = match &identity {
        authentication::AuthenticatedIdentity::Device(session) => {
            let target_user_id = requested_user_id.unwrap_or(session.user.id);
            if target_user_id != session.user.id && !session.user.is_administrator {
                return Err(ApiError::Forbidden);
            }
            let policy: jellyfin_model::UserPolicy = if target_user_id == session.user.id {
                serde_json::from_value(session.user.policy.clone())
                    .map_err(|_| ApiError::Internal)?
            } else {
                serde_json::from_value(state.users.get(target_user_id).await?.policy)
                    .map_err(|_| ApiError::Internal)?
            };
            let item = state
                .user_library
                .item(&session.user, target_user_id, item_id)
                .await?;
            (Some(target_user_id), Some(policy), item)
        }
        // An API key has the default unrestricted controller authorization.
        // If it explicitly selects a user, use that user's normal library and
        // transcoding policy just like other playback endpoints do.
        authentication::AuthenticatedIdentity::ApiKey(_) => match requested_user_id {
            Some(target_user_id) => {
                let user = state.users.get(target_user_id).await?;
                let policy =
                    serde_json::from_value(user.policy.clone()).map_err(|_| ApiError::Internal)?;
                let item = state
                    .user_library
                    .item(&user, target_user_id, item_id)
                    .await?;
                (Some(target_user_id), Some(policy), item)
            }
            None => (
                None,
                None,
                state
                    .base_items
                    .get(item_id)
                    .await?
                    .ok_or(ApiError::NotFound)?,
            ),
        },
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let source_id = Uuid::parse_str(media_source_id).map_err(|_| ApiError::NotFound)?;
        if source_id == item_id {
            item
        } else {
            state
                .base_items
                .alternate_media_version(item_id, source_id)
                .await?
                .ok_or(ApiError::NotFound)?
        }
    } else {
        item
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = jellyfin_controller::media_source_path(&item).ok_or(ApiError::NotFound)?;
    let actual_container = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let streams = state
        .media_streams
        .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
        .await?;
    let supports_direct = supports_direct_play(&query, actual_container, &streams);
    let requires_transcode = universal_requires_transcode(&query);
    if supports_direct && !requires_transcode {
        if path.starts_with("http://") || path.starts_with("https://") {
            // The official controller only exposes an upstream URL when both
            // client capabilities opt in.  Otherwise Jellyfin remains the
            // streaming endpoint and proxies the authorized remote source.
            if should_redirect_remote_media(&query) {
                return Ok(Redirect::temporary(path).into_response());
            }
            return crate::videos::proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item.id,
                path,
                crate::videos::required_remote_user_agent(&item),
            )
            .await;
        }
        return serve_path(headers, path, request).await;
    }

    if target_policy.is_some_and(|policy| !policy.enable_audio_playback_transcoding) {
        return Err(ApiError::Forbidden);
    }

    if universal_uses_hls(&query) {
        let hls_query = crate::hls_segment::TranscodeQuery::universal_audio(
            query.media_source_id.clone(),
            query.device_id.clone(),
            target_user_id,
            query.audio_codec.clone(),
            query.audio_bitrate.or(query.max_streaming_bitrate),
            query.max_audio_channels,
            query.max_audio_sample_rate,
            query.audio_stream_index,
            query.start_time_ticks,
        );
        let hls_uri = hls_query.audio_master_uri(item_id, identity.access_token())?;
        return crate::hls_segment::ensure_master_playlist(
            &state, headers, &hls_uri, hls_query, &identity,
        )
        .await;
    }

    let codec = query.audio_codec.as_deref().unwrap_or("aac").to_owned();
    let container = query.transcoding_container.as_deref().map_or_else(
        || audio_container(&codec).to_owned(),
        |container| container.trim_start_matches('.').to_owned(),
    );
    let output = state.transcode_directory.join(format!(
        "{item_id}-audio-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(path),
        &output,
        &codec,
        query.audio_bitrate.or(query.max_streaming_bitrate),
        query
            .transcoding_audio_channels
            .or(query.max_audio_channels),
        query.max_audio_sample_rate,
        query.audio_stream_index,
        query.start_time_ticks,
        true,
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
}

pub(crate) async fn universal_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    Query(mut query): Query<UniversalQuery>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    if query.container.is_empty() {
        query.container.push(container);
    }
    universal(State(state), headers, Path(item_id), Query(query), request).await
}

fn universal_requires_transcode(query: &UniversalQuery) -> bool {
    query.audio_codec.is_some()
        || query.max_audio_channels.is_some()
        || query.transcoding_audio_channels.is_some()
        || query.audio_stream_index.is_some()
        || query.max_streaming_bitrate.is_some()
        || query.audio_bitrate.is_some()
        || query.max_audio_sample_rate.is_some()
        || query.start_time_ticks.is_some_and(|ticks| ticks != 0)
        || query.transcoding_container.is_some()
}

fn should_redirect_remote_media(query: &UniversalQuery) -> bool {
    query.enable_remote_media == Some(true) && query.enable_redirection == Some(true)
}

fn universal_uses_hls(query: &UniversalQuery) -> bool {
    query.transcoding_protocol == Some(jellyfin_model::MediaStreamProtocol::Hls)
}

/// Mirrors UniversalAudioController's DirectPlayProfile construction: each
/// comma-delimited `container` value is a `container|codec|codec` profile.
/// A profile without codecs permits the matching container. When scan metadata
/// identifies the selected audio codec, a codec-constrained profile must name
/// it before raw bytes can be sent to the player.
fn supports_direct_play(
    query: &UniversalQuery,
    actual_container: &str,
    streams: &[MediaStream],
) -> bool {
    let selected_stream = streams.iter().find(|stream| {
        stream.stream_type == MediaStreamType::Audio
            && query
                .audio_stream_index
                .is_none_or(|index| stream.index == index)
    });
    if selected_stream.is_some_and(|stream| {
        query
            ._max_audio_bit_depth
            .is_some_and(|maximum| stream.bit_depth.is_some_and(|depth| depth > maximum))
    }) {
        return false;
    }
    let selected_codec = selected_stream
        .and_then(|stream| stream.codec.as_deref())
        .map(str::trim)
        .filter(|codec| !codec.is_empty());

    query.container.iter().any(|profile| {
        let mut parts = profile
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty());
        let Some(container) = parts.next() else {
            return false;
        };
        if !container.eq_ignore_ascii_case(actual_container) {
            return false;
        }
        let codecs = parts.collect::<Vec<_>>();
        codecs.is_empty()
            || selected_codec.is_none()
            || selected_codec.is_some_and(|codec| {
                codecs
                    .iter()
                    .any(|supported| supported.eq_ignore_ascii_case(codec))
            })
    })
}

fn audio_container(codec: &str) -> &str {
    match codec.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "opus" => "opus",
        "aac" | "alac" => "m4a",
        "ac3" | "eac3" => "ac3",
        "wav" | "pcm_s16le" => "wav",
        _ => codec,
    }
}

fn codec_for_container(container: &str) -> &str {
    match container.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "flac" => "flac",
        "opus" | "ogg" => "opus",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "wav" | "wave" => "pcm_s16le",
        _ => "aac",
    }
}

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let mut requested_item = match identity {
        authentication::AuthenticatedIdentity::Device(authenticated) => {
            state
                .user_library
                .item(&authenticated.user, authenticated.user.id, item_id)
                .await?
        }
        // API keys use the same unrestricted default authorization policy as
        // the official stream controller. They have no device user whose
        // library policy can be applied before opening the selected source.
        authentication::AuthenticatedIdentity::ApiKey(_) => state
            .base_items
            .get(item_id)
            .await?
            .ok_or(ApiError::NotFound)?,
    };
    let item_types = jellyfin_controller::ItemTypeRegistry::default();
    let item_type = item_types
        .resolve(&requested_item.item_type)
        .ok_or(ApiError::NotFound)?;
    requested_item.item_type = item_type.name().to_owned();
    if requested_item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        crate::media_source::resolve_static_item(&state, requested_item, media_source_id).await?
    } else {
        requested_item
    };
    if item.item_type != "Audio" {
        return Err(ApiError::NotFound);
    }
    let path = jellyfin_controller::media_source_path(&item).ok_or(ApiError::NotFound)?;
    if let Some(container) = requested_container {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            return Err(ApiError::UnsupportedMediaType);
        }
    }
    if query.static_stream.unwrap_or(false) {
        if path.starts_with("http://") || path.starts_with("https://") {
            return crate::videos::proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item.id,
                path,
                crate::videos::required_remote_user_agent(&item),
            )
            .await;
        }
        return serve_path(headers, path, request).await;
    }

    let codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| codec_for_container(requested_container.unwrap_or("m4a")));
    let container = requested_container
        .filter(|container| !container.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| audio_container(codec).to_owned());
    let output = state.transcode_directory.join(format!(
        "{item_id}-audio-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let command = audio_command(
        &state.ffmpeg_path,
        std::path::Path::new(&path),
        &output,
        codec,
        query.audio_bitrate,
        query
            .audio_channels
            .or(query.max_audio_channels)
            .or(query.transcoding_max_audio_channels),
        query.audio_sample_rate,
        query.audio_stream_index,
        query.start_time_ticks,
        query.copy_timestamps.unwrap_or(false),
    );
    serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;
    use axum_extra::extract::Query;

    use super::{
        StreamQuery, UniversalQuery, should_redirect_remote_media, supports_direct_play,
        universal_requires_transcode, universal_uses_hls,
    };

    #[test]
    fn audio_stream_binds_alternate_media_source_id_case_insensitively() {
        for key in ["MediaSourceId", "mediaSourceId", "mediasourceid"] {
            let uri: Uri = format!("/Audio/item/stream?{key}=alternate")
                .parse()
                .unwrap();
            let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        }
    }

    #[test]
    fn audio_stream_binds_android_transcoding_parameters() {
        let uri: Uri = "/audio/item/stream?static=false&audioCodec=mp3&AudioBitrate=192000&audioSampleRate=44100&maxAudioChannels=2&audioStreamIndex=1&startTimeTicks=10000&CopyTimestamps=true"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert!(!query.static_stream.unwrap());
        assert_eq!(query.audio_codec.as_deref(), Some("mp3"));
        assert_eq!(query.audio_bitrate, Some(192000));
        assert_eq!(query.audio_sample_rate, Some(44100));
        assert_eq!(query.max_audio_channels, Some(2));
        assert_eq!(query.audio_stream_index, Some(1));
        assert_eq!(query.start_time_ticks, Some(10000));
        assert_eq!(query.copy_timestamps, Some(true));
    }

    #[test]
    fn universal_audio_binds_sdk_parameters_case_insensitively() {
        let uri: Uri = "/audio/item/universal?mediasourceid=alternate&transcodingAudioChannels=2&AudioBitRate=128000&maxAudioSampleRate=48000&maxAudioBitDepth=24&transcodingProtocol=hls&enableRemoteMedia=true&enableRedirection=false"
            .parse()
            .unwrap();
        let query = Query::<super::UniversalQuery>::try_from_uri(&uri)
            .unwrap()
            .0;
        assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        assert_eq!(query.transcoding_audio_channels, Some(2));
        assert_eq!(query.audio_bitrate, Some(128000));
        assert_eq!(query.max_audio_sample_rate, Some(48000));
        assert_eq!(query._max_audio_bit_depth, Some(24));
        assert_eq!(
            query.transcoding_protocol,
            Some(jellyfin_model::MediaStreamProtocol::Hls)
        );
        assert_eq!(query.enable_remote_media, Some(true));
        assert_eq!(query.enable_redirection, Some(false));
    }

    #[test]
    fn universal_audio_honors_camel_case_audio_bitrate() {
        // ASP.NET binds `audioBitRate` case-insensitively, so clients using the conventional
        // camel-case spelling must not fall through to direct playback.
        let uri: Uri = "/audio/item/universal?audioBitrate=128000".parse().unwrap();
        let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.audio_bitrate, Some(128_000));
        assert!(universal_requires_transcode(&query));
    }

    #[test]
    fn universal_audio_redirects_remote_media_only_when_both_capabilities_opt_in() {
        for (query_string, expected) in [
            ("", false),
            ("enableRemoteMedia=true", false),
            ("enableRedirection=true", false),
            ("enableRemoteMedia=true&enableRedirection=true", true),
        ] {
            let uri: Uri = format!("/audio/item/universal?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(
                should_redirect_remote_media(&query),
                expected,
                "{query_string}"
            );
        }
    }

    #[test]
    fn universal_audio_selects_hls_only_when_requested() {
        for (query_string, expected) in [
            ("", false),
            ("transcodingProtocol=http", false),
            ("transcodingProtocol=hls", true),
        ] {
            let uri: Uri = format!("/audio/item/universal?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(universal_uses_hls(&query), expected, "{query_string}");
        }
    }

    #[test]
    fn universal_audio_direct_play_honors_profile_audio_codecs() {
        let stream = jellyfin_model::MediaStream {
            stream_type: jellyfin_model::MediaStreamType::Audio,
            index: 0,
            codec: Some("flac".to_owned()),
            ..Default::default()
        };
        for (profile, expected) in [
            ("flac", true),
            ("flac|flac", true),
            ("flac|aac", false),
            ("mp3|flac", false),
        ] {
            let uri: Uri = format!("/audio/item/universal?container={profile}")
                .parse()
                .unwrap();
            let query = Query::<UniversalQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(
                supports_direct_play(&query, "flac", &[stream.clone()]),
                expected,
                "{profile}"
            );
        }
    }

    #[test]
    fn progressive_audio_honors_camel_case_audio_bitrate() {
        let uri: Uri = "/audio/item/stream?audioBitrate=192000".parse().unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.audio_bitrate, Some(192_000));
    }
}

pub(crate) async fn serve_path(
    mut headers: HeaderMap,
    path: &str,
    mut request: Request<Body>,
) -> Result<Response, ApiError> {
    request.headers_mut().clear();
    for name in [
        axum::http::header::RANGE,
        axum::http::header::IF_RANGE,
        axum::http::header::IF_MODIFIED_SINCE,
        axum::http::header::IF_UNMODIFIED_SINCE,
    ] {
        if let Some(value) = headers.remove(&name) {
            request.headers_mut().insert(name, value);
        }
    }
    let response = match ServeFile::new(path)
        .with_buf_chunk_size(64 * 1024)
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(error) => match error {},
    };
    Ok(response.map(Body::new))
}

/// Starts a progressive FFmpeg job and streams the output as it grows.
///
/// Jellyfin's progressive endpoints return the response before FFmpeg has
/// completed. Android's ExoPlayer relies on that behavior for long files.
pub(crate) async fn serve_transcoded_path(
    command: FfmpegCommand,
    output_path: &str,
    is_head: bool,
) -> Result<Response, ApiError> {
    let content_type = jellyfin_model::MimeTypes::get_mime_type(output_path)
        .unwrap_or_else(|_| "application/octet-stream".to_owned());
    let response = Response::builder()
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::ACCEPT_RANGES, "none");
    if is_head {
        return response.body(Body::empty()).map_err(|_| ApiError::Internal);
    }

    let child = Command::new(&command.program)
        .args(&command.arguments)
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ApiError::Internal)?;
    let state = ProgressiveTranscodeState {
        child,
        output_path: PathBuf::from(output_path),
        file: None,
    };
    let body = Body::from_stream(stream::unfold(state, next_transcode_chunk));
    response.body(body).map_err(|_| ApiError::Internal)
}

struct ProgressiveTranscodeState {
    child: tokio::process::Child,
    output_path: PathBuf,
    file: Option<tokio::fs::File>,
}

async fn next_transcode_chunk(
    mut state: ProgressiveTranscodeState,
) -> Option<(Result<Bytes, io::Error>, ProgressiveTranscodeState)> {
    loop {
        if state.file.is_none() {
            match tokio::fs::File::open(&state.output_path).await {
                Ok(file) => state.file = Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if let Some(status) = state.child.try_wait().ok().flatten() {
                        return Some((
                            Err(if status.success() {
                                io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "FFmpeg exited without producing output",
                                )
                            } else {
                                io::Error::other(format!("FFmpeg exited with {status}"))
                            }),
                            state,
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    continue;
                }
                Err(error) => return Some((Err(error), state)),
            }
        }

        let mut buffer = vec![0_u8; 64 * 1024];
        let read = state
            .file
            .as_mut()
            .expect("progressive output file is open")
            .read(&mut buffer)
            .await;
        match read {
            Ok(length) if length > 0 => {
                buffer.truncate(length);
                return Some((Ok(Bytes::from(buffer)), state));
            }
            Ok(_) => match state.child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    let _ = tokio::fs::remove_file(&state.output_path).await;
                    return None;
                }
                Ok(Some(status)) => {
                    let _ = tokio::fs::remove_file(&state.output_path).await;
                    return Some((
                        Err(io::Error::other(format!("FFmpeg exited with {status}"))),
                        state,
                    ));
                }
                Ok(None) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => return Some((Err(error), state)),
            },
            Err(error) => return Some((Err(error), state)),
        }
    }
}
