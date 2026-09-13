use std::{fmt::Write as _, sync::Arc};

use axum::{
    Json,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_controller::{
    FfmpegCommand, embedded_subtitle_filter_index, video_command, video_remux_command,
};
use jellyfin_data::BaseItemPage;
use jellyfin_model::{
    EncodingContext, MediaStream, MediaStreamType, SubtitleDeliveryMethod, TranscodeReason,
    VideoRangeType,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authentication, encoding_runtime, stream_options::StreamOptions,
    user_library,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MergeVersionsQuery {
    #[serde(
        default,
        rename = "ids",
        alias = "Ids",
        deserialize_with = "crate::query::comma::deserialize"
    )]
    ids: Vec<Uuid>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct StreamQuery {
    // `container` is a query parameter on the extensionless official route.
    // The by-container route receives it from the path instead.
    #[serde(rename = "container", alias = "Container")]
    container: Option<String>,
    #[serde(rename = "static", alias = "Static")]
    static_stream: Option<bool>,
    #[serde(rename = "params", alias = "Params")]
    params: Option<String>,
    #[serde(rename = "tag", alias = "Tag")]
    tag: Option<String>,
    #[serde(
        rename = "deviceProfileId",
        alias = "DeviceProfileId",
        alias = "deviceprofileid"
    )]
    _device_profile_id: Option<String>,
    #[serde(
        rename = "playSessionId",
        alias = "PlaySessionId",
        alias = "playsessionid"
    )]
    play_session_id: Option<String>,
    #[serde(
        rename = "segmentContainer",
        alias = "SegmentContainer",
        alias = "segmentcontainer"
    )]
    segment_container: Option<String>,
    #[serde(
        rename = "segmentLength",
        alias = "SegmentLength",
        alias = "segmentlength"
    )]
    _segment_length: Option<i32>,
    #[serde(rename = "minSegments", alias = "MinSegments", alias = "minsegments")]
    _min_segments: Option<i32>,
    #[serde(
        rename = "mediaSourceId",
        alias = "MediaSourceId",
        alias = "mediasourceid"
    )]
    media_source_id: Option<String>,
    #[serde(rename = "deviceId", alias = "DeviceId", alias = "deviceid")]
    device_id: Option<String>,
    #[serde(
        rename = "enableAutoStreamCopy",
        alias = "EnableAutoStreamCopy",
        alias = "enableautostreamcopy"
    )]
    enable_auto_stream_copy: Option<bool>,
    #[serde(
        rename = "allowVideoStreamCopy",
        alias = "AllowVideoStreamCopy",
        alias = "allowvideostreamcopy"
    )]
    allow_video_stream_copy: Option<bool>,
    #[serde(
        rename = "allowAudioStreamCopy",
        alias = "AllowAudioStreamCopy",
        alias = "allowaudiostreamcopy"
    )]
    allow_audio_stream_copy: Option<bool>,
    #[serde(rename = "videoCodec", alias = "VideoCodec", alias = "videocodec")]
    video_codec: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "videoBitRate",
        alias = "VideoBitRate",
        alias = "VideoBitrate",
        alias = "videoBitrate",
        alias = "videobitrate"
    )]
    video_bitrate: Option<i32>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
        alias = "AudioBitrate",
        alias = "audioBitrate",
        alias = "audiobitrate"
    )]
    audio_bitrate: Option<i32>,
    #[serde(
        rename = "audioSampleRate",
        alias = "AudioSampleRate",
        alias = "audiosamplerate"
    )]
    audio_sample_rate: Option<i32>,
    #[serde(
        rename = "maxAudioBitDepth",
        alias = "MaxAudioBitDepth",
        alias = "maxaudiobitdepth"
    )]
    max_audio_bit_depth: Option<i32>,
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
    #[serde(rename = "profile", alias = "Profile")]
    profile: Option<String>,
    #[serde(rename = "level", alias = "Level")]
    level: Option<String>,
    #[serde(rename = "framerate", alias = "Framerate")]
    framerate: Option<f32>,
    #[serde(
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
    #[serde(rename = "maxWidth", alias = "MaxWidth", alias = "maxwidth")]
    max_width: Option<i32>,
    #[serde(rename = "maxHeight", alias = "MaxHeight", alias = "maxheight")]
    max_height: Option<i32>,
    #[serde(rename = "width", alias = "Width")]
    width: Option<i32>,
    #[serde(rename = "height", alias = "Height")]
    height: Option<i32>,
    #[serde(
        rename = "maxFramerate",
        alias = "MaxFramerate",
        alias = "maxframerate"
    )]
    max_framerate: Option<f32>,
    #[serde(
        rename = "audioStreamIndex",
        alias = "AudioStreamIndex",
        alias = "audiostreamindex"
    )]
    audio_stream_index: Option<i32>,
    #[serde(
        rename = "videoStreamIndex",
        alias = "VideoStreamIndex",
        alias = "videostreamindex"
    )]
    video_stream_index: Option<i32>,
    #[serde(
        rename = "subtitleStreamIndex",
        alias = "SubtitleStreamIndex",
        alias = "subtitlestreamindex"
    )]
    subtitle_stream_index: Option<i32>,
    #[serde(
        default,
        rename = "subtitleMethod",
        alias = "SubtitleMethod",
        alias = "subtitlemethod",
        deserialize_with = "crate::query::optional_subtitle_delivery_method"
    )]
    subtitle_method: Option<SubtitleDeliveryMethod>,
    #[serde(skip)]
    legacy_subtitle_method_unknown: bool,
    #[serde(
        rename = "maxRefFrames",
        alias = "MaxRefFrames",
        alias = "maxrefframes"
    )]
    max_ref_frames: Option<i32>,
    #[serde(
        rename = "maxVideoBitDepth",
        alias = "MaxVideoBitDepth",
        alias = "maxvideobitdepth"
    )]
    max_video_bit_depth: Option<i32>,
    #[serde(rename = "requireAvc", alias = "RequireAvc", alias = "requireavc")]
    require_avc: Option<bool>,
    #[serde(rename = "deInterlace", alias = "DeInterlace", alias = "deinterlace")]
    de_interlace: Option<bool>,
    #[serde(
        rename = "requireNonAnamorphic",
        alias = "RequireNonAnamorphic",
        alias = "requirenonanamorphic"
    )]
    require_non_anamorphic: Option<bool>,
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
    #[serde(
        rename = "cpuCoreLimit",
        alias = "CpuCoreLimit",
        alias = "cpucorelimit"
    )]
    cpu_core_limit: Option<i32>,
    #[serde(
        rename = "liveStreamId",
        alias = "LiveStreamId",
        alias = "livestreamid"
    )]
    live_stream_id: Option<String>,
    #[serde(
        rename = "enableMpegtsM2TsMode",
        alias = "EnableMpegtsM2TsMode",
        alias = "enablempegtsm2tsmode"
    )]
    enable_mpegts_m2_ts_mode: Option<bool>,
    #[serde(
        rename = "subtitleCodec",
        alias = "SubtitleCodec",
        alias = "subtitlecodec"
    )]
    subtitle_codec: Option<String>,
    #[serde(
        rename = "transcodeReasons",
        alias = "TranscodeReasons",
        alias = "transcodereasons"
    )]
    transcode_reasons: Option<String>,
    #[serde(
        default,
        rename = "context",
        alias = "Context",
        deserialize_with = "crate::query::optional_encoding_context"
    )]
    context: Option<EncodingContext>,
    #[serde(
        default,
        rename = "streamOptions",
        alias = "StreamOptions",
        alias = "streamoptions"
    )]
    _stream_options: Vec<String>,
    #[serde(
        rename = "enableAudioVbrEncoding",
        alias = "EnableAudioVbrEncoding",
        alias = "enableaudiovbrencoding"
    )]
    enable_audio_vbr_encoding: Option<bool>,
}

pub(crate) async fn stream(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    query: Result<Query<StreamQuery>, axum_extra::extract::QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, None, query, request).await
}

pub(crate) async fn stream_with_container(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, container)): Path<(Uuid, String)>,
    query: Result<Query<StreamQuery>, axum_extra::extract::QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    stream_file(state, headers, item_id, Some(&container), query, request).await
}

