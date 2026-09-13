//! Emby encoding discovery backed by the server's ffmpeg capability probe.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use jellyfin_api::AppState;
use serde::Serialize;
use serde_json::{Value, json};

const FULL_TONE_MAP_OPTIONS: &str = "full-tone-map-options";
const PUBLIC_TONE_MAP_OPTIONS: &str = "public-tone-map-options";
const SUBTITLE_OPTIONS: &str = "subtitle-options";
const FFMPEG_OPTIONS: &str = "ffmpeg-options";

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
        .route("/Encoding/ToneMapOptions", get(tone_map_options))
        .route("/encoding/tonemapoptions", get(tone_map_options))
        .route(
            "/Encoding/FullToneMapOptions",
            get(full_tone_map_options).post(update_full_tone_map_options),
        )
        .route(
            "/encoding/fulltonemapoptions",
            get(full_tone_map_options).post(update_full_tone_map_options),
        )
        .route(
            "/Encoding/PublicToneMapOptions",
            get(public_tone_map_options).post(update_public_tone_map_options),
        )
        .route(
            "/encoding/publictonemapoptions",
            get(public_tone_map_options).post(update_public_tone_map_options),
        )
        .route(
            "/Encoding/SubtitleOptions",
            get(subtitle_options).post(update_subtitle_options),
        )
        .route(
            "/encoding/subtitleoptions",
            get(subtitle_options).post(update_subtitle_options),
        )
        .route(
            "/Encoding/FfmpegOptions",
            get(ffmpeg_options).post(update_ffmpeg_options),
        )
        .route(
            "/encoding/ffmpegoptions",
            get(ffmpeg_options).post(update_ffmpeg_options),
        )
        .route(
            "/Encoding/CodecParameters",
            get(codec_parameters).post(update_codec_parameters),
        )
        .route(
            "/encoding/codecparameters",
            get(codec_parameters).post(update_codec_parameters),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct EditObjectContainer {
    object: Value,
    default_object: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    editor_root: Option<Value>,
}

impl EditObjectContainer {
    fn new(object: Value) -> Self {
        Self {
            object,
            default_object: json!({}),
            // The checked-in Emby SDKs intentionally model these editor
            // objects as opaque values. Emitting invented CLR names or editor
            // descriptors would be less compatible than omitting the SDK's
            // nullable metadata until Emby's closed implementation is known.
            type_name: None,
            editor_root: None,
        }
    }
}

async fn full_tone_map_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<EditObjectContainer>, Response> {
    load_editor(&state, &headers, &uri, FULL_TONE_MAP_OPTIONS).await
}

async fn public_tone_map_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<EditObjectContainer>, Response> {
    load_editor(&state, &headers, &uri, PUBLIC_TONE_MAP_OPTIONS).await
}

async fn subtitle_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<EditObjectContainer>, Response> {
    load_editor(&state, &headers, &uri, SUBTITLE_OPTIONS).await
}

async fn ffmpeg_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<EditObjectContainer>, Response> {
    load_editor(&state, &headers, &uri, FFMPEG_OPTIONS).await
}

async fn update_full_tone_map_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    save_editor(&state, &headers, &uri, FULL_TONE_MAP_OPTIONS, payload).await
}

async fn update_public_tone_map_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    save_editor(&state, &headers, &uri, PUBLIC_TONE_MAP_OPTIONS, payload).await
}

async fn update_subtitle_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    save_editor(&state, &headers, &uri, SUBTITLE_OPTIONS, payload).await
}

async fn update_ffmpeg_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    save_editor(&state, &headers, &uri, FFMPEG_OPTIONS, payload).await
}

async fn load_editor(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    key: &str,
) -> Result<Json<EditObjectContainer>, Response> {
    state.require_emby_user(headers, uri).await?;
    let object = state
        .emby_encoding_configuration(key)
        .await?
        .unwrap_or_else(|| json!({}));
    Ok(Json(EditObjectContainer::new(object)))
}

async fn save_editor(
    state: &AppState,
    headers: &HeaderMap,
    uri: &axum::http::Uri,
    key: &str,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    // Authorization intentionally precedes body parsing so a malformed body
    // cannot disclose this administrator-only mutation surface.
    state.require_emby_administrator(headers, uri).await?;
    let payload = payload.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let object: Value =
        serde_json::from_slice(&payload).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    if !object.is_object() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    state.save_emby_encoding_configuration(key, object).await?;
    Ok(StatusCode::OK)
}

