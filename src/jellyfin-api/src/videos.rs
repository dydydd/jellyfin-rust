use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use axum_extra::extract::Query;
use jellyfin_controller::video_command;
use jellyfin_data::BaseItemPage;
use serde::Deserialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, user_library};

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
    #[serde(rename = "videoCodec", alias = "VideoCodec", alias = "videocodec")]
    video_codec: Option<String>,
    #[serde(rename = "audioCodec", alias = "AudioCodec", alias = "audiocodec")]
    audio_codec: Option<String>,
    #[serde(
        rename = "videoBitRate",
        alias = "VideoBitRate",
        alias = "videobitrate"
    )]
    video_bitrate: Option<i64>,
    #[serde(
        rename = "audioBitRate",
        alias = "AudioBitRate",
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
        rename = "transcodingMaxAudioChannels",
        alias = "TranscodingMaxAudioChannels",
        alias = "transcodingmaxaudiochannels"
    )]
    transcoding_max_audio_channels: Option<i32>,
    #[serde(rename = "maxWidth", alias = "MaxWidth", alias = "maxwidth")]
    max_width: Option<i32>,
    #[serde(rename = "maxHeight", alias = "MaxHeight", alias = "maxheight")]
    max_height: Option<i32>,
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
        rename = "subtitleMethod",
        alias = "SubtitleMethod",
        alias = "subtitlemethod"
    )]
    subtitle_method: Option<String>,
    #[serde(
        rename = "startTimeTicks",
        alias = "StartTimeTicks",
        alias = "starttimeticks"
    )]
    start_time_ticks: Option<i64>,
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

async fn stream_file(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: Uuid,
    requested_container: Option<&str>,
    query: StreamQuery,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let authenticated =
        authentication::authenticated_session_for_uri(&state, &headers, request.uri()).await?;
    let requested_item = state
        .library_controller
        .item(&authenticated.user, authenticated.user.id, item_id)
        .await?;
    if !matches!(
        requested_item.item_type.as_str(),
        "Video" | "Movie" | "Episode" | "MusicVideo" | "Trailer"
    ) {
        return Err(ApiError::NotFound);
    }
    let item = if let Some(media_source_id) = query
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
    let path = jellyfin_controller::media_source_path(&item)
        .map(str::to_owned)
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
            return proxy_remote_stream(&state.remote_stream_client, &headers, item_id, &path)
                .await;
        }
        return crate::audio::serve_path(headers, &path, request).await;
    }

    let container = requested_container
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".to_owned());
    let video_codec = query
        .video_codec
        .as_deref()
        .unwrap_or_else(|| video_codec_for_container(&container));
    let audio_codec = query
        .audio_codec
        .as_deref()
        .unwrap_or_else(|| audio_codec_for_container(&container));
    let output = state
        .transcode_directory
        .join(format!("{item_id}-video-{video_codec}.{container}"));
    tokio::fs::create_dir_all(&state.transcode_directory)
        .await
        .map_err(|_| ApiError::Internal)?;
    let command = video_command(
        &state.ffmpeg_path,
        std::path::Path::new(&path),
        &output,
        video_codec,
        audio_codec,
        query.video_bitrate,
        query.audio_bitrate,
        query
            .audio_channels
            .or(query.max_audio_channels)
            .or(query.transcoding_max_audio_channels),
        query.audio_sample_rate,
        query.max_width,
        query.max_height,
        query.audio_stream_index,
        query.video_stream_index,
        query.start_time_ticks,
        query
            .subtitle_stream_index
            .filter(|_| should_burn_subtitles(query.subtitle_method.as_deref())),
    );
    crate::audio::serve_transcoded_path(
        command,
        &output.to_string_lossy(),
        request.method() == axum::http::Method::HEAD,
    )
    .await
}

fn should_burn_subtitles(method: Option<&str>) -> bool {
    method.is_none_or(|method| {
        let method = method.trim();
        method.eq_ignore_ascii_case("Encode") || method == "0"
    })
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

async fn proxy_remote_stream(
    client: &reqwest::Client,
    client_headers: &HeaderMap,
    item_id: Uuid,
    path: &str,
) -> Result<Response, ApiError> {
    let mut request = client.get(path);
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
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .clear_alternate_sources(&authenticated.user, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn merge_versions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MergeVersionsQuery>,
) -> Result<StatusCode, ApiError> {
    let authenticated = authentication::authenticated_session(&state, &headers).await?;
    state
        .videos
        .merge_versions(&authenticated.user, &query.ids)
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
    let target_user_id = query.user_id.unwrap_or(authenticated.user.id);
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
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        time::Duration,
    };

    use axum::{
        body::to_bytes,
        http::{Uri, header},
    };
    use axum_extra::extract::Query;

    use super::*;

    #[test]
    fn video_stream_binds_android_progressive_parameters() {
        let uri: Uri = "/Videos/item/stream.mp4?static=false&videoCodec=h264&audioCodec=aac&videoBitRate=2000000&maxWidth=1280&audioStreamIndex=2&videoStreamIndex=0&subtitleStreamIndex=3&subtitleMethod=Encode&startTimeTicks=10000"
            .parse()
            .unwrap();
        let query = Query::<StreamQuery>::try_from_uri(&uri).unwrap().0;
        assert!(!query.static_stream.unwrap());
        assert_eq!(query.video_codec.as_deref(), Some("h264"));
        assert_eq!(query.audio_codec.as_deref(), Some("aac"));
        assert_eq!(query.video_bitrate, Some(2_000_000));
        assert_eq!(query.max_width, Some(1280));
        assert_eq!(query.audio_stream_index, Some(2));
        assert_eq!(query.video_stream_index, Some(0));
        assert_eq!(query.subtitle_stream_index, Some(3));
        assert_eq!(query.subtitle_method.as_deref(), Some("Encode"));
        assert_eq!(query.start_time_ticks, Some(10_000));
        assert_eq!(video_codec_for_container("mp4"), "h264");
        assert_eq!(audio_codec_for_container("webm"), "opus");
        assert!(should_burn_subtitles(Some("Encode")));
        assert!(should_burn_subtitles(Some("0")));
        assert!(!should_burn_subtitles(Some("External")));
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
        )
        .await
        .expect_err("closed upstream port must fail");
        assert!(matches!(error, ApiError::UpstreamUnavailable));
    }
}