/// Compatibility endpoint used by older Emby clients (`/Videos/{id}/{name}`).
pub(crate) async fn stream_with_file_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, stream_file_name)): Path<(Uuid, String)>,
    query: Result<Query<StreamQuery>, axum_extra::extract::QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let container = stream_file_name
        .rsplit_once('.')
        .map_or(stream_file_name.as_str(), |(_, suffix)| suffix);
    stream_file(state, headers, item_id, Some(container), query, request).await
}

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: Result<Query<StreamQuery>, axum_extra::extract::QueryRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let identity =
        authentication::authenticated_identity(&state, &headers, Some(request.uri())).await?;
    let stream_options = StreamOptions::from_uri(request.uri());
    let Query(mut query) = query.map_err(|_| ApiError::InvalidRequest)?;
    validate_progressive_query(requested_container, &query)?;
    apply_legacy_params(&mut query)?;
    let (mut requested_item, device_policy) = match identity {
        authentication::AuthenticatedIdentity::Device(authenticated) => {
            let policy: jellyfin_model::UserPolicy =
                serde_json::from_value(authenticated.user.policy.clone())
                    .map_err(|_| ApiError::Internal)?;
            (
                state
                    .user_library
                    .item(&authenticated.user, authenticated.user.id, item_id)
                    .await?,
                Some(policy),
            )
        }
        // The official default authorization handler treats API keys as an
        // unrestricted principal. Unlike a device session, they have no
        // target-user library policy to apply before opening a stream.
        authentication::AuthenticatedIdentity::ApiKey(_) => (
            state
                .base_items
                .get(item_id)
                .await?
                .ok_or(ApiError::NotFound)?,
            None,
        ),
    };
    // Device sessions are hydrated by LibraryController. API keys load the
    // unrestricted persisted row directly, so normalize official CLR aliases
    // before applying this route's supported-video-type check.
    let item_types = jellyfin_controller::ItemTypeRegistry::default();
    let item_type = item_types
        .resolve(&requested_item.item_type)
        .ok_or(ApiError::NotFound)?;
    requested_item.item_type = item_type.name().to_owned();
    if !matches!(
        requested_item.item_type.as_str(),
        "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
    ) {
        return Err(ApiError::NotFound);
    }
    let opened_source = query
        .live_stream_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|live_stream_id| {
            state
                .live_streams
                .get(item_id, live_stream_id)
                .ok_or(ApiError::NotFound)
        })
        .transpose()?;
    let item = if opened_source.is_some() {
        requested_item
    } else if let Some(media_source_id) = query
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let version_id = Uuid::parse_str(media_source_id).map_err(|_| ApiError::NotFound)?;
        if version_id == requested_item.id {
            requested_item
        } else {
            state
                .base_items
                .alternate_video_version(requested_item.id, version_id)
                .await?
                .ok_or(ApiError::NotFound)?
        }
    } else {
        requested_item
    };
    let path = opened_source
        .as_ref()
        .and_then(|source| source.path.clone())
        .or_else(|| jellyfin_controller::media_source_path(&item).map(str::to_owned))
        .ok_or(ApiError::NotFound)?;
    if let Some(container) = requested_container {
        let actual = std::path::Path::new(&path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        if !container.eq_ignore_ascii_case(actual) {
            // Some Jellyfin clients (including VidHub) use `stream.mp4` as a
            // generic static-stream endpoint even after selecting a direct
            // play MKV source. The route suffix is not the source of truth;
            // ServeFile derives the response MIME type from the real path.
            tracing::warn!(
                %item_id,
                requested_container = container,
                actual_container = actual,
                "static video route container differs from the media source",
            );
        }
    }
    if query.static_stream.unwrap_or(false) {
        if path
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || path
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            return proxy_remote_stream(
                &state.remote_stream_client,
                &headers,
                item_id,
                &path,
                required_remote_user_agent(&item),
            )
            .await;
        }
        return crate::audio::serve_path(headers, &path, request).await;
    }

    let container = output_container(requested_container, query.container.as_deref());
    let video_codec = query
        .video_codec
        .as_deref()
        .unwrap_or_else(|| video_codec_for_container(&container));
    let audio_codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| audio_codec_for_container(&container));
    let media_streams = if let Some(source) = opened_source.as_ref() {
        source.media_streams.clone()
    } else {
        state
            .media_streams
            .get_media_streams(jellyfin_controller::MediaStreamFilter::for_item(item.id))
            .await?
    };
    let selected_subtitle = query.subtitle_stream_index.and_then(|index| {
        media_streams
            .iter()
            .find(|stream| stream.stream_type == MediaStreamType::Subtitle && stream.index == index)
    });
    let subtitle_method = effective_subtitle_method(&query, selected_subtitle);
    let burns_subtitle = query.subtitle_stream_index.is_some()
        && should_burn_subtitles(subtitle_method, query.legacy_subtitle_method_unknown);
    let external_text_subtitle_filter = burns_subtitle
        .then_some(selected_subtitle)
        .flatten()
        .filter(|stream| stream.is_external && stream.is_text_subtitle_stream())
        .and_then(|stream| stream.path.as_deref());
    let external_graphical_subtitle = external_graphical_subtitle_request(
        &media_streams,
        selected_subtitle,
        burns_subtitle,
        &query,
    );
    let burns_text_subtitle =
        burns_subtitle && selected_subtitle.is_some_and(MediaStream::is_text_subtitle_stream);
    let subtitle_filter_index = if burns_subtitle
        && external_text_subtitle_filter.is_none()
        && external_graphical_subtitle.is_none()
    {
        query
            .subtitle_stream_index
            .and_then(|index| embedded_subtitle_filter_index(&media_streams, index))
    } else {
        None
    };
    let embedded_subtitle =
        embedded_subtitle_request(&query, &media_streams, selected_subtitle, subtitle_method);
    let output = state.transcode_directory.join(format!(
        "{item_id}-video-{}.{}",
        Uuid::new_v4().simple(),
        container
    ));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let copy_timestamps = query.copy_timestamps.unwrap_or(false);
    let source_video = selected_stream(
        &media_streams,
        MediaStreamType::Video,
        query.video_stream_index,
    );
    let source_audio = selected_stream(
        &media_streams,
        MediaStreamType::Audio,
        query.audio_stream_index,
    );
    let requested_profile = query
        .profile
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| stream_options.get_request_option(video_codec, "profile"));
    let requested_level = query
        .level
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| stream_options.get_request_option(video_codec, "level"));
    let de_interlace = source_video.is_some_and(|stream| stream.is_interlaced)
        && (query.de_interlace.unwrap_or(false)
            || stream_options
                .get_request_option(video_codec, "deinterlace")
                .is_some_and(|value| value.eq_ignore_ascii_case("true")));
    let mut is_local_copy_remux = if subtitle_filter_index.is_none()
        && is_local_path(&path)
        && copy_remux_has_no_transform_with_deinterlace(&query, de_interlace)
    {
        can_copy_remux_with_options(
            &query,
            &stream_options,
            &container,
            &media_streams,
            video_codec,
            audio_codec,
        )
    } else {
        false
    };
    if let Some(policy) = device_policy {
        if is_local_copy_remux && !policy.enable_playback_remuxing {
            is_local_copy_remux = false;
        }
        if !is_local_copy_remux
            && !progressive_codecs_allowed(
                video_codec,
                audio_codec,
                policy.enable_video_playback_transcoding,
                policy.enable_audio_playback_transcoding,
            )
        {
            return Err(ApiError::Forbidden);
        }
    }
    let video_profile = output_video_profile(video_codec, requested_profile);
    let video_level = output_video_level(video_codec, requested_level);
    let option_channels = stream_options
        .get_request_option(audio_codec, "audiochannels")
        .and_then(|value| value.parse().ok());
    let audio_channels = source_audio.and_then(|source| {
        encoding_runtime::output_audio_channels(
            audio_codec,
            source.channels,
            option_channels,
            query.max_audio_channels,
            query.audio_channels,
            query.transcoding_max_audio_channels,
        )
    });
    let audio_bitrate = encoding_runtime::output_audio_bitrate(
        audio_codec,
        source_audio.is_some(),
        source_audio.and_then(|source| source.channels),
        audio_channels,
        query.audio_bitrate.map(i64::from),
    );
    let transcodes_audio = !audio_codec.eq_ignore_ascii_case("copy");
    let encoding_options = crate::configuration::encoding_runtime_options(&state).await?;
    let mut command = if is_local_copy_remux {
        video_remux_command(
            &state.ffmpeg_path,
            std::path::Path::new(&path),
            &output,
            query.audio_stream_index,
            query.video_stream_index,
            query.start_time_ticks,
            copy_timestamps,
        )
    } else {
        video_command(
            &state.ffmpeg_path,
            std::path::Path::new(&path),
            &output,
            video_codec,
            audio_codec,
            query.video_bitrate.map(i64::from),
            audio_bitrate,
            transcodes_audio.then_some(audio_channels).flatten(),
            transcodes_audio
                .then_some(query.audio_sample_rate)
                .flatten(),
            query.max_width.or(query.width),
            query.max_height.or(query.height),
            query.framerate.or(query.max_framerate),
            de_interlace,
            query.require_non_anamorphic.unwrap_or(false),
            video_profile.as_deref(),
            video_level.as_deref(),
            query.audio_stream_index,
            query.video_stream_index,
            query.start_time_ticks,
            subtitle_filter_index,
            copy_timestamps,
        )
    };
    if let Some(subtitle) = external_graphical_subtitle.as_ref() {
        apply_external_graphical_subtitle_burn(
            &mut command,
            subtitle,
            query.start_time_ticks,
            copy_timestamps,
        );
    } else if let Some(path) = external_text_subtitle_filter {
        apply_external_text_subtitle_burn(&mut command, std::path::Path::new(path));
    }
    if burns_text_subtitle && !copy_timestamps {
        apply_subtitle_time_offset(&mut command, query.start_time_ticks);
    }
    if let Some(subtitle) = embedded_subtitle.as_ref() {
        apply_embedded_subtitle(&mut command, subtitle, query.start_time_ticks);
    }
    apply_encoding_context(&mut command, query.context);
    encoding_runtime::apply_audio_vbr(
        &mut command,
        audio_codec,
        audio_bitrate,
        audio_channels,
        encoding_runtime::audio_vbr_enabled(
            encoding_options.enable_audio_vbr,
            query.enable_audio_vbr_encoding,
        ),
    );
    encoding_runtime::apply_thread_count(
        &mut command,
        query.cpu_core_limit,
        encoding_options.encoding_thread_count,
    );
    apply_mpegts_m2_ts_mode(
        &mut command,
        query.enable_mpegts_m2_ts_mode.unwrap_or(false),
    );
    crate::audio::serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
        Arc::clone(&state.transcode_jobs),
        query.device_id,
        query.play_session_id,
        is_local_copy_remux || video_codec.eq_ignore_ascii_case("copy"),
        is_local_copy_remux || audio_codec.eq_ignore_ascii_case("copy"),
        query
            .transcode_reasons
            .as_deref()
            .and_then(TranscodeReason::parse_names)
            .unwrap_or(TranscodeReason::NONE),
    )
}

#[cfg(test)]
pub(crate) fn apply_cpu_core_limit(command: &mut FfmpegCommand, requested: Option<i32>) {
    if requested.is_some() {
        encoding_runtime::apply_thread_count(command, requested, -1);
    }
}

fn apply_mpegts_m2_ts_mode(command: &mut FfmpegCommand, enabled: bool) {
    if !enabled {
        return;
    }
    let output_index = command.arguments.len().saturating_sub(1);
    command.arguments.splice(
        output_index..output_index,
        ["-mpegts_m2ts_mode".to_owned(), "1".to_owned()],
    );
}

fn apply_encoding_context(command: &mut FfmpegCommand, context: Option<EncodingContext>) {
    if context != Some(EncodingContext::Static) {
        return;
    }
    let mut index = 0;
    while index + 1 < command.arguments.len() {
        let option = command.arguments[index].as_str();
        let value = command.arguments[index + 1].as_str();
        if (option == "-f" && value == "mp4")
            || (option == "-movflags" && value == "frag_keyframe+empty_moov+delay_moov")
        {
            command.arguments.drain(index..=index + 1);
        } else {
            index += 1;
        }
    }
}

fn validate_progressive_query(
    route_container: Option<&str>,
    query: &StreamQuery,
) -> Result<(), ApiError> {
    for value in [
        route_container,
        query.container.as_deref(),
        query.segment_container.as_deref(),
        query.audio_codec.as_deref(),
        query.video_codec.as_deref(),
        query.subtitle_codec.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !crate::audio::valid_encoding_name(value) {
            return Err(ApiError::InvalidRequest);
        }
    }
    if query
        .level
        .as_deref()
        .is_some_and(|value| !crate::audio::valid_encoding_level(value))
    {
        return Err(ApiError::InvalidRequest);
    }
    Ok(())
}