async fn codec_parameters(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<EditObjectContainer>, Response> {
    state.require_emby_user(&headers, &uri).await?;
    let query = CodecParametersQuery::parse(&uri)?;
    let object = state
        .emby_encoding_configuration(&query.persistence_key())
        .await?
        .unwrap_or_else(|| json!({}));
    Ok(Json(EditObjectContainer::new(object)))
}

async fn update_codec_parameters(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let query = CodecParametersQuery::parse(&uri)?;
    let payload = payload.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let object: Value =
        serde_json::from_slice(&payload).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    if !object.is_object() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    state
        .save_emby_encoding_configuration(&query.persistence_key(), object)
        .await?;
    Ok(StatusCode::OK)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodecParameterContext {
    Playback,
    Conversion,
}

impl CodecParameterContext {
    fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("Playback") || value == "0" {
            Some(Self::Playback)
        } else if value.eq_ignore_ascii_case("Conversion") || value == "1" {
            Some(Self::Conversion)
        } else {
            None
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Playback => "playback",
            Self::Conversion => "conversion",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CodecParametersQuery {
    codec_id: String,
    context: CodecParameterContext,
}

impl CodecParametersQuery {
    fn parse(uri: &axum::http::Uri) -> Result<Self, Response> {
        let mut codec_id = None;
        let mut context = None::<String>;
        for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
            if name.eq_ignore_ascii_case("CodecId") {
                codec_id = Some(value.into_owned());
            } else if name.eq_ignore_ascii_case("ParameterContext") {
                context = Some(value.into_owned());
            }
        }
        let codec_id = codec_id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
        let context = context
            .as_deref()
            .and_then(CodecParameterContext::parse)
            .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())?;
        Ok(Self { codec_id, context })
    }

    fn persistence_key(&self) -> String {
        let mut encoded = String::with_capacity(self.codec_id.len() * 2);
        for byte in self.codec_id.as_bytes() {
            use std::fmt::Write as _;
            write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
        }
        format!("codec-parameters-{}-{encoded}", self.context.key())
    }
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
struct ToneMapOptions {
    show_advanced: bool,
    is_software_tone_mapping_available: bool,
    is_any_hardware_tone_mapping_available: bool,
    show_nvidia_options: bool,
    show_quick_sync_options: bool,
    show_vaapi_options: bool,
    is_open_cl_available: bool,
    is_open_cl_super_t_available: bool,
    is_vaapi_native_available: bool,
    is_quick_sync_native_available: bool,
    operating_system: &'static str,
}

async fn tone_map_options(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<ToneMapOptions>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let (_, decoders) = state.encoder_codec_names();
    Ok(Json(ToneMapOptions {
        show_advanced: false,
        is_software_tone_mapping_available: !decoders.is_empty(),
        is_any_hardware_tone_mapping_available: false,
        show_nvidia_options: false,
        show_quick_sync_options: false,
        show_vaapi_options: false,
        is_open_cl_available: false,
        is_open_cl_super_t_available: false,
        is_vaapi_native_available: false,
        is_quick_sync_native_available: false,
        operating_system: std::env::consts::OS,
    }))
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
    use super::{CodecParameterContext, CodecParametersQuery, is_video_codec};

    #[test]
    fn codec_filter_excludes_audio() {
        assert!(is_video_codec("libx264"));
        assert!(is_video_codec("hevc_nvenc"));
        assert!(!is_video_codec("libopus"));
    }

    #[test]
    fn codec_parameter_query_matches_sdk_names_case_insensitively() {
        let query = CodecParametersQuery::parse(
            &"/Encoding/CodecParameters?cOdEcId=h264%2Bnvenc&pArAmEtErCoNtExT=conversion"
                .parse()
                .expect("valid URI"),
        )
        .expect("query binding");
        assert_eq!(query.codec_id, "h264+nvenc");
        assert_eq!(query.context, CodecParameterContext::Conversion);
        assert_eq!(
            query.persistence_key(),
            "codec-parameters-conversion-683236342b6e76656e63"
        );
    }

    #[test]
    fn codec_parameter_query_uses_last_duplicate_and_defined_enum_integers() {
        let query = CodecParametersQuery::parse(
            &"/encoding/codecparameters?CodecId=old&CODECID=hevc&ParameterContext=invalid&parametercontext=0"
                .parse()
                .expect("valid URI"),
        )
        .expect("query binding");
        assert_eq!(query.codec_id, "hevc");
        assert_eq!(query.context, CodecParameterContext::Playback);
    }

    #[test]
    fn codec_parameter_query_rejects_missing_empty_and_unknown_values() {
        for uri in [
            "/Encoding/CodecParameters",
            "/Encoding/CodecParameters?CodecId=&ParameterContext=Playback",
            "/Encoding/CodecParameters?CodecId=h264",
            "/Encoding/CodecParameters?CodecId=h264&ParameterContext=2",
        ] {
            assert!(
                CodecParametersQuery::parse(&uri.parse().expect("valid URI")).is_err(),
                "{uri}"
            );
        }
    }
}
