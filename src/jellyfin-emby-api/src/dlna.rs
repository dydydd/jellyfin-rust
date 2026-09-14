//! Emby DLNA profile management.
//!
//! Emby 4.10 stores administrator-created profiles separately from its
//! bundled system profiles. The compatibility layer keeps those opaque
//! protocol-owned profiles in PostgreSQL and never registers them as
//! Jellyfin playback profiles, so neither root nor `/api` behavior changes.

#![allow(clippy::result_large_err)]

use std::{cmp::Ordering, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, Path, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use jellyfin_api::AppState;
use md5::{Digest, Md5};
use serde_json::{Map, Value, json};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Dlna/ProfileInfos", get(profile_infos))
        .route("/dlna/profileinfos", get(profile_infos))
        .route("/Dlna/Profiles", post(create_profile))
        .route("/dlna/profiles", post(create_profile))
        // Register the literal segment alongside the dynamic route. The
        // outer Emby router additionally normalizes mixed-case literals.
        .route("/Dlna/Profiles/Default", get(default_profile))
        .route("/dlna/profiles/default", get(default_profile))
        .route(
            "/Dlna/Profiles/{id}",
            get(get_profile).post(update_profile).delete(delete_profile),
        )
        .route(
            "/dlna/profiles/{id}",
            get(get_profile).post(update_profile).delete(delete_profile),
        )
}

async fn profile_infos(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let mut profiles = state.emby_dlna_profiles().await?;
    profiles.sort_by(profile_name_order);
    Ok(Json(profiles))
}

async fn default_profile(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<Value>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    Ok(Json(official_default_profile()))
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let id = required_id(&id)?;
    state
        .emby_dlna_profile(id)
        .await?
        .map(Json)
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())
}

