//! Emby encoding discovery backed by the server's ffmpeg capability probe.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Encoding/CodecInformation/Video",
            get(codec_information_video),
        )
        .route(
            "/encoding/codecinformation/video",
            get(codec_information_video),
        )
        .route(
            "/Encoding/CodecConfiguration/Defaults",
            get(codec_configuration_defaults),
        )
        .route(
            "/encoding/codecconfiguration/defaults",
            get(codec_configuration_defaults),
        )
}

async fn codec_information_video(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<VideoCodec>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let (encoders, decoders) = state.encoder_codec_names();
    let mut codecs = Vec::new();
    for (direction, names) in [("Encoder", encoders), ("Decoder", decoders)] {
        for name in names.into_iter().filter(|name| is_video_codec(name)) {
            codecs.push(VideoCodec {
                id: name.clone(),
                name: name.clone(),
                description: name.clone(),
                media_type_name: "Video",
                codec_kind: "Video",
                direction,
                framework_codec: name,
                is_hardware_codec: false,
                is_enabled_by_default: direction == "Encoder",
                default_priority: 0,
            });
        }
    }
    Ok(Json(codecs))
}

async fn codec_configuration_defaults(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<CodecConfiguration>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let (encoders, _) = state.encoder_codec_names();
    Ok(Json(
        encoders
            .into_iter()
            .filter(|name| is_video_codec(name))
            .map(|codec_id| CodecConfiguration {
                codec_id,
                is_enabled: true,
                priority: 0,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct VideoCodec {
    id: String,
    name: String,
    description: String,
    media_type_name: &'static str,
    codec_kind: &'static str,
    direction: &'static str,
    framework_codec: String,
    is_hardware_codec: bool,
    is_enabled_by_default: bool,
    default_priority: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CodecConfiguration {
    codec_id: String,
    is_enabled: bool,
    priority: i32,
}

fn is_video_codec(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "av1", "h264", "x264", "hevc", "h265", "x265", "vp8", "vp9", "mpeg", "theora", "prores",
        "vc1", "wmv",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::is_video_codec;

    #[test]
    fn codec_filter_excludes_audio() {
        assert!(is_video_codec("libx264"));
        assert!(is_video_codec("hevc_nvenc"));
        assert!(!is_video_codec("libopus"));
    }
}