fn apply_legacy_params(query: &mut StreamQuery) -> Result<(), ApiError> {
    let Some(params) = query.params.clone().filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    for (index, value) in params.split(';').enumerate() {
        if value.trim().is_empty() {
            continue;
        }
        match index {
            1 => query.device_id = Some(value.to_owned()),
            2 => query.media_source_id = Some(value.to_owned()),
            3 => query.static_stream = Some(value.eq_ignore_ascii_case("true")),
            4 if crate::audio::valid_encoding_name(value) => {
                query.video_codec = Some(value.to_owned());
            }
            5 if crate::audio::valid_encoding_name(value) => {
                query.audio_codec = Some(value.to_owned());
            }
            6 => {
                query.audio_stream_index =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            7 => {
                query.subtitle_stream_index =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            8 => {
                query.video_bitrate = Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            9 => {
                query.audio_bitrate = Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            10 => {
                query.max_audio_channels =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            11 => {
                query.max_framerate = Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            12 => query.max_width = Some(value.trim().parse().map_err(|_| ApiError::Internal)?),
            13 => query.max_height = Some(value.trim().parse().map_err(|_| ApiError::Internal)?),
            14 => {
                query.start_time_ticks =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            // The controller's direct query parameter is validated as a
            // complete value. The legacy packed parser instead calls the
            // unanchored Regex.IsMatch API, whose pattern succeeds whenever
            // the string contains an ASCII digit.
            15 if value.bytes().any(|byte| byte.is_ascii_digit()) => {
                query.level = Some(value.to_owned());
            }
            16 => {
                query.max_ref_frames = Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            17 => {
                query.max_video_bit_depth =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            18 if crate::audio::valid_encoding_name(value) => {
                query.profile = Some(value.to_owned());
            }
            20 => query.play_session_id = Some(value.to_owned()),
            22 => query.live_stream_id = Some(value.to_owned()),
            24 => query.copy_timestamps = Some(value.eq_ignore_ascii_case("true")),
            25 => {
                let value = value.trim();
                query.subtitle_method = match value {
                    "Encode" | "0" => Some(SubtitleDeliveryMethod::Encode),
                    "Embed" | "1" => Some(SubtitleDeliveryMethod::Embed),
                    "External" | "2" => Some(SubtitleDeliveryMethod::External),
                    "Hls" | "3" => Some(SubtitleDeliveryMethod::Hls),
                    "Drop" | "4" => Some(SubtitleDeliveryMethod::Drop),
                    _ if value.parse::<i32>().is_ok() => {
                        // Enum.TryParse accepts any representable underlying
                        // integer. Preserve its effective no-subtitle behavior
                        // without exposing an invalid public model enum.
                        query.legacy_subtitle_method_unknown = true;
                        None
                    }
                    _ => query.subtitle_method,
                };
            }
            26 => {
                query.transcoding_max_audio_channels =
                    Some(value.trim().parse().map_err(|_| ApiError::Internal)?);
            }
            28 => query.tag = Some(value.to_owned()),
            29 => query.require_avc = Some(value.eq_ignore_ascii_case("true")),
            30 if crate::audio::valid_encoding_name(value) => {
                query.subtitle_codec = Some(value.to_owned());
            }
            31 => query.require_non_anamorphic = Some(value.eq_ignore_ascii_case("true")),
            32 => query.de_interlace = Some(value.eq_ignore_ascii_case("true")),
            33 => query.transcode_reasons = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct EmbeddedSubtitleRequest {
    map: String,
    codec: String,
    external_path: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ExternalGraphicalSubtitleRequest {
    path: String,
    stream_index: usize,
    canvas_size: Option<(i32, i32)>,
    preprocess_filter: Option<String>,
}

fn external_graphical_subtitle_request(
    streams: &[MediaStream],
    selected: Option<&MediaStream>,
    burns_subtitle: bool,
    query: &StreamQuery,
) -> Option<ExternalGraphicalSubtitleRequest> {
    // Official Jellyfin opens external graphical subtitles as FFmpeg input 1;
    // text subtitles instead remain file-backed inputs to the libass filter.
    let selected = selected.filter(|stream| {
        burns_subtitle && stream.is_external && !stream.is_text_subtitle_stream()
    })?;
    let path = selected.path.as_deref()?;
    let stream_index = streams
        .iter()
        .filter(|candidate| candidate.path.as_deref() == Some(path))
        .take_while(|candidate| candidate.index != selected.index)
        .count();
    let canvas_size = match (selected.width, selected.height) {
        (Some(width), Some(height))
            if width > 0
                && height > 0
                && !selected
                    .codec
                    .as_deref()
                    .is_some_and(|codec| codec.eq_ignore_ascii_case("dvbsub")) =>
        {
            Some((width, height))
        }
        _ => None,
    };
    let video = selected_stream(streams, MediaStreamType::Video, query.video_stream_index);
    let output_size = fixed_output_size(
        video.and_then(|stream| stream.width),
        video.and_then(|stream| stream.height),
        query.max_width.or(query.width),
        query.max_height.or(query.height),
    );
    let preprocess_filter = output_size.map(|(width, height)| {
        let same_aspect_ratio = selected
            .width
            .zip(selected.height)
            .filter(|(subtitle_width, subtitle_height)| {
                *subtitle_width > 0 && *subtitle_height > 0
            })
            .is_some_and(|(subtitle_width, subtitle_height)| {
                let video_ratio = f64::from(width) / f64::from(height);
                let subtitle_ratio = f64::from(subtitle_width) / f64::from(subtitle_height);
                (video_ratio - subtitle_ratio).abs() < 0.01
            });
        if same_aspect_ratio {
            format!("scale,scale={width}:{height}:fast_bilinear")
        } else {
            format!(
                "scale,scale=-1:{height}:fast_bilinear,crop,pad=max({width}\\,iw):max({height}\\,ih):(ow-iw)/2:(oh-ih)/2:black@0,crop={width}:{height}"
            )
        }
    });

    Some(ExternalGraphicalSubtitleRequest {
        path: preferred_vobsub_path(path),
        stream_index,
        canvas_size,
        preprocess_filter,
    })
}

fn embedded_subtitle_request(
    query: &StreamQuery,
    streams: &[MediaStream],
    selected: Option<&MediaStream>,
    method: Option<SubtitleDeliveryMethod>,
) -> Option<EmbeddedSubtitleRequest> {
    if query.legacy_subtitle_method_unknown || method != Some(SubtitleDeliveryMethod::Embed) {
        return None;
    }
    let selected_index = query.subtitle_stream_index?;
    if selected.is_none() && !streams.is_empty() {
        return None;
    }
    let requested_codec = query
        .subtitle_codec
        .as_deref()
        .and_then(|value| value.split(',').find(|codec| !codec.is_empty()));
    let codec = match (
        requested_codec,
        selected.and_then(|stream| stream.codec.as_deref()),
    ) {
        (Some(requested), Some(source)) if requested.eq_ignore_ascii_case(source) => "copy",
        (Some(requested), _) => requested,
        (None, _) => "copy",
    }
    .to_owned();

    if let Some(stream) = selected.filter(|stream| stream.is_external) {
        let path = stream.path.as_deref()?;
        let in_file_index = streams
            .iter()
            .filter(|candidate| candidate.path.as_deref() == Some(path))
            .take_while(|candidate| candidate.index != stream.index)
            .count();
        return Some(EmbeddedSubtitleRequest {
            map: format!("1:{in_file_index}"),
            codec,
            external_path: Some(path.to_owned()),
        });
    }

    Some(EmbeddedSubtitleRequest {
        map: format!("0:{selected_index}"),
        codec,
        external_path: None,
    })
}

fn effective_subtitle_method(
    query: &StreamQuery,
    selected: Option<&MediaStream>,
) -> Option<SubtitleDeliveryMethod> {
    if query.legacy_subtitle_method_unknown {
        return None;
    }
    let method = query.subtitle_method.unwrap_or_default();
    if method == SubtitleDeliveryMethod::Embed
        && selected
            .and_then(|stream| stream.codec.as_deref())
            .is_some_and(|codec| codec.eq_ignore_ascii_case("dvbsub"))
    {
        Some(SubtitleDeliveryMethod::Encode)
    } else {
        Some(method)
    }
}

fn apply_external_text_subtitle_burn(command: &mut FfmpegCommand, path: &std::path::Path) {
    let escaped = escape_subtitle_filter_path(path);
    let filter = format!("subtitles='{escaped}'");
    if let Some(filter_index) = command
        .arguments
        .iter()
        .position(|argument| argument == "-vf")
        .and_then(|index| index.checked_add(1))
        .filter(|index| *index < command.arguments.len())
    {
        command.arguments[filter_index].push(',');
        command.arguments[filter_index].push_str(&filter);
        return;
    }
    let output_index = command.arguments.len().saturating_sub(1);
    command
        .arguments
        .splice(output_index..output_index, ["-vf".to_owned(), filter]);
}

fn apply_external_graphical_subtitle_burn(
    command: &mut FfmpegCommand,
    subtitle: &ExternalGraphicalSubtitleRequest,
    start_time_ticks: Option<i64>,
    copy_timestamps: bool,
) {
    let Some(video_map_index) = command
        .arguments
        .windows(2)
        .position(|pair| pair[0] == "-map" && pair[1].starts_with("0:"))
    else {
        return;
    };
    let video_input = command.arguments[video_map_index + 1].clone();
    let mut input = Vec::with_capacity(6);
    // Seek the subtitle input independently to the same progressive start.
    // Graphical streams already share the resulting zero-based timeline, so
    // unlike text subtitles they must not receive an additional setpts shift.
    if let Some(ticks) = start_time_ticks.filter(|ticks| *ticks > 0) {
        input.extend(["-ss".to_owned(), format_ticks_as_seconds(ticks)]);
    }
    if let Some((width, height)) = subtitle.canvas_size {
        input.extend(["-canvas_size".to_owned(), format!("{width}x{height}")]);
    }
    input.extend(["-i".to_owned(), format!("file:{}", subtitle.path)]);
    command
        .arguments
        .splice(video_map_index..video_map_index, input);

    let main_filter = command
        .arguments
        .iter()
        .position(|argument| argument == "-vf")
        .filter(|index| index + 1 < command.arguments.len())
        .map(|index| {
            let filter = command.arguments[index + 1].clone();
            command.arguments.drain(index..=index + 1);
            filter
        });
    let subtitle_input = format!("1:{}", subtitle.stream_index);
    let subtitle_chain = subtitle
        .preprocess_filter
        .as_deref()
        .map(|filter| format!("[{subtitle_input}]{filter}[sub];"));
    let graph = match (main_filter, subtitle.preprocess_filter.as_ref()) {
        (Some(main), Some(_)) => format!(
            "[{video_input}]{main}[main];{}[main][sub]overlay=eof_action=pass:repeatlast=0[v]",
            subtitle_chain.as_deref().unwrap_or_default()
        ),
        (Some(main), None) => format!(
            "[{video_input}]{main}[main];[main][{subtitle_input}]overlay=eof_action=pass:repeatlast=0[v]"
        ),
        (None, Some(_)) => format!(
            "{}[{video_input}][sub]overlay=eof_action=pass:repeatlast=0[v]",
            subtitle_chain.as_deref().unwrap_or_default()
        ),
        (None, None) => {
            format!("[{video_input}][{subtitle_input}]overlay=eof_action=pass:repeatlast=0[v]")
        }
    };
    let output_index = command.arguments.len().saturating_sub(1);
    command.arguments.splice(
        output_index..output_index,
        ["-filter_complex".to_owned(), graph],
    );
    if let Some(index) = command
        .arguments
        .windows(2)
        .position(|pair| pair[0] == "-map" && pair[1] == video_input)
    {
        command.arguments[index + 1] = "[v]".to_owned();
    }
    if copy_timestamps {
        command
            .arguments
            .retain(|argument| argument != "-start_at_zero");
    }
}

fn apply_subtitle_time_offset(command: &mut FfmpegCommand, start_time_ticks: Option<i64>) {
    let Some(ticks) = start_time_ticks.filter(|ticks| *ticks > 0) else {
        return;
    };
    let seconds = round_ticks_to_seconds(ticks);
    let Some(filter_index) = command
        .arguments
        .iter()
        .position(|argument| argument == "-vf")
        .and_then(|index| index.checked_add(1))
        .filter(|index| *index < command.arguments.len())
    else {
        return;
    };
    let _ = write!(command.arguments[filter_index], ",setpts=PTS -{seconds}/TB");
}

fn apply_embedded_subtitle(
    command: &mut FfmpegCommand,
    subtitle: &EmbeddedSubtitleRequest,
    start_time_ticks: Option<i64>,
) {
    if let Some(path) = subtitle.external_path.as_deref() {
        let input_index = command
            .arguments
            .iter()
            .position(|argument| argument == "-map")
            .unwrap_or_else(|| command.arguments.len().saturating_sub(1));
        let mut input = Vec::new();
        if let Some(ticks) = start_time_ticks.filter(|ticks| *ticks > 0) {
            input.extend(["-ss".to_owned(), format_ticks_as_seconds(ticks)]);
        }
        input.extend(["-i".to_owned(), path.to_owned()]);
        command.arguments.splice(input_index..input_index, input);
    }
    let output_index = command.arguments.len().saturating_sub(1);
    command.arguments.splice(
        output_index..output_index,
        [
            "-map".to_owned(),
            subtitle.map.clone(),
            "-c:s:0".to_owned(),
            subtitle.codec.clone(),
            "-disposition:s:0".to_owned(),
            "default".to_owned(),
        ],
    );
}

fn escape_subtitle_filter_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', r#"'\\\''"#)
        .replace('"', "\\\"")
}

fn preferred_vobsub_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sub"))
    {
        let index_path = path.with_extension("idx");
        if index_path.exists() {
            return index_path.to_string_lossy().into_owned();
        }
    }
    path.to_string_lossy().into_owned()
}

fn fixed_output_size(
    video_width: Option<i32>,
    video_height: Option<i32>,
    maximum_width: Option<i32>,
    maximum_height: Option<i32>,
) -> Option<(i32, i32)> {
    let (mut width, mut height) = (video_width?, video_height?);
    if width <= 0 || height <= 0 {
        return None;
    }
    let maximum_width = maximum_width.unwrap_or(width).min(4096);
    let maximum_height = maximum_height.unwrap_or(height).min(4096);
    if width > maximum_width || height > maximum_height {
        let scale = (f64::from(maximum_width) / f64::from(width))
            .min(f64::from(maximum_height) / f64::from(height));
        width = (f64::from(width) * scale).round_ties_even() as i32;
        height = (f64::from(height) * scale).round_ties_even() as i32;
    }
    width = 2 * (width / 2);
    height = 2 * (height / 2);
    (width > 0 && height > 0).then_some((width, height))
}

fn format_ticks_as_seconds(ticks: i64) -> String {
    let milliseconds = ticks / 10_000;
    let rounded_milliseconds = milliseconds + i64::from(ticks % 10_000 >= 5_000);
    format!(
        "{}.{:03}",
        rounded_milliseconds / 1_000,
        rounded_milliseconds % 1_000
    )
}

fn round_ticks_to_seconds(ticks: i64) -> i64 {
    let seconds = ticks / 10_000_000;
    let remainder = ticks % 10_000_000;
    seconds
        + i64::from(remainder > 5_000_000 || (remainder == 5_000_000 && seconds.rem_euclid(2) == 1))
}

fn is_local_path(path: &str) -> bool {
    !path.contains("://")
}

fn progressive_codecs_allowed(
    video_codec: &str,
    audio_codec: &str,
    allow_video_transcoding: bool,
    allow_audio_transcoding: bool,
) -> bool {
    (video_codec.eq_ignore_ascii_case("copy") || allow_video_transcoding)
        && (audio_codec.eq_ignore_ascii_case("copy") || allow_audio_transcoding)
}

fn copy_remux_has_no_transform_with_deinterlace(query: &StreamQuery, de_interlace: bool) -> bool {
    query.enable_auto_stream_copy.unwrap_or(true)
        && query.allow_video_stream_copy.unwrap_or(true)
        && query.allow_audio_stream_copy.unwrap_or(true)
        && query.width.is_none()
        && query.height.is_none()
        && !de_interlace
        && !(query.subtitle_stream_index.is_some()
            && should_burn_subtitles(query.subtitle_method, query.legacy_subtitle_method_unknown))
}

#[cfg(test)]
fn copy_remux_has_no_transform(query: &StreamQuery) -> bool {
    copy_remux_has_no_transform_with_deinterlace(query, query.de_interlace == Some(true))
}

#[cfg(test)]
fn can_copy_remux(
    query: &StreamQuery,
    container: &str,
    streams: &[MediaStream],
    requested_video_codec: &str,
    requested_audio_codec: &str,
) -> bool {
    can_copy_remux_with_options(
        query,
        &StreamOptions::default(),
        container,
        streams,
        requested_video_codec,
        requested_audio_codec,
    )
}

fn can_copy_remux_with_options(
    query: &StreamQuery,
    stream_options: &StreamOptions,
    container: &str,
    streams: &[MediaStream],
    requested_video_codec: &str,
    requested_audio_codec: &str,
) -> bool {
    let Some(video_stream) =
        selected_stream(streams, MediaStreamType::Video, query.video_stream_index)
    else {
        return false;
    };
    let Some(video_codec) = video_stream
        .codec
        .as_deref()
        .filter(|codec| !codec.trim().is_empty())
    else {
        return false;
    };
    let Some(audio_stream) =
        selected_stream(streams, MediaStreamType::Audio, query.audio_stream_index)
    else {
        return false;
    };
    let Some(audio_codec) = audio_stream
        .codec
        .as_deref()
        .filter(|codec| !codec.trim().is_empty())
    else {
        return false;
    };
    !(query.require_avc == Some(true)
        && video_codec.eq_ignore_ascii_case("h264")
        && video_stream.is_avc == Some(false))
        // EncodingHelper.CanStreamCopyVideo only rejects this requirement
        // when the persisted probe positively identifies an anamorphic
        // stream. Unknown probe state preserves the official copy fallback.
        && !(query.require_non_anamorphic == Some(true)
            && video_stream.is_anamorphic == Some(true))
        && video_profile_allows_copy(
            video_codec,
            video_stream.profile.as_deref(),
            query
                .profile
                .as_deref()
                .filter(|value| !value.is_empty())
                .or_else(|| stream_options.get_request_option(video_codec, "profile")),
        )
        && video_level_allows_copy(
            video_stream.level,
            query
                .level
                .as_deref()
                .filter(|value| !value.is_empty())
                .or_else(|| stream_options.get_request_option(video_codec, "level")),
        )
        && video_range_allows_copy(
            video_stream.video_range_type,
            stream_options.get_request_option(video_codec, "rangetype"),
        )
        && video_rotation_allows_copy(
            video_stream.rotation,
            stream_options.get_request_option(video_codec, "rotation"),
        )
        && video_dimensions_allow_copy(video_stream, query.max_width, query.max_height)
        && video_framerate_allows_copy(
            video_stream,
            query.max_framerate.or(query.framerate),
        )
        && video_bitrate_allows_copy(video_stream, query.video_bitrate)
        && !query
            .max_ref_frames
            .or_else(|| stream_option_i32(stream_options, video_codec, "maxrefframes"))
            .is_some_and(|maximum| {
            video_stream
                .ref_frames
                .is_some_and(|actual| actual > maximum)
        })
        && !query
            .max_video_bit_depth
            .or_else(|| stream_option_i32(stream_options, video_codec, "videobitdepth"))
            .is_some_and(|maximum| {
            video_stream
                .bit_depth
                .is_some_and(|actual| actual > maximum)
        })
        // EncodingHelper.CanStreamCopyAudio applies this constraint only
        // when stream probing supplied an audio bit depth.
        && !query
            .max_audio_bit_depth
            .or_else(|| stream_option_i32(stream_options, audio_codec, "audiobitdepth"))
            .is_some_and(|maximum| {
            audio_stream
                .bit_depth
                .is_some_and(|actual| actual > maximum)
        })
        && audio_channels_allow_copy(
            audio_stream,
            stream_option_i32(stream_options, audio_codec, "audiochannels")
                .or(query.max_audio_channels)
                .or(query.audio_channels)
                .or(query.transcoding_max_audio_channels),
        )
        && audio_sample_rate_allows_copy(audio_stream, query.audio_sample_rate)
        && audio_bitrate_allows_copy(audio_stream, query.audio_bitrate)
        && codecs_match(video_codec, requested_video_codec)
        && codecs_match(audio_codec, requested_audio_codec)
        && copy_remux_container_supports(container, video_codec, audio_codec)
}

fn stream_option_i32(stream_options: &StreamOptions, qualifier: &str, name: &str) -> Option<i32> {
    stream_options
        .get_request_option(qualifier, name)
        .and_then(|value| value.parse().ok())
}

fn video_rotation_allows_copy(actual: Option<i32>, requested: Option<&str>) -> bool {
    let actual = actual.unwrap_or(0);
    actual == 0
        || requested.is_none_or(|requested| {
            requested
                .split(',')
                .filter(|value| !value.is_empty())
                .any(|value| value == actual.to_string())
        })
}

fn video_range_allows_copy(actual: VideoRangeType, requested: Option<&str>) -> bool {
    let requested = requested
        .into_iter()
        .flat_map(|value| value.split(','))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return true;
    }
    if actual == VideoRangeType::Unknown {
        return false;
    }
    let contains = |name: &str| {
        requested
            .iter()
            .any(|value| value.eq_ignore_ascii_case(name))
    };
    let actual_name = video_range_type_name(actual);
    if contains(actual_name) {
        return true;
    }
    match actual {
        VideoRangeType::Sdr => true,
        VideoRangeType::Hdr10Plus => contains("HDR10"),
        VideoRangeType::DoviWithHdr10 => contains("HDR10") || contains("DOVI"),
        VideoRangeType::DoviWithHlg => contains("HLG") || contains("DOVI"),
        VideoRangeType::DoviWithSdr => contains("SDR") || contains("DOVI"),
        VideoRangeType::DoviWithEl => contains("DOVI"),
        _ => false,
    }
}

const fn video_range_type_name(range: VideoRangeType) -> &'static str {
    match range {
        VideoRangeType::Unknown => "Unknown",
        VideoRangeType::Sdr => "SDR",
        VideoRangeType::Hdr10 => "HDR10",
        VideoRangeType::Hlg => "HLG",
        VideoRangeType::Dovi => "DOVI",
        VideoRangeType::DoviWithHdr10 => "DOVIWithHDR10",
        VideoRangeType::DoviWithHlg => "DOVIWithHLG",
        VideoRangeType::DoviWithSdr => "DOVIWithSDR",
        VideoRangeType::DoviWithEl => "DOVIWithEL",
        VideoRangeType::DoviWithHdr10Plus => "DOVIWithHDR10Plus",
        VideoRangeType::DoviWithElHdr10Plus => "DOVIWithELHDR10Plus",
        VideoRangeType::DoviInvalid => "DOVIInvalid",
        VideoRangeType::Hdr10Plus => "HDR10Plus",
    }
}

fn video_dimensions_allow_copy(
    stream: &MediaStream,
    max_width: Option<i32>,
    max_height: Option<i32>,
) -> bool {
    max_width.is_none_or(|maximum| stream.width.is_some_and(|actual| actual <= maximum))
        && max_height.is_none_or(|maximum| stream.height.is_some_and(|actual| actual <= maximum))
}

fn video_framerate_allows_copy(stream: &MediaStream, requested: Option<f32>) -> bool {
    requested.is_none_or(|maximum| {
        // EncodingHelper permits a 0.05 fps tolerance for probe rounding.
        stream
            .reference_frame_rate()
            .is_some_and(|actual| actual <= maximum + 0.05)
    })
}

fn video_bitrate_allows_copy(stream: &MediaStream, requested: Option<i32>) -> bool {
    requested.is_none_or(|maximum| stream.bit_rate.is_some_and(|actual| actual <= maximum))
}

fn audio_channels_allow_copy(stream: &MediaStream, requested: Option<i32>) -> bool {
    requested.is_none_or(|maximum| {
        stream
            .channels
            .is_some_and(|actual| actual > 0 && actual <= maximum)
    })
}

fn audio_sample_rate_allows_copy(stream: &MediaStream, requested: Option<i32>) -> bool {
    requested.is_none_or(|maximum| {
        stream
            .sample_rate
            .is_some_and(|actual| actual > 0 && actual <= maximum)
    })
}

fn audio_bitrate_allows_copy(stream: &MediaStream, requested: Option<i32>) -> bool {
    // EncodingHelper allows an unknown audio bitrate but rejects a known
    // bitrate above the requested ceiling.
    requested.is_none_or(|maximum| stream.bit_rate.is_none_or(|actual| actual <= maximum))
}

fn selected_stream(
    streams: &[MediaStream],
    stream_type: MediaStreamType,
    requested_index: Option<i32>,
) -> Option<&MediaStream> {
    streams.iter().find(|stream| {
        stream.stream_type == stream_type
            && requested_index.is_none_or(|index| stream.index == index)
    })
}

fn requested_video_profiles(profile: Option<&str>) -> impl Iterator<Item = &str> {
    profile.into_iter().flat_map(|value| {
        value
            // EncodingJobInfo uses only Jellyfin's pipe/comma profile
            // separators; a semicolon remains part of the profile name.
            .split(|character| matches!(character, ',' | '|'))
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
    })
}

fn compact_profile(profile: &str) -> String {
    profile
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalized_video_codec(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("h265") {
        "hevc"
    } else {
        codec
    }
}

fn video_profile_score(codec: &str, profile: &str) -> Option<usize> {
    let profiles = match normalized_video_codec(codec).to_ascii_lowercase().as_str() {
        "h264" => [
            "constrainedbaseline",
            "baseline",
            "extended",
            "main",
            "high",
            "progressivehigh",
            "constrainedhigh",
            "high10",
        ]
        .as_slice(),
        "hevc" => ["main", "main10"].as_slice(),
        "av1" => ["main", "high", "professional"].as_slice(),
        _ => return None,
    };
    let profile = compact_profile(profile);
    profiles.iter().position(|known| *known == profile)
}

fn video_profile_allows_copy(
    codec: &str,
    source_profile: Option<&str>,
    requested_profile: Option<&str>,
) -> bool {
    let requested_profiles = requested_video_profiles(requested_profile).collect::<Vec<_>>();
    if requested_profiles.is_empty() {
        return true;
    }
    let Some(source_profile) = source_profile.filter(|profile| !profile.is_empty()) else {
        // EncodingHelper.CanStreamCopyVideo intentionally preserves this
        // fallback when ffprobe did not report a source profile.
        return true;
    };
    let source_profile = compact_profile(source_profile);
    if requested_profiles
        .iter()
        .any(|profile| compact_profile(profile) == source_profile)
    {
        return true;
    }
    matches!(
        (
            video_profile_score(codec, &source_profile),
            video_profile_score(codec, requested_profiles[0]),
        ),
        (Some(source), Some(requested)) if source <= requested
    )
}

fn requested_video_level(level: Option<&str>) -> Option<f64> {
    level
        .map(str::trim)
        .filter(|level| !level.is_empty())
        .and_then(|level| level.parse::<f64>().ok())
        .filter(|level| level.is_finite())
}

fn video_level_allows_copy(source_level: Option<f64>, requested_level: Option<&str>) -> bool {
    requested_video_level(requested_level)
        .zip(source_level)
        .is_none_or(|(requested, source)| source <= requested)
}

pub(crate) fn output_video_profile(codec: &str, profile: Option<&str>) -> Option<String> {
    let profile = requested_video_profiles(profile).next()?;
    video_profile_score(codec, profile).map(|_| compact_profile(profile))
}

pub(crate) fn output_video_level(codec: &str, level: Option<&str>) -> Option<String> {
    let requested = requested_video_level(level)?;
    let maximum = match normalized_video_codec(codec).to_ascii_lowercase().as_str() {
        "h264" => 51.0,
        "hevc" => 150.0,
        "av1" => 15.0,
        _ => return level.map(str::trim).map(str::to_owned),
    };
    if requested < 0.0 || requested >= maximum {
        Some(maximum.to_string())
    } else {
        level.map(str::trim).map(str::to_owned)
    }
}

fn codecs_match(actual: &str, requested: &str) -> bool {
    actual.trim().eq_ignore_ascii_case(requested.trim())
}

fn copy_remux_container_supports(container: &str, video_codec: &str, audio_codec: &str) -> bool {
    let container = container.trim().to_ascii_lowercase();
    let video_codec = video_codec.trim().to_ascii_lowercase();
    let audio_codec = audio_codec.trim().to_ascii_lowercase();
    match container.as_str() {
        // Matroska accepts the broadest practical set of already-probed
        // FFmpeg codecs, so it is the safe fallback for clients that support
        // it. Keep MP4/WebM conservative: a failed mux after advertising a
        // direct stream is worse than falling back to an encode.
        "mkv" | "matroska" => true,
        "mp4" | "m4v" | "mov" => {
            matches!(
                video_codec.as_str(),
                "h264" | "hevc" | "h265" | "av1" | "mpeg4"
            ) && matches!(
                audio_codec.as_str(),
                "aac" | "ac3" | "eac3" | "mp3" | "alac"
            )
        }
        "ts" | "mpegts" => {
            matches!(
                video_codec.as_str(),
                "h264" | "hevc" | "h265" | "mpeg2video"
            ) && matches!(audio_codec.as_str(), "aac" | "ac3" | "eac3" | "mp3")
        }
        "webm" => {
            matches!(video_codec.as_str(), "vp8" | "vp9" | "av1")
                && matches!(audio_codec.as_str(), "opus" | "vorbis")
        }
        _ => false,
    }
}

fn should_burn_subtitles(
    method: Option<SubtitleDeliveryMethod>,
    legacy_unknown_method: bool,
) -> bool {
    !legacy_unknown_method && method.unwrap_or_default() == SubtitleDeliveryMethod::Encode
}

fn video_codec_for_container(container: &str) -> &str {
    match container {
        "webm" => "vp9",
        _ => "h264",
    }
}

fn audio_codec_for_container(container: &str) -> &str {
    match container {
        "webm" => "opus",
        _ => "aac",
    }
}

fn output_container(requested_container: Option<&str>, query_container: Option<&str>) -> String {
    requested_container
        .or(query_container)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_owned())
}

pub(crate) fn required_remote_user_agent(
    item: &jellyfin_data::entities::base_item::Model,
) -> Option<&str> {
    let headers = item
        .data
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|data| {
            [
                "RequiredHttpHeaders",
                "requiredHttpHeaders",
                "required_http_headers",
            ]
            .iter()
            .find_map(|key| data.get(*key))
        })
        .and_then(serde_json::Value::as_object)?;
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("User-Agent"))
        .and_then(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn proxy_remote_stream(
    client: &reqwest::Client,
    client_headers: &HeaderMap,
    item_id: Uuid,
    path: &str,
    user_agent: Option<&str>,
) -> Result<Response, ApiError> {
    let mut request = client.get(path);
    if let Some(user_agent) = user_agent {
        request = request.header(header::USER_AGENT, user_agent);
    }
    if let Some(range) = client_headers.get(header::RANGE) {
        request = request.header(header::RANGE, range);
    }
    let upstream = request.send().await.map_err(|error| {
        tracing::warn!(
            %item_id,
            timeout = error.is_timeout(),
            connect = error.is_connect(),
            "remote video stream request failed",
        );
        ApiError::UpstreamUnavailable
    })?;
    let status = upstream.status();
    let mut response = Response::builder().status(status);
    let headers = response.headers_mut().ok_or(ApiError::Internal)?;
    for name in [
        header::CONTENT_RANGE,
        header::CONTENT_LENGTH,
        header::CONTENT_TYPE,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/octet-stream"),
        );
    }
    let accept_ranges = upstream
        .headers()
        .get(header::ACCEPT_RANGES)
        .cloned()
        .unwrap_or_else(|| {
            header::HeaderValue::from_static(if status == StatusCode::PARTIAL_CONTENT {
                "bytes"
            } else {
                "none"
            })
        });
    headers.insert(header::ACCEPT_RANGES, accept_ranges);
    tracing::info!(
        %item_id,
        %status,
        range_requested = client_headers.contains_key(header::RANGE),
        "proxying remote video stream",
    );
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|_| ApiError::Internal)
}

pub(crate) async fn delete_alternate_sources(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    state
        .videos
        .clear_alternate_sources(identity.is_administrator_equivalent(), item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn merge_versions(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<MergeVersionsQuery>,
) -> Result<StatusCode, ApiError> {
    let identity = authentication::authenticated_identity(&state, &headers, Some(&uri)).await?;
    state
        .videos
        .merge_versions(identity.is_administrator_equivalent(), &query.ids)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn additional_parts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
    Query(query): Query<user_library::UserIdQuery>,
) -> Result<Json<user_library::BaseItemQueryResult>, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    let target_user_id = query
        .user_id
        .filter(|user_id| !user_id.is_nil())
        .unwrap_or(authenticated.user.id);
    let items = state
        .user_library
        .additional_parts(&authenticated.user, target_user_id, item_id)
        .await?;
    let total_record_count = u64::try_from(items.len()).unwrap_or(u64::MAX);
    Ok(Json(
        crate::items::page_to_dto_all_fields(
            state.as_ref(),
            BaseItemPage {
                items,
                total_record_count,
                start_index: 0,
            },
            target_user_id,
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        time::Duration,
    };

    use axum::{
        body::to_bytes,
        http::{Uri, header},
        response::IntoResponse,
    };
    use axum_extra::extract::Query;

    use super::*;

    #[test]
    fn video_stream_binds_android_progressive_parameters() {
        // These names deliberately mix the canonical SDK casing, PascalCase,
        // and lower-case legacy spelling accepted by ASP.NET model binding.
        let uri: Uri = "/videos/item/stream.mp4?container=webm&static=false&params=client%3Dandroid&Tag=etag&deviceprofileid=profile&PlaySessionId=play-session&segmentcontainer=ts&SegmentLength=6&minsegments=2&MediaSourceId=alternate&deviceid=device&enableautostreamcopy=true&AllowVideoStreamCopy=false&allowaudiostreamcopy=true&videoCodec=h264&audioCodec=aac&videoBitrate=2000000&audioBitrate=128000&audioSampleRate=48000&MaxAudioBitDepth=24&audioChannels=2&maxAudioChannels=6&Profile=high&Level=4.1&framerate=24&width=1280&Height=720&MaxFramerate=23.976&audioStreamIndex=2&videoStreamIndex=0&subtitleStreamIndex=3&subtitleMethod=Encode&MaxRefFrames=4&maxvideobitdepth=10&RequireAvc=true&deinterlace=true&requireNonAnamorphic=true&startTimeTicks=10000&CopyTimestamps=true&cpuCoreLimit=2&liveStreamId=live&enableMpegtsM2TsMode=true&subtitleCodec=srt&transcodeReasons=ContainerNotSupported&context=Streaming&streamoptions=quality%3Dhigh&enableAudioVbrEncoding=false"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.container.as_deref(), Some("webm"));
        assert!(!query.static_stream.unwrap());
        assert_eq!(query.params.as_deref(), Some("client=android"));
        assert_eq!(query.tag.as_deref(), Some("etag"));
        assert_eq!(query._device_profile_id.as_deref(), Some("profile"));
        assert_eq!(query.play_session_id.as_deref(), Some("play-session"));
        assert_eq!(query.segment_container.as_deref(), Some("ts"));
        assert_eq!(query._segment_length, Some(6));
        assert_eq!(query._min_segments, Some(2));
        assert_eq!(query.media_source_id.as_deref(), Some("alternate"));
        assert_eq!(query.device_id.as_deref(), Some("device"));
        assert_eq!(query.enable_auto_stream_copy, Some(true));
        assert_eq!(query.allow_video_stream_copy, Some(false));
        assert_eq!(query.allow_audio_stream_copy, Some(true));
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
        assert_eq!(query.audio_codec.as_deref(), Some("aac"));
        assert_eq!(query.video_bitrate, Some(2_000_000));
        assert_eq!(query.audio_bitrate, Some(128_000));
        assert_eq!(query.audio_sample_rate, Some(48_000));
        assert_eq!(query.max_audio_bit_depth, Some(24));
        assert_eq!(query.audio_channels, Some(2));
        assert_eq!(query.max_audio_channels, Some(6));
        assert_eq!(query.profile.as_deref(), Some("high"));
        assert_eq!(query.level.as_deref(), Some("4.1"));
        assert_eq!(query.framerate, Some(24.0));
        assert_eq!(query.width, Some(1280));
        assert_eq!(query.height, Some(720));
        assert_eq!(query.max_framerate, Some(23.976));
        assert_eq!(query.audio_stream_index, Some(2));
        assert_eq!(query.video_stream_index, Some(0));
        assert_eq!(query.subtitle_stream_index, Some(3));
        assert_eq!(query.subtitle_method, Some(SubtitleDeliveryMethod::Encode));
        assert_eq!(query.max_ref_frames, Some(4));
        assert_eq!(query.max_video_bit_depth, Some(10));
        assert_eq!(query.require_avc, Some(true));
        assert_eq!(query.de_interlace, Some(true));
        assert_eq!(query.require_non_anamorphic, Some(true));
        assert_eq!(query.start_time_ticks, Some(10_000));
        assert_eq!(query.copy_timestamps, Some(true));
        assert_eq!(query.cpu_core_limit, Some(2));
        assert_eq!(query.live_stream_id.as_deref(), Some("live"));
        assert_eq!(query.enable_mpegts_m2_ts_mode, Some(true));
        assert_eq!(query.subtitle_codec.as_deref(), Some("srt"));
        assert_eq!(
            query.transcode_reasons.as_deref(),
            Some("ContainerNotSupported")
        );
        assert_eq!(query.context, Some(EncodingContext::Streaming));
        assert_eq!(query._stream_options, ["quality=high"]);
        assert_eq!(query.enable_audio_vbr_encoding, Some(false));
        assert_eq!(video_codec_for_container("mp4"), "h264");
        assert_eq!(audio_codec_for_container("webm"), "opus");
        assert!(should_burn_subtitles(
            Some(SubtitleDeliveryMethod::Encode),
            false
        ));
        assert!(should_burn_subtitles(None, false));
        assert!(!should_burn_subtitles(
            Some(SubtitleDeliveryMethod::External),
            false
        ));
    }

    #[test]
    fn progressive_video_binds_enum_names_and_defined_integers() {
        for (query_string, method, context) in [
            (
                "subtitlemethod=eMbEd&context=sTaTiC",
                SubtitleDeliveryMethod::Embed,
                EncodingContext::Static,
            ),
            (
                "SubtitleMethod=4&Context=0",
                SubtitleDeliveryMethod::Drop,
                EncodingContext::Streaming,
            ),
            (
                "SubtitleMethod=%2B01&Context=%2B00",
                SubtitleDeliveryMethod::Embed,
                EncodingContext::Streaming,
            ),
        ] {
            let uri: Uri = format!("/videos/item/stream?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(query.subtitle_method, Some(method));
            assert_eq!(query.context, Some(context));
        }

        for query_string in [
            "subtitleMethod=Unknown",
            "subtitleMethod=5",
            "context=Download",
            "context=2",
            "cpuCoreLimit=2147483648",
            "videoBitRate=2147483648",
            "audioBitRate=2147483648",
        ] {
            let uri: Uri = format!("/videos/item/stream?{query_string}")
                .parse()
                .unwrap();
            assert!(
                Query::<StreamQuery>::try_from_uri(&uri).is_err(),
                "{query_string}"
            );
        }
    }

    #[test]
    fn progressive_video_binds_subtitle_seek_casing_and_rejects_malformed_values() {
        for query_string in [
            "SubtitleStreamIndex=4&SubtitleMethod=Encode&StartTimeTicks=105000000",
            "subtitleStreamIndex=4&subtitleMethod=Encode&startTimeTicks=105000000",
            "subtitlestreamindex=4&subtitlemethod=Encode&starttimeticks=105000000",
        ] {
            let uri: Uri = format!("/videos/item/stream.mp4?{query_string}")
                .parse()
                .unwrap();
            let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
            assert_eq!(query.subtitle_stream_index, Some(4), "{query_string}");
            assert_eq!(
                query.subtitle_method,
                Some(SubtitleDeliveryMethod::Encode),
                "{query_string}"
            );
            assert_eq!(query.start_time_ticks, Some(105_000_000), "{query_string}");
        }

        for query_string in [
            "SubtitleStreamIndex=pgs",
            "subtitleStreamIndex=pgs",
            "subtitlestreamindex=pgs",
            "StartTimeTicks=ten",
            "startTimeTicks=ten",
            "starttimeticks=ten",
            "subtitlemethod=invalid",
        ] {
            let uri: Uri = format!("/videos/item/stream.mp4?{query_string}")
                .parse()
                .unwrap();
            let result =
                Query::<StreamQuery>::try_from_uri(&uri).map_err(|_| ApiError::InvalidRequest);
            assert_eq!(
                result.unwrap_err().into_response().status(),
                StatusCode::BAD_REQUEST,
                "{query_string}"
            );
        }
    }

    #[test]
    fn progressive_video_applies_supported_legacy_params_over_query_values() {
        let mut values = vec![""; 34];
        values[1] = "legacy-device";
        values[2] = "legacy-source";
        values[3] = "true";
        values[4] = "h264";
        values[5] = "aac";
        values[6] = " 2 ";
        values[7] = " 3 ";
        values[8] = " 2000000 ";
        values[9] = " 128000 ";
        values[10] = " 6 ";
        values[11] = " 23.976 ";
        values[12] = " 1280 ";
        values[13] = " 720 ";
        values[14] = " 10000 ";
        values[15] = "level=4.1x";
        values[16] = " 4 ";
        values[17] = " 10 ";
        values[18] = "high";
        values[20] = "legacy-session";
        values[22] = "legacy-live";
        values[24] = "true";
        values[25] = "Embed";
        values[26] = " 6 ";
        values[28] = "legacy-tag";
        values[29] = "true";
        values[30] = "srt";
        values[31] = "true";
        values[32] = "true";
        values[33] = "ContainerNotSupported";
        let mut query = StreamQuery {
            device_id: Some("query-device".to_owned()),
            params: Some(values.join(";")),
            ..StreamQuery::default()
        };
        apply_legacy_params(&mut query).unwrap();
        assert_eq!(query.device_id.as_deref(), Some("legacy-device"));
        assert_eq!(query.media_source_id.as_deref(), Some("legacy-source"));
        assert_eq!(query.static_stream, Some(true));
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
        assert_eq!(query.audio_codec.as_deref(), Some("aac"));
        assert_eq!(query.audio_stream_index, Some(2));
        assert_eq!(query.subtitle_stream_index, Some(3));
        assert_eq!(query.video_bitrate, Some(2_000_000));
        assert_eq!(query.audio_bitrate, Some(128_000));
        assert_eq!(query.max_audio_channels, Some(6));
        assert_eq!(query.max_framerate, Some(23.976));
        assert_eq!(query.max_width, Some(1280));
        assert_eq!(query.max_height, Some(720));
        assert_eq!(query.start_time_ticks, Some(10_000));
        assert_eq!(query.level.as_deref(), Some("level=4.1x"));
        assert_eq!(query.max_ref_frames, Some(4));
        assert_eq!(query.max_video_bit_depth, Some(10));
        assert_eq!(query.profile.as_deref(), Some("high"));
        assert_eq!(query.play_session_id.as_deref(), Some("legacy-session"));
        assert_eq!(query.live_stream_id.as_deref(), Some("legacy-live"));
        assert_eq!(query.copy_timestamps, Some(true));
        assert_eq!(query.subtitle_method, Some(SubtitleDeliveryMethod::Embed));
        assert_eq!(query.transcoding_max_audio_channels, Some(6));
        assert_eq!(query.tag.as_deref(), Some("legacy-tag"));
        assert_eq!(query.require_avc, Some(true));
        assert_eq!(query.subtitle_codec.as_deref(), Some("srt"));
        assert_eq!(query.require_non_anamorphic, Some(true));
        assert_eq!(query.de_interlace, Some(true));
        assert_eq!(
            query.transcode_reasons.as_deref(),
            Some("ContainerNotSupported")
        );
    }

    #[test]
    fn progressive_video_preserves_unknown_packed_enum_effect() {
        let mut values = vec![""; 26];
        values[25] = " 5 ";
        let mut query = StreamQuery {
            subtitle_method: Some(SubtitleDeliveryMethod::Encode),
            subtitle_stream_index: Some(3),
            params: Some(values.join(";")),
            ..StreamQuery::default()
        };
        apply_legacy_params(&mut query).unwrap();
        assert_eq!(query.subtitle_method, None);
        assert!(query.legacy_subtitle_method_unknown);
        assert!(!should_burn_subtitles(
            query.subtitle_method,
            query.legacy_subtitle_method_unknown
        ));
    }

    #[test]
    fn progressive_video_validates_explicit_values_before_packed_overrides() {
        let mut values = vec![""; 6];
        values[4] = "h264";
        let mut query = StreamQuery {
            video_codec: Some("bad!".to_owned()),
            params: Some(values.join(";")),
            ..StreamQuery::default()
        };
        assert!(validate_progressive_query(None, &query).is_err());
        apply_legacy_params(&mut query).unwrap();
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
    }

    #[test]
    fn progressive_video_validates_sdk_codec_container_and_level_parameters() {
        for query in [
            StreamQuery {
                video_codec: Some("h264;touch".to_owned()),
                ..StreamQuery::default()
            },
            StreamQuery {
                subtitle_codec: Some("srt/ass".to_owned()),
                ..StreamQuery::default()
            },
            StreamQuery {
                level: Some("4.1.2".to_owned()),
                ..StreamQuery::default()
            },
        ] {
            assert!(validate_progressive_query(None, &query).is_err());
        }
        assert!(validate_progressive_query(None, &StreamQuery::default()).is_ok());
        assert!(validate_progressive_query(Some("../../mp4"), &StreamQuery::default()).is_err());
    }

    #[test]
    fn progressive_video_applies_the_official_cpu_core_limit() {
        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".to_owned(),
                "input.mkv".to_owned(),
                "output.mp4".to_owned(),
            ],
        };
        apply_cpu_core_limit(&mut command, Some(1));
        assert_eq!(
            command.arguments,
            ["-i", "input.mkv", "-threads", "1", "output.mp4"]
        );

        let mut automatic = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec!["output.ts".to_owned()],
        };
        apply_cpu_core_limit(&mut automatic, Some(-1));
        assert_eq!(automatic.arguments, ["-threads", "0", "output.ts"]);

        let mut clamped = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec!["output.mkv".to_owned()],
        };
        apply_cpu_core_limit(&mut clamped, Some(i32::MAX));
        let expected = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .to_string();
        assert_eq!(clamped.arguments, ["-threads", &expected, "output.mkv"]);

        let mut omitted = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec!["output.webm".to_owned()],
        };
        apply_cpu_core_limit(&mut omitted, None);
        assert_eq!(omitted.arguments, ["output.webm"]);
    }

    #[test]
    fn progressive_video_checks_audio_and_video_transcoding_policies_separately() {
        assert!(progressive_codecs_allowed("copy", "copy", false, false));
        assert!(progressive_codecs_allowed("h264", "copy", true, false));
        assert!(progressive_codecs_allowed("copy", "aac", false, true));
        assert!(!progressive_codecs_allowed("h264", "aac", true, false));
        assert!(!progressive_codecs_allowed("h264", "aac", false, true));
    }

    #[test]
    fn progressive_video_applies_m2ts_mode_only_when_requested() {
        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".to_owned(),
                "input.mkv".to_owned(),
                "output.ts".to_owned(),
            ],
        };
        apply_mpegts_m2_ts_mode(&mut command, true);
        assert_eq!(
            command.arguments,
            ["-i", "input.mkv", "-mpegts_m2ts_mode", "1", "output.ts"]
        );

        let mut disabled = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec!["output.ts".to_owned()],
        };
        apply_mpegts_m2_ts_mode(&mut disabled, false);
        assert_eq!(disabled.arguments, ["output.ts"]);
    }

    #[test]
    fn progressive_video_embeds_the_selected_internal_subtitle() {
        let streams = vec![MediaStream {
            codec: Some("srt".to_owned()),
            index: 3,
            stream_type: MediaStreamType::Subtitle,
            ..MediaStream::default()
        }];
        let query = StreamQuery {
            subtitle_stream_index: Some(3),
            subtitle_method: Some(SubtitleDeliveryMethod::Embed),
            subtitle_codec: Some("ass,srt".to_owned()),
            ..StreamQuery::default()
        };
        let subtitle = embedded_subtitle_request(
            &query,
            &streams,
            streams.first(),
            Some(SubtitleDeliveryMethod::Embed),
        )
        .unwrap();
        assert_eq!(subtitle.map, "0:3");
        assert_eq!(subtitle.codec, "ass");

        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec!["-i".into(), "input.mkv".into(), "output.mkv".into()],
        };
        apply_embedded_subtitle(&mut command, &subtitle, None);
        assert_eq!(
            command.arguments,
            [
                "-i",
                "input.mkv",
                "-map",
                "0:3",
                "-c:s:0",
                "ass",
                "-disposition:s:0",
                "default",
                "output.mkv"
            ]
        );
    }

    #[test]
    fn progressive_video_normalizes_dvbsub_embed_to_encode() {
        let stream = MediaStream {
            codec: Some("DVBSUB".to_owned()),
            index: 3,
            stream_type: MediaStreamType::Subtitle,
            ..MediaStream::default()
        };
        let query = StreamQuery {
            subtitle_stream_index: Some(3),
            subtitle_method: Some(SubtitleDeliveryMethod::Embed),
            ..StreamQuery::default()
        };
        assert_eq!(
            effective_subtitle_method(&query, Some(&stream)),
            Some(SubtitleDeliveryMethod::Encode)
        );
    }

    #[test]
    fn progressive_video_embeds_and_seeks_an_external_subtitle_input() {
        let streams = vec![MediaStream {
            codec: Some("srt".to_owned()),
            index: 4,
            stream_type: MediaStreamType::Subtitle,
            is_external: true,
            path: Some("/media/Movie: English.srt".to_owned()),
            ..MediaStream::default()
        }];
        let query = StreamQuery {
            subtitle_stream_index: Some(4),
            subtitle_method: Some(SubtitleDeliveryMethod::Embed),
            subtitle_codec: Some("srt".to_owned()),
            ..StreamQuery::default()
        };
        let subtitle = embedded_subtitle_request(
            &query,
            &streams,
            streams.first(),
            Some(SubtitleDeliveryMethod::Embed),
        )
        .unwrap();
        assert_eq!(subtitle.map, "1:0");
        assert_eq!(subtitle.codec, "copy");

        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".into(),
                "input.mkv".into(),
                "-map".into(),
                "0:v:0".into(),
                "output.mkv".into(),
            ],
        };
        apply_embedded_subtitle(&mut command, &subtitle, Some(10_000_000));
        assert_eq!(
            command.arguments,
            [
                "-i",
                "input.mkv",
                "-ss",
                "1.000",
                "-i",
                "/media/Movie: English.srt",
                "-map",
                "0:v:0",
                "-map",
                "1:0",
                "-c:s:0",
                "copy",
                "-disposition:s:0",
                "default",
                "output.mkv"
            ]
        );
    }

    #[test]
    fn progressive_video_burns_external_subtitle_paths_without_an_si() {
        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".into(),
                "input.mkv".into(),
                "-vf".into(),
                "scale=1280:-2".into(),
                "output.mp4".into(),
            ],
        };
        apply_external_text_subtitle_burn(
            &mut command,
            std::path::Path::new("/media/Movie: English.srt"),
        );
        apply_subtitle_time_offset(&mut command, Some(105_000_000));
        assert_eq!(
            command.arguments[3],
            "scale=1280:-2,subtitles='/media/Movie\\: English.srt',setpts=PTS -10/TB"
        );
        assert_eq!(
            escape_subtitle_filter_path(std::path::Path::new("C:\\Movie's.srt")),
            r#"C\:/Movie'\\\''s.srt"#
        );
    }

    #[test]
    fn progressive_video_burns_external_pgs_from_a_sought_overlay_input() {
        let video = MediaStream {
            codec: Some("h264".to_owned()),
            index: 0,
            stream_type: MediaStreamType::Video,
            width: Some(1920),
            height: Some(1080),
            ..MediaStream::default()
        };
        let subtitle = MediaStream {
            codec: Some("PGSSUB".to_owned()),
            index: 4,
            stream_type: MediaStreamType::Subtitle,
            is_external: true,
            path: Some("/media/movie.sup".to_owned()),
            width: Some(1920),
            height: Some(1080),
            ..MediaStream::default()
        };
        let streams = vec![video, subtitle.clone()];
        let query = StreamQuery {
            video_stream_index: Some(0),
            subtitle_stream_index: Some(4),
            subtitle_method: Some(SubtitleDeliveryMethod::Encode),
            max_width: Some(1280),
            max_height: Some(720),
            ..StreamQuery::default()
        };
        let request =
            external_graphical_subtitle_request(&streams, Some(&subtitle), true, &query).unwrap();
        assert_eq!(request.path, "/media/movie.sup");
        assert_eq!(request.stream_index, 0);

        let mut command = video_command(
            std::path::Path::new("/usr/bin/ffmpeg"),
            std::path::Path::new("/media/movie.mkv"),
            std::path::Path::new("/tmp/out.mp4"),
            "h264",
            "aac",
            None,
            None,
            None,
            None,
            Some(1280),
            Some(720),
            None,
            false,
            false,
            None,
            None,
            None,
            Some(0),
            Some(105_000_000),
            None,
            false,
        );
        apply_external_graphical_subtitle_burn(&mut command, &request, Some(105_000_000), false);

        assert_eq!(
            command
                .arguments
                .windows(2)
                .filter(|pair| *pair == ["-ss", "10.500"])
                .count(),
            2
        );
        assert!(command.arguments.windows(4).any(|arguments| {
            arguments == ["-canvas_size", "1920x1080", "-i", "file:/media/movie.sup"]
        }));
        assert!(
            command
                .arguments
                .windows(2)
                .any(|pair| pair == ["-map", "[v]"])
        );
        let graph = command
            .arguments
            .windows(2)
            .find(|pair| pair[0] == "-filter_complex")
            .map(|pair| pair[1].as_str())
            .unwrap();
        assert!(
            graph.contains("[1:0]scale,scale=1280:720:fast_bilinear[sub]"),
            "{graph}"
        );
        assert!(
            graph.contains("[main][sub]overlay=eof_action=pass:repeatlast=0[v]"),
            "{graph}"
        );
        assert!(!graph.contains("setpts"), "{graph}");
        assert!(
            command
                .arguments
                .iter()
                .all(|argument| !argument.contains("subtitles="))
        );
    }

    #[test]
    fn progressive_video_uses_the_selected_external_graphical_stream_specifier() {
        let video = MediaStream {
            index: 0,
            stream_type: MediaStreamType::Video,
            ..MediaStream::default()
        };
        let first = MediaStream {
            codec: Some("pgssub".to_owned()),
            index: 3,
            stream_type: MediaStreamType::Subtitle,
            is_external: true,
            path: Some("/media/movie.mks".to_owned()),
            ..MediaStream::default()
        };
        let second = MediaStream {
            index: 4,
            ..first.clone()
        };
        let streams = vec![video, first, second.clone()];
        let query = StreamQuery {
            subtitle_stream_index: Some(4),
            subtitle_method: Some(SubtitleDeliveryMethod::Encode),
            ..StreamQuery::default()
        };
        let request =
            external_graphical_subtitle_request(&streams, Some(&second), true, &query).unwrap();
        assert_eq!(request.stream_index, 1);

        let mut command = FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".into(),
                "/media/movie.mkv".into(),
                "-map".into(),
                "0:v:0".into(),
                "/tmp/out.mp4".into(),
            ],
        };
        apply_external_graphical_subtitle_burn(&mut command, &request, None, false);
        let graph = command
            .arguments
            .windows(2)
            .find(|pair| pair[0] == "-filter_complex")
            .map(|pair| pair[1].as_str())
            .unwrap();
        assert_eq!(graph, "[0:v:0][1:1]overlay=eof_action=pass:repeatlast=0[v]");
    }

    #[test]
    fn progressive_video_uses_vobsub_idx_and_preserves_copy_timestamps() {
        let temporary = std::env::temp_dir().join(format!(
            "jellyfin-progressive-vobsub-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&temporary).unwrap();
        let sub_path = temporary.join("movie.SUB");
        let idx_path = temporary.join("movie.idx");
        fs::write(&sub_path, "subtitle data").unwrap();
        assert_eq!(
            preferred_vobsub_path(&sub_path.to_string_lossy()),
            sub_path.to_string_lossy()
        );
        fs::write(&idx_path, "subtitle index").unwrap();
        let subtitle = MediaStream {
            codec: Some("VoBsUb".to_owned()),
            index: 2,
            stream_type: MediaStreamType::Subtitle,
            is_external: true,
            path: Some(sub_path.to_string_lossy().into_owned()),
            ..MediaStream::default()
        };
        let query = StreamQuery {
            subtitle_stream_index: Some(2),
            subtitle_method: Some(SubtitleDeliveryMethod::Encode),
            ..StreamQuery::default()
        };
        let request = external_graphical_subtitle_request(
            std::slice::from_ref(&subtitle),
            Some(&subtitle),
            true,
            &query,
        )
        .unwrap();
        assert_eq!(request.path, idx_path.to_string_lossy());

        let mut command = video_command(
            std::path::Path::new("/usr/bin/ffmpeg"),
            std::path::Path::new("/media/movie.mkv"),
            std::path::Path::new("/tmp/out.mp4"),
            "h264",
            "aac",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            Some(10_000_000),
            None,
            true,
        );
        apply_external_graphical_subtitle_burn(&mut command, &request, Some(10_000_000), true);
        assert!(command.arguments.windows(2).any(|pair| {
            pair[0] == "-i" && pair[1] == format!("file:{}", idx_path.to_string_lossy())
        }));
        assert!(
            !command
                .arguments
                .iter()
                .any(|argument| argument == "-start_at_zero")
        );
        let graph = command
            .arguments
            .windows(2)
            .find(|pair| pair[0] == "-filter_complex")
            .map(|pair| pair[1].as_str())
            .unwrap();
        assert!(graph.contains("[1:0]overlay="), "{graph}");
        assert!(!graph.contains("setpts"), "{graph}");

        fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn progressive_video_defaults_to_streaming_context() {
        let fragmented = || FfmpegCommand {
            program: "/usr/bin/ffmpeg".into(),
            arguments: vec![
                "-i".to_owned(),
                "input.mkv".to_owned(),
                "-f".to_owned(),
                "mp4".to_owned(),
                "-movflags".to_owned(),
                "frag_keyframe+empty_moov+delay_moov".to_owned(),
                "output.mp4".to_owned(),
            ],
        };
        let mut default_context = fragmented();
        apply_encoding_context(&mut default_context, None);
        assert!(default_context.arguments.contains(&"-movflags".to_owned()));

        let mut static_context = fragmented();
        apply_encoding_context(&mut static_context, Some(EncodingContext::Static));
        assert_eq!(static_context.arguments, ["-i", "input.mkv", "output.mp4"]);

        let mut streaming = fragmented();
        apply_encoding_context(&mut streaming, Some(EncodingContext::Streaming));
        assert!(streaming.arguments.contains(&"-movflags".to_owned()));
    }

    #[test]
    fn video_stream_binds_lowercase_container_and_framerate() {
        let uri: Uri = "/videos/item/stream?container=.WEBM&framerate=25"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert_eq!(query.container.as_deref(), Some(".WEBM"));
        assert_eq!(query.framerate, Some(25.0));
        assert_eq!(
            output_container(None, query.container.as_deref()),
            "webm",
            "extensionless routes use the official query container"
        );
        assert_eq!(
            output_container(Some("mkv"), query.container.as_deref()),
            "mkv",
            "the by-container route remains authoritative"
        );
    }

    #[test]
    fn local_copy_remux_requires_matching_codecs_and_no_transform() {
        let streams = vec![
            MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("h264".to_owned()),
                ..MediaStream::default()
            },
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                codec: Some("aac".to_owned()),
                ..MediaStream::default()
            },
        ];
        let query = StreamQuery {
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            ..StreamQuery::default()
        };

        assert!(copy_remux_has_no_transform(&query));
        assert!(can_copy_remux(&query, "mp4", &streams, "h264", "aac"));

        let compatible_limited_streams = vec![
            MediaStream {
                width: Some(1280),
                height: Some(720),
                average_frame_rate: Some(24.0),
                bit_rate: Some(2_000_000),
                ..streams[0].clone()
            },
            MediaStream {
                channels: Some(2),
                sample_rate: Some(48_000),
                bit_rate: Some(128_000),
                ..streams[1].clone()
            },
        ];
        let compatible_limits = StreamQuery {
            max_width: Some(1280),
            max_height: Some(720),
            max_framerate: Some(23.976),
            video_bitrate: Some(2_000_000),
            audio_bitrate: Some(128_000),
            max_audio_channels: Some(2),
            audio_sample_rate: Some(48_000),
            ..query.clone()
        };
        assert!(copy_remux_has_no_transform(&compatible_limits));
        assert!(can_copy_remux(
            &compatible_limits,
            "mp4",
            &compatible_limited_streams,
            "h264",
            "aac",
        ));
        let incompatible_framerate_streams = vec![
            MediaStream {
                average_frame_rate: Some(24.1),
                ..compatible_limited_streams[0].clone()
            },
            compatible_limited_streams[1].clone(),
        ];
        assert!(!can_copy_remux(
            &compatible_limits,
            "mp4",
            &incompatible_framerate_streams,
            "h264",
            "aac",
        ));

        let non_avc_streams = vec![
            MediaStream {
                is_avc: Some(false),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        let require_avc = StreamQuery {
            require_avc: Some(true),
            ..query.clone()
        };
        assert!(!can_copy_remux(
            &require_avc,
            "mp4",
            &non_avc_streams,
            "h264",
            "aac",
        ));

        let anamorphic_streams = vec![
            MediaStream {
                is_anamorphic: Some(true),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        let require_non_anamorphic = StreamQuery {
            require_non_anamorphic: Some(true),
            ..query.clone()
        };
        assert!(!can_copy_remux(
            &require_non_anamorphic,
            "mp4",
            &anamorphic_streams,
            "h264",
            "aac",
        ));

        let too_many_ref_frames = StreamQuery {
            max_ref_frames: Some(3),
            ..query.clone()
        };
        let high_ref_streams = vec![
            MediaStream {
                ref_frames: Some(4),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        assert!(!can_copy_remux(
            &too_many_ref_frames,
            "mp4",
            &high_ref_streams,
            "h264",
            "aac",
        ));

        let low_bit_depth_limit = StreamQuery {
            max_video_bit_depth: Some(8),
            ..query.clone()
        };
        let high_bit_depth_streams = vec![
            MediaStream {
                bit_depth: Some(10),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        assert!(!can_copy_remux(
            &low_bit_depth_limit,
            "mp4",
            &high_bit_depth_streams,
            "h264",
            "aac",
        ));

        let low_audio_bit_depth_limit = StreamQuery {
            max_audio_bit_depth: Some(16),
            ..query.clone()
        };
        let high_bit_depth_audio_streams = vec![
            streams[0].clone(),
            MediaStream {
                bit_depth: Some(24),
                ..streams[1].clone()
            },
        ];
        assert!(!can_copy_remux(
            &low_audio_bit_depth_limit,
            "mp4",
            &high_bit_depth_audio_streams,
            "h264",
            "aac",
        ));

        let high_profile_streams = vec![
            MediaStream {
                profile: Some("High".to_owned()),
                level: Some(42.0),
                ..streams[0].clone()
            },
            streams[1].clone(),
        ];
        let constrained_profile = StreamQuery {
            profile: Some("Main".to_owned()),
            ..query.clone()
        };
        assert!(!can_copy_remux(
            &constrained_profile,
            "mp4",
            &high_profile_streams,
            "h264",
            "aac",
        ));
        let lower_profile = StreamQuery {
            profile: Some("High".to_owned()),
            level: Some("41".to_owned()),
            ..query.clone()
        };
        assert!(!can_copy_remux(
            &lower_profile,
            "mp4",
            &high_profile_streams,
            "h264",
            "aac",
        ));

        let resized = StreamQuery {
            max_width: Some(1280),
            ..query
        };
        assert!(copy_remux_has_no_transform(&resized));
        assert!(
            !can_copy_remux(&resized, "mp4", &streams, "h264", "aac"),
            "unknown source dimensions require an encode under the official check"
        );
        let no_auto_copy = StreamQuery {
            enable_auto_stream_copy: Some(false),
            ..StreamQuery::default()
        };
        assert!(!copy_remux_has_no_transform(&no_auto_copy));
        assert!(!can_copy_remux(
            &StreamQuery {
                video_codec: Some("hevc".to_owned()),
                audio_codec: Some("aac".to_owned()),
                ..StreamQuery::default()
            },
            "mp4",
            &streams,
            "hevc",
            "aac",
        ));
        assert!(!is_local_path("https://media.example/video.mkv"));
        assert!(!is_local_path("rtsp://media.example/video.mkv"));
        assert!(is_local_path("/library/video.mkv"));
    }

    #[test]
    fn progressive_copy_uses_codec_qualified_stream_option_constraints() {
        let streams = vec![
            MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("h264".to_owned()),
                profile: Some("High".to_owned()),
                level: Some(41.0),
                ref_frames: Some(4),
                bit_depth: Some(10),
                rotation: Some(90),
                video_range_type: VideoRangeType::Hdr10,
                ..MediaStream::default()
            },
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                codec: Some("aac".to_owned()),
                channels: Some(6),
                bit_depth: Some(24),
                ..MediaStream::default()
            },
        ];
        let query = StreamQuery {
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            ..StreamQuery::default()
        };

        for query_string in [
            "h264-profile=Baseline",
            "h264-level=40",
            "h264-maxrefframes=3",
            "h264-videobitdepth=8",
            "aac-audiobitdepth=16",
            "aac-audiochannels=2",
            "h264-rangetype=SDR",
            "h264-rotation=0",
        ] {
            let uri: Uri = format!("/Videos/id/stream?{query_string}").parse().unwrap();
            let options = StreamOptions::from_uri(&uri);
            assert!(
                !can_copy_remux_with_options(&query, &options, "mp4", &streams, "h264", "aac",),
                "{query_string}"
            );
        }
    }

    #[test]
    fn empty_declared_profile_falls_back_to_qualified_stream_option() {
        let streams = vec![
            MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("h264".to_owned()),
                profile: Some("High".to_owned()),
                ..MediaStream::default()
            },
            MediaStream {
                index: 1,
                stream_type: MediaStreamType::Audio,
                codec: Some("aac".to_owned()),
                ..MediaStream::default()
            },
        ];
        let query = StreamQuery {
            profile: Some(String::new()),
            video_codec: Some("h264".to_owned()),
            audio_codec: Some("aac".to_owned()),
            ..StreamQuery::default()
        };
        let uri: Uri = "/Videos/id/stream?profile=&h264-profile=Baseline"
            .parse()
            .unwrap();
        let options = StreamOptions::from_uri(&uri);

        assert!(!can_copy_remux_with_options(
            &query, &options, "mp4", &streams, "h264", "aac",
        ));
    }

    #[test]
    fn video_profile_and_level_follow_official_copy_and_encoder_rules() {
        assert!(video_profile_allows_copy(
            "h264",
            Some("Constrained Baseline"),
            Some("Main"),
        ));
        assert!(video_profile_allows_copy("h264", None, Some("Baseline")));
        assert!(!video_profile_allows_copy(
            "h264",
            Some("High"),
            Some("Main"),
        ));
        assert!(video_level_allows_copy(Some(41.0), Some("41")));
        assert!(!video_level_allows_copy(Some(42.0), Some("41")));
        assert_eq!(
            output_video_profile("h264", Some("High")),
            Some("high".to_owned())
        );
        assert_eq!(output_video_profile("h264", Some("unknown")), None);
        assert_eq!(
            output_video_level("h264", Some("99")),
            Some("51".to_owned())
        );
        assert_eq!(
            output_video_level("hevc", Some("153")),
            Some("150".to_owned())
        );
        assert_eq!(output_video_level("av1", Some("15")), Some("15".to_owned()));
    }

    #[tokio::test]
    async fn remote_stream_forwards_ranges_without_buffering_the_upstream_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock upstream listener");
        let address = listener.local_addr().expect("mock upstream address");
        let (release_sender, release_receiver) = mpsc::channel();
        let upstream = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("upstream request");
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("upstream read timeout");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).expect("upstream request bytes");
                assert_ne!(read, 0, "request ended before its headers");
                request.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Type: video/x-matroska\r\nContent-Length: 10\r\nContent-Range: bytes 20-29/100\r\nConnection: close\r\n\r\nhello",
                )
                .expect("first upstream chunk");
            socket.flush().expect("flush upstream headers");
            release_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("proxy returned before the complete body");
            socket.write_all(b"world").expect("last upstream chunk");
            String::from_utf8(request).expect("HTTP request text")
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            header::RANGE,
            header::HeaderValue::from_static("bytes=20-29"),
        );
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            proxy_remote_stream(
                &reqwest::Client::new(),
                &headers,
                Uuid::new_v4(),
                &format!("http://{address}/signed/video.mkv?token=secret"),
                Some("Jellyfin Remote Stream Test"),
            ),
        )
        .await
        .expect("proxy must return after upstream headers")
        .expect("remote response");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 20-29/100");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/x-matroska");
        release_sender.send(()).expect("release upstream body");
        assert_eq!(
            to_bytes(response.into_body(), 10)
                .await
                .expect("proxied body"),
            "helloworld"
        );
        let request = upstream.join().expect("mock upstream thread");
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("range: bytes=20-29")),
            "{request}"
        );
        assert!(
            request.lines().any(|line| {
                line.eq_ignore_ascii_case("user-agent: Jellyfin Remote Stream Test")
            }),
            "{request}"
        );
    }

    #[tokio::test]
    async fn unavailable_remote_stream_maps_to_bad_gateway() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("unused listener");
        let address = listener.local_addr().expect("unused listener address");
        drop(listener);
        let error = proxy_remote_stream(
            &reqwest::Client::new(),
            &HeaderMap::new(),
            Uuid::new_v4(),
            &format!("http://{address}/video.mkv"),
            None,
        )
        .await
        .expect_err("closed upstream port must fail");
        assert!(matches!(error, ApiError::UpstreamUnavailable));
    }
}