async fn create_profile(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    // Emby's service-level Admin role is evaluated before request DTO
    // binding. Preserve that precedence for malformed JSON too.
    state.require_emby_administrator(&headers, &uri).await?;
    let mut profile = parse_profile(payload)?;
    let name = profile_name(&profile)?;
    let id = profile
        .get("Id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map_or_else(|| profile_id_for_name(name), ToOwned::to_owned);
    let object = profile
        .as_object_mut()
        .expect("profile normalization always returns an object");
    object.insert("Id".to_owned(), Value::String(id.clone()));
    object.insert("Type".to_owned(), Value::String("User".to_owned()));
    force_video_detection_off(object);
    state.save_emby_dlna_profile(&id, profile).await?;
    Ok(StatusCode::OK)
}

async fn update_profile(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let id = required_id(&id)?;
    let mut profile = parse_profile(payload)?;
    profile_name(&profile)?;
    if state.emby_dlna_profile(id).await?.is_none() {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    let object = profile
        .as_object_mut()
        .expect("profile normalization always returns an object");
    // ServiceStack binds the path Id onto the inherited request DTO. A body
    // Id using any casing therefore cannot redirect the update to another
    // persisted profile.
    object.insert("Id".to_owned(), Value::String(id.to_owned()));
    object.insert("Type".to_owned(), Value::String("User".to_owned()));
    force_video_detection_off(object);
    state.save_emby_dlna_profile(id, profile).await?;
    Ok(StatusCode::OK)
}

async fn delete_profile(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, Response> {
    state.require_emby_administrator(&headers, &uri).await?;
    let id = required_id(&id)?;
    if !state.delete_emby_dlna_profile(id).await? {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    Ok(StatusCode::OK)
}

fn required_id(id: &str) -> Result<&str, Response> {
    if id.is_empty() {
        Err(StatusCode::BAD_REQUEST.into_response())
    } else {
        Ok(id)
    }
}

fn parse_profile(payload: Result<Bytes, BytesRejection>) -> Result<Value, Response> {
    let payload = payload.map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let value: Value =
        serde_json::from_slice(&payload).map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    normalize_profile(value).map_err(|()| StatusCode::BAD_REQUEST.into_response())
}

fn profile_name(profile: &Value) -> Result<&str, Response> {
    profile
        .get("Name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| StatusCode::BAD_REQUEST.into_response())
}

fn profile_name_order(left: &Value, right: &Value) -> Ordering {
    let left = left.get("Name").and_then(Value::as_str).unwrap_or_default();
    let right = right
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    left.to_ascii_lowercase()
        .cmp(&right.to_ascii_lowercase())
        .then_with(|| left.cmp(right))
}

fn force_video_detection_off(profile: &mut Map<String, Value>) {
    let detection = profile
        .entry("ProtocolInfoDetection".to_owned())
        .or_insert_with(protocol_detection_defaults);
    if let Some(detection) = detection.as_object_mut() {
        detection.insert("EnabledForVideo".to_owned(), Value::Bool(false));
    }
}

fn profile_id_for_name(name: &str) -> String {
    let filename: String = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    // Emby derives a Guid from MD5(UTF-16LE(full XML path)). The Rust server
    // has no corresponding profile file, so use a stable protocol-private
    // virtual path while preserving the same UTF-16LE and Guid byte layout.
    dotnet_md5_guid_n(&format!("dlna/user/{filename}.xml"))
}

fn dotnet_md5_guid_n(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len() * 2);
    for unit in value.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let digest = Md5::digest(bytes);
    let order = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let mut id = String::with_capacity(32);
    for index in order {
        use std::fmt::Write;
        write!(id, "{:02x}", digest[index]).expect("writing to String cannot fail");
    }
    id
}

fn normalize_profile(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = profile_defaults();
    for (name, value) in input {
        if key(&name, "Type") {
            set_enum(&mut output, "Type", value, DEVICE_PROFILE_TYPES)?;
        } else if key(&name, "Path") {
            // Emby's Path property is ignored by its wire serializer and is
            // server-owned. Never accept or disclose a client filesystem path.
        } else if let Some(canonical) = canonical(
            &name,
            &[
                "UserId",
                "AlbumArtPn",
                "FriendlyName",
                "Manufacturer",
                "ManufacturerUrl",
                "ModelName",
                "ModelDescription",
                "ModelNumber",
                "ModelUrl",
                "SerialNumber",
                "ProtocolInfo",
                "Name",
                "Id",
                "SupportedMediaTypes",
            ],
        ) {
            set_optional_string(&mut output, canonical, value)?;
        } else if let Some(canonical) = canonical(
            &name,
            &[
                "MaxAlbumArtWidth",
                "MaxAlbumArtHeight",
                "TimelineOffsetSeconds",
            ],
        ) {
            set_i32(&mut output, canonical, value)?;
        } else if let Some(canonical) = canonical(&name, &["MaxIconWidth", "MaxIconHeight"]) {
            set_optional_i32(&mut output, canonical, value)?;
        } else if let Some(canonical) = canonical(
            &name,
            &[
                "EnableAlbumArtInDidl",
                "EnableSingleAlbumArtLimit",
                "EnableSingleSubtitleLimit",
                "RequiresPlainVideoItems",
                "RequiresPlainFolders",
                "IgnoreTranscodeByteRangeRequests",
                "SupportsSamsungBookmark",
            ],
        ) {
            set_bool(&mut output, canonical, value)?;
        } else if key(&name, "Identification") {
            set_optional_object(
                &mut output,
                "Identification",
                value,
                normalize_identification,
            )?;
        } else if key(&name, "ProtocolInfoDetection") {
            output.insert(
                "ProtocolInfoDetection".to_owned(),
                normalize_protocol_detection(value)?,
            );
        } else if key(&name, "MaxStreamingBitrate") {
            set_optional_i64(&mut output, "MaxStreamingBitrate", value)?;
        } else if let Some(canonical) = canonical(
            &name,
            &["MusicStreamingTranscodingBitrate", "MaxStaticMusicBitrate"],
        ) {
            set_optional_i32(&mut output, canonical, value)?;
        } else if key(&name, "DeclaredFeatures") {
            set_string_array(&mut output, "DeclaredFeatures", value)?;
        } else if key(&name, "DirectPlayProfiles") {
            set_object_array(
                &mut output,
                "DirectPlayProfiles",
                value,
                normalize_direct_play,
            )?;
        } else if key(&name, "TranscodingProfiles") {
            set_object_array(
                &mut output,
                "TranscodingProfiles",
                value,
                normalize_transcoding,
            )?;
        } else if key(&name, "ContainerProfiles") {
            set_object_array(&mut output, "ContainerProfiles", value, normalize_container)?;
        } else if key(&name, "CodecProfiles") {
            set_object_array(&mut output, "CodecProfiles", value, normalize_codec)?;
        } else if key(&name, "ResponseProfiles") {
            set_object_array(&mut output, "ResponseProfiles", value, normalize_response)?;
        } else if key(&name, "SubtitleProfiles") {
            set_object_array(&mut output, "SubtitleProfiles", value, normalize_subtitle)?;
        }
    }
    Ok(Value::Object(output))
}

fn profile_defaults() -> Map<String, Value> {
    Map::from_iter([
        ("Type".to_owned(), Value::String("System".to_owned())),
        ("MaxAlbumArtWidth".to_owned(), json!(0)),
        ("MaxAlbumArtHeight".to_owned(), json!(0)),
        ("EnableAlbumArtInDidl".to_owned(), json!(false)),
        ("EnableSingleAlbumArtLimit".to_owned(), json!(false)),
        ("EnableSingleSubtitleLimit".to_owned(), json!(false)),
        ("TimelineOffsetSeconds".to_owned(), json!(0)),
        ("RequiresPlainVideoItems".to_owned(), json!(false)),
        ("RequiresPlainFolders".to_owned(), json!(false)),
        ("IgnoreTranscodeByteRangeRequests".to_owned(), json!(false)),
        ("SupportsSamsungBookmark".to_owned(), json!(false)),
        (
            "ProtocolInfoDetection".to_owned(),
            protocol_detection_defaults(),
        ),
        (
            "SupportedMediaTypes".to_owned(),
            Value::String("Audio,Photo,Video".to_owned()),
        ),
        ("MaxStreamingBitrate".to_owned(), json!(8_000_000_i64)),
        (
            "MusicStreamingTranscodingBitrate".to_owned(),
            json!(256_000),
        ),
        ("DeclaredFeatures".to_owned(), json!([])),
        ("DirectPlayProfiles".to_owned(), json!([])),
        ("TranscodingProfiles".to_owned(), json!([])),
        ("ContainerProfiles".to_owned(), json!([])),
        ("CodecProfiles".to_owned(), json!([])),
        ("ResponseProfiles".to_owned(), json!([])),
        ("SubtitleProfiles".to_owned(), json!([])),
    ])
}

fn normalize_protocol_detection(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = protocol_detection_defaults()
        .as_object()
        .expect("defaults are an object")
        .clone();
    for (name, value) in input {
        if key(&name, "EnabledForVideo") {
            set_bool(&mut output, "EnabledForVideo", value)?;
        } else if key(&name, "EnabledForAudio") {
            set_bool(&mut output, "EnabledForAudio", value)?;
        } else if key(&name, "EnabledForPhotos") {
            set_bool(&mut output, "EnabledForPhotos", value)?;
        }
    }
    Ok(Value::Object(output))
}

fn protocol_detection_defaults() -> Value {
    json!({
        "EnabledForVideo": false,
        "EnabledForAudio": true,
        "EnabledForPhotos": true
    })
}

fn normalize_identification(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([("Headers".to_owned(), json!([]))]);
    for (name, value) in input {
        if let Some(canonical) = canonical(
            &name,
            &[
                "FriendlyName",
                "ModelNumber",
                "SerialNumber",
                "ModelName",
                "ModelDescription",
                "DeviceDescription",
                "ModelUrl",
                "Manufacturer",
                "ManufacturerUrl",
            ],
        ) {
            set_optional_string(&mut output, canonical, value)?;
        } else if key(&name, "Headers") {
            set_object_array(&mut output, "Headers", value, normalize_header)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_header(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([("Match".to_owned(), json!("Equals"))]);
    for (name, value) in input {
        if key(&name, "Name") {
            set_optional_string(&mut output, "Name", value)?;
        } else if key(&name, "Value") {
            set_optional_string(&mut output, "Value", value)?;
        } else if key(&name, "Match") {
            set_enum(&mut output, "Match", value, HEADER_MATCH_TYPES)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_direct_play(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([("Type".to_owned(), json!("Audio"))]);
    for (name, value) in input {
        if let Some(canonical) = canonical(&name, &["Container", "AudioCodec", "VideoCodec"]) {
            set_optional_string(&mut output, canonical, value)?;
        } else if key(&name, "Type") {
            set_enum(&mut output, "Type", value, DLNA_PROFILE_TYPES)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_transcoding(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Type".to_owned(), json!("Audio")),
        ("EstimateContentLength".to_owned(), json!(false)),
        ("EnableMpegtsM2TsMode".to_owned(), json!(false)),
        ("TranscodeSeekInfo".to_owned(), json!("Auto")),
        ("CopyTimestamps".to_owned(), json!(false)),
        ("Context".to_owned(), json!("Streaming")),
        ("MinSegments".to_owned(), json!(0)),
        ("SegmentLength".to_owned(), json!(0)),
        ("BreakOnNonKeyFrames".to_owned(), json!(false)),
        ("AllowInterlacedVideoStreamCopy".to_owned(), json!(false)),
        ("MaxManifestSubtitles".to_owned(), json!(0)),
        ("MaxWidth".to_owned(), json!(0)),
        ("MaxHeight".to_owned(), json!(0)),
        ("FillEmptySubtitleSegments".to_owned(), json!(false)),
    ]);
    for (name, value) in input {
        if let Some(canonical) = canonical(
            &name,
            &[
                "Container",
                "VideoCodec",
                "AudioCodec",
                "Protocol",
                "MaxAudioChannels",
                "ManifestSubtitles",
            ],
        ) {
            set_optional_string(&mut output, canonical, value)?;
        } else if key(&name, "Type") {
            set_enum(&mut output, "Type", value, DLNA_PROFILE_TYPES)?;
        } else if key(&name, "TranscodeSeekInfo") {
            set_enum(
                &mut output,
                "TranscodeSeekInfo",
                value,
                TRANSCODE_SEEK_TYPES,
            )?;
        } else if key(&name, "Context") {
            set_enum(&mut output, "Context", value, ENCODING_CONTEXT_TYPES)?;
        } else if let Some(canonical) = canonical(
            &name,
            &[
                "EstimateContentLength",
                "EnableMpegtsM2TsMode",
                "CopyTimestamps",
                "BreakOnNonKeyFrames",
                "AllowInterlacedVideoStreamCopy",
                "FillEmptySubtitleSegments",
            ],
        ) {
            set_bool(&mut output, canonical, value)?;
        } else if let Some(canonical) = canonical(
            &name,
            &[
                "MinSegments",
                "SegmentLength",
                "MaxManifestSubtitles",
                "MaxWidth",
                "MaxHeight",
            ],
        ) {
            set_i32(&mut output, canonical, value)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_container(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Type".to_owned(), json!("Audio")),
        ("Conditions".to_owned(), json!([])),
    ]);
    for (name, value) in input {
        if key(&name, "Type") {
            set_enum(&mut output, "Type", value, DLNA_PROFILE_TYPES)?;
        } else if key(&name, "Container") {
            set_optional_string(&mut output, "Container", value)?;
        } else if key(&name, "Conditions") {
            set_object_array(&mut output, "Conditions", value, normalize_condition)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_codec(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Type".to_owned(), json!("Video")),
        ("Conditions".to_owned(), json!([])),
        ("ApplyConditions".to_owned(), json!([])),
    ]);
    for (name, value) in input {
        if key(&name, "Type") {
            set_enum(&mut output, "Type", value, CODEC_TYPES)?;
        } else if key(&name, "Codec") {
            set_optional_string(&mut output, "Codec", value)?;
        } else if key(&name, "Container") {
            set_optional_string(&mut output, "Container", value)?;
        } else if key(&name, "Conditions") {
            set_object_array(&mut output, "Conditions", value, normalize_condition)?;
        } else if key(&name, "ApplyConditions") {
            set_object_array(&mut output, "ApplyConditions", value, normalize_condition)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_response(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Type".to_owned(), json!("Audio")),
        ("Conditions".to_owned(), json!([])),
    ]);
    for (name, value) in input {
        if let Some(canonical) = canonical(
            &name,
            &["Container", "AudioCodec", "VideoCodec", "OrgPn", "MimeType"],
        ) {
            set_optional_string(&mut output, canonical, value)?;
        } else if key(&name, "Type") {
            set_enum(&mut output, "Type", value, DLNA_PROFILE_TYPES)?;
        } else if key(&name, "Conditions") {
            set_object_array(&mut output, "Conditions", value, normalize_condition)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_subtitle(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Method".to_owned(), json!("Encode")),
        ("AllowChunkedResponse".to_owned(), json!(false)),
    ]);
    for (name, value) in input {
        if let Some(canonical) = canonical(
            &name,
            &["Format", "DidlMode", "Language", "Container", "Protocol"],
        ) {
            set_optional_string(&mut output, canonical, value)?;
        } else if key(&name, "Method") {
            set_enum(&mut output, "Method", value, SUBTITLE_METHOD_TYPES)?;
        } else if key(&name, "AllowChunkedResponse") {
            set_bool(&mut output, "AllowChunkedResponse", value)?;
        }
    }
    Ok(Value::Object(output))
}

fn normalize_condition(value: Value) -> Result<Value, ()> {
    let input = object(value)?;
    let mut output = Map::from_iter([
        ("Condition".to_owned(), json!("Equals")),
        ("Property".to_owned(), json!("AudioChannels")),
        ("IsRequired".to_owned(), json!(true)),
    ]);
    for (name, value) in input {
        if key(&name, "Condition") {
            set_enum(&mut output, "Condition", value, PROFILE_CONDITION_TYPES)?;
        } else if key(&name, "Property") {
            set_enum(&mut output, "Property", value, PROFILE_CONDITION_VALUES)?;
        } else if key(&name, "Value") {
            set_optional_string(&mut output, "Value", value)?;
        } else if key(&name, "IsRequired") {
            set_bool(&mut output, "IsRequired", value)?;
        }
    }
    Ok(Value::Object(output))
}

fn official_default_profile() -> Value {
    let mut profile = normalize_profile(json!({
        "Type": "System",
        "Name": "Generic Device",
        "AlbumArtPn": "JPEG_SM",
        "MaxAlbumArtWidth": 640,
        "MaxAlbumArtHeight": 480,
        "MaxIconWidth": 48,
        "MaxIconHeight": 48,
        "Manufacturer": "Emby",
        "ManufacturerUrl": "https://emby.media",
        "ModelName": "Windows Media Player Sharing",
        "ModelNumber": "12.0",
        "ModelUrl": "https://emby.media",
        "ProtocolInfo": "http-get:*:video/mpeg:*,http-get:*:video/mp4:*,http-get:*:video/vnd.dlna.mpeg-tts:*,http-get:*:video/avi:*,http-get:*:video/x-matroska:*,http-get:*:video/x-ms-wmv:*,http-get:*:video/wtv:*,http-get:*:audio/mpeg:*,http-get:*:audio/mp3:*,http-get:*:audio/mp4:*,http-get:*:audio/x-ms-wma*,http-get:*:audio/wav:*,http-get:*:audio/L16:*,http-get:*image/jpeg:*,http-get:*image/png:*,http-get:*image/gif:*,http-get:*image/tiff:*",
        "MaxStreamingBitrate": 140_000_000,
        "MusicStreamingTranscodingBitrate": 320_000,
        "ProtocolInfoDetection": {
            "EnabledForVideo": true,
            "EnabledForAudio": true,
            "EnabledForPhotos": true
        },
        "TranscodingProfiles": [
            {"Container": "mp3", "AudioCodec": "mp3", "Type": "Audio"},
            {"Container": "ts", "AudioCodec": "aac", "VideoCodec": "h264", "Type": "Video"},
            {"Container": "jpeg", "Type": "Photo"}
        ],
        "DirectPlayProfiles": [
            {"Container": "", "Type": "Video"},
            {"Container": "", "Type": "Audio"}
        ],
        "SubtitleProfiles": [
            {"Format": "srt", "Method": "External"},
            {"Format": "sub", "Method": "External"},
            {"Format": "srt", "Method": "Embed"},
            {"Format": "ass", "Method": "Embed"},
            {"Format": "ssa", "Method": "Embed"},
            {"Format": "smi", "Method": "Embed"},
            {"Format": "dvdsub", "Method": "Embed"},
            {"Format": "pgs", "Method": "Embed"},
            {"Format": "pgssub", "Method": "Embed"},
            {"Format": "sub", "Method": "Embed"},
            {"Format": "subrip", "Method": "Embed"},
            {"Format": "vtt", "Method": "Embed"}
        ],
        "ResponseProfiles": [
            {"Container": "m4v", "Type": "Video", "MimeType": "video/mp4"}
        ]
    }))
    .expect("the built-in profile is valid");
    profile["Type"] = json!("System");
    profile
}

type Normalizer = fn(Value) -> Result<Value, ()>;

fn object(value: Value) -> Result<Map<String, Value>, ()> {
    match value {
        Value::Object(value) => Ok(value),
        _ => Err(()),
    }
}

fn key(actual: &str, expected: &str) -> bool {
    actual.eq_ignore_ascii_case(expected)
}

fn canonical<'a>(actual: &str, candidates: &'a [&str]) -> Option<&'a str> {
    candidates
        .iter()
        .copied()
        .find(|candidate| key(actual, candidate))
}

fn set_optional_string(
    output: &mut Map<String, Value>,
    name: &str,
    value: Value,
) -> Result<(), ()> {
    if value.is_null() {
        output.remove(name);
    } else if value.is_string() {
        output.insert(name.to_owned(), value);
    } else {
        return Err(());
    }
    Ok(())
}

fn set_bool(output: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), ()> {
    let parsed = match value {
        Value::Bool(value) => value,
        Value::String(value) if value.eq_ignore_ascii_case("true") => true,
        Value::String(value) if value.eq_ignore_ascii_case("false") => false,
        _ => return Err(()),
    };
    output.insert(name.to_owned(), Value::Bool(parsed));
    Ok(())
}

fn parse_i64(value: Value) -> Result<i64, ()> {
    match value {
        Value::Number(value) => value.as_i64().ok_or(()),
        Value::String(value) => value.parse().map_err(|_| ()),
        _ => Err(()),
    }
}

fn set_i32(output: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), ()> {
    let parsed = i32::try_from(parse_i64(value)?).map_err(|_| ())?;
    output.insert(name.to_owned(), json!(parsed));
    Ok(())
}

fn set_optional_i32(output: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), ()> {
    if value.is_null() {
        output.remove(name);
    } else {
        set_i32(output, name, value)?;
    }
    Ok(())
}

fn set_optional_i64(output: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), ()> {
    if value.is_null() {
        output.remove(name);
    } else {
        output.insert(name.to_owned(), json!(parse_i64(value)?));
    }
    Ok(())
}

fn set_enum(
    output: &mut Map<String, Value>,
    name: &str,
    value: Value,
    variants: &[(&str, i64)],
) -> Result<(), ()> {
    let variant = match value {
        Value::Number(number) => number
            .as_i64()
            .and_then(|number| variants.iter().find(|(_, value)| *value == number)),
        Value::String(value) => variants
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&value))
            .or_else(|| {
                value
                    .parse::<i64>()
                    .ok()
                    .and_then(|number| variants.iter().find(|(_, value)| *value == number))
            }),
        _ => None,
    }
    .ok_or(())?;
    output.insert(name.to_owned(), Value::String(variant.0.to_owned()));
    Ok(())
}

fn set_optional_object(
    output: &mut Map<String, Value>,
    name: &str,
    value: Value,
    normalize: Normalizer,
) -> Result<(), ()> {
    match value {
        Value::Null => {
            output.remove(name);
        }
        value => {
            output.insert(name.to_owned(), normalize(value)?);
        }
    }
    Ok(())
}

fn set_string_array(output: &mut Map<String, Value>, name: &str, value: Value) -> Result<(), ()> {
    let values = match value {
        Value::Null => {
            output.remove(name);
            return Ok(());
        }
        Value::Array(values) => values,
        _ => return Err(()),
    };
    if values.iter().any(|value| !value.is_string()) {
        return Err(());
    }
    output.insert(name.to_owned(), Value::Array(values));
    Ok(())
}

fn set_object_array(
    output: &mut Map<String, Value>,
    name: &str,
    value: Value,
    normalize: Normalizer,
) -> Result<(), ()> {
    let values = match value {
        Value::Null => {
            output.remove(name);
            return Ok(());
        }
        Value::Array(values) => values,
        _ => return Err(()),
    };
    let normalized = values
        .into_iter()
        .map(normalize)
        .collect::<Result<Vec<_>, _>>()?;
    output.insert(name.to_owned(), Value::Array(normalized));
    Ok(())
}

const DEVICE_PROFILE_TYPES: &[(&str, i64)] = &[("System", 0), ("User", 1)];
const DLNA_PROFILE_TYPES: &[(&str, i64)] = &[("Audio", 0), ("Video", 1), ("Photo", 2)];
const CODEC_TYPES: &[(&str, i64)] = &[("Video", 0), ("VideoAudio", 1), ("Audio", 2)];
const SUBTITLE_METHOD_TYPES: &[(&str, i64)] = &[
    ("Encode", 0),
    ("Embed", 1),
    ("External", 2),
    ("Hls", 3),
    ("VideoSideData", 4),
];
const HEADER_MATCH_TYPES: &[(&str, i64)] = &[("Equals", 0), ("Regex", 1), ("Substring", 2)];
const TRANSCODE_SEEK_TYPES: &[(&str, i64)] = &[("Auto", 0), ("Bytes", 1)];
const ENCODING_CONTEXT_TYPES: &[(&str, i64)] = &[("Streaming", 0), ("Static", 1)];
const PROFILE_CONDITION_TYPES: &[(&str, i64)] = &[
    ("Equals", 0),
    ("NotEquals", 1),
    ("LessThanEqual", 2),
    ("GreaterThanEqual", 3),
    ("EqualsAny", 4),
];
const PROFILE_CONDITION_VALUES: &[(&str, i64)] = &[
    ("AudioChannels", 0),
    ("AudioBitrate", 1),
    ("AudioProfile", 2),
    ("Width", 3),
    ("Height", 4),
    ("Has64BitOffsets", 5),
    ("PacketLength", 6),
    ("VideoBitDepth", 7),
    ("VideoBitrate", 8),
    ("VideoFramerate", 9),
    ("VideoLevel", 10),
    ("VideoProfile", 11),
    ("VideoTimestamp", 12),
    ("IsAnamorphic", 13),
    ("RefFrames", 14),
    ("NumAudioStreams", 16),
    ("NumVideoStreams", 17),
    ("IsSecondaryAudio", 18),
    ("VideoCodecTag", 19),
    ("IsAvc", 20),
    ("IsInterlaced", 21),
    ("AudioSampleRate", 22),
    ("AudioBitDepth", 23),
    ("VideoRange", 24),
    ("VideoRotation", 25),
    ("IsExternalAudio", 26),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_matches_emby_constructor_contract() {
        let profile = official_default_profile();
        assert_eq!(profile["Name"], "Generic Device");
        assert_eq!(profile["Type"], "System");
        assert_eq!(profile["MaxStreamingBitrate"], 140_000_000_i64);
        assert_eq!(profile["DirectPlayProfiles"].as_array().unwrap().len(), 2);
        assert_eq!(profile["TranscodingProfiles"].as_array().unwrap().len(), 3);
        assert_eq!(profile["SubtitleProfiles"].as_array().unwrap().len(), 12);
        assert_eq!(profile["ResponseProfiles"].as_array().unwrap().len(), 1);
        assert_eq!(profile["ProtocolInfoDetection"]["EnabledForVideo"], true);
    }

    #[test]
    fn profile_binding_is_recursive_case_insensitive_and_last_wins() {
        let profile = normalize_profile(json!({
            "name": "first",
            "NAME": "second",
            "unknown": "ignored",
            "maxstreamingbitrate": "1234",
            "directplayprofiles": [{"container": "mkv", "TYPE": "1"}],
            "codecprofiles": [{
                "type": 2,
                "conditions": [{"property": "22", "isrequired": "false"}]
            }]
        }))
        .unwrap();
        assert_eq!(profile["Name"], "second");
        assert_eq!(profile["MaxStreamingBitrate"], 1234);
        assert_eq!(profile["DirectPlayProfiles"][0]["Type"], "Video");
        assert_eq!(profile["CodecProfiles"][0]["Type"], "Audio");
        assert_eq!(
            profile["CodecProfiles"][0]["Conditions"][0]["Property"],
            "AudioSampleRate"
        );
        assert_eq!(
            profile["CodecProfiles"][0]["Conditions"][0]["IsRequired"],
            false
        );
        assert!(profile.get("unknown").is_none());
    }

    #[test]
    fn stable_profile_id_uses_dotnet_guid_byte_order() {
        assert_eq!(
            profile_id_for_name("Living Room"),
            profile_id_for_name("Living Room")
        );
        assert_eq!(
            profile_id_for_name("Living/Room"),
            profile_id_for_name("Living_Room")
        );
        assert_eq!(profile_id_for_name("Living Room").len(), 32);
    }
}
