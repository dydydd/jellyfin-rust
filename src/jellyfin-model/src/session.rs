use crate::{DeviceProfile, PlayMethod};
use chrono::{DateTime, Utc};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum MediaType {
    #[default]
    Unknown,
    Video,
    Audio,
    Photo,
    Book,
}

impl<'de> Deserialize<'de> for MediaType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_official_enum(deserializer, &MEDIA_TYPES)
    }
}

impl std::str::FromStr for MediaType {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum GeneralCommandType {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    PageUp,
    PageDown,
    PreviousLetter,
    NextLetter,
    ToggleOsd,
    ToggleContextMenu,
    Select,
    Back,
    TakeScreenshot,
    SendKey,
    SendString,
    GoHome,
    GoToSettings,
    VolumeUp,
    VolumeDown,
    Mute,
    Unmute,
    ToggleMute,
    SetVolume,
    SetAudioStreamIndex,
    SetSubtitleStreamIndex,
    ToggleFullscreen,
    DisplayContent,
    GoToSearch,
    DisplayMessage,
    SetRepeatMode,
    ChannelUp,
    ChannelDown,
    Guide,
    ToggleStats,
    PlayMediaSource,
    PlayTrailers,
    SetShuffleQueue,
    PlayState,
    PlayNext,
    ToggleOsdMenu,
    Play,
    SetMaxStreamingBitrate,
    SetPlaybackOrder,
}

impl<'de> Deserialize<'de> for GeneralCommandType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_official_enum(deserializer, &GENERAL_COMMAND_TYPES)
    }
}

impl std::str::FromStr for GeneralCommandType {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct ClientCapabilitiesDto {
    pub playable_media_types: Vec<MediaType>,
    pub supported_commands: Vec<GeneralCommandType>,
    pub supports_media_control: bool,
    pub supports_persistent_identifier: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schemars(with = "Option<serde_json::Value>"))]
    pub device_profile: Option<DeviceProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_store_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct ClientCapabilitiesWire {
    #[serde(deserialize_with = "deserialize_media_type_collection")]
    playable_media_types: Vec<MediaType>,
    #[serde(deserialize_with = "deserialize_general_command_collection")]
    supported_commands: Vec<GeneralCommandType>,
    supports_media_control: bool,
    supports_persistent_identifier: bool,
    device_profile: Option<DeviceProfile>,
    app_store_url: Option<String>,
    icon_url: Option<String>,
}

impl Default for ClientCapabilitiesWire {
    fn default() -> Self {
        let value = ClientCapabilitiesDto::default();
        Self {
            playable_media_types: value.playable_media_types,
            supported_commands: value.supported_commands,
            supports_media_control: value.supports_media_control,
            supports_persistent_identifier: value.supports_persistent_identifier,
            device_profile: value.device_profile,
            app_store_url: value.app_store_url,
            icon_url: value.icon_url,
        }
    }
}

impl<'de> Deserialize<'de> for ClientCapabilitiesDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        normalize_client_capabilities_json(&mut value).map_err(D::Error::custom)?;
        let value: ClientCapabilitiesWire =
            serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            playable_media_types: value.playable_media_types,
            supported_commands: value.supported_commands,
            supports_media_control: value.supports_media_control,
            supports_persistent_identifier: value.supports_persistent_identifier,
            device_profile: value.device_profile,
            app_store_url: value.app_store_url,
            icon_url: value.icon_url,
        })
    }
}

impl ClientCapabilitiesDto {
    /// Deserializes capabilities stored on a device session.
    ///
    /// Jellyfin's wire DTO defaults an omitted persistent-identifier flag to
    /// `false`, while its runtime `ClientCapabilities` model defaults the flag
    /// to `true`. New sessions are persisted before a client reports
    /// capabilities, so their stored JSON is empty and must use the runtime
    /// default when projected back into device and session DTOs.
    #[must_use]
    pub fn from_stored_value(value: Value) -> Self {
        let supports_persistent_identifier = value
            .as_object()
            .and_then(|object| {
                object.iter().find_map(|(name, value)| {
                    name.eq_ignore_ascii_case("SupportsPersistentIdentifier")
                        .then_some(value)
                })
            })
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut capabilities: Self = serde_json::from_value(value).unwrap_or_default();
        capabilities.supports_persistent_identifier = supports_persistent_identifier;
        capabilities
    }
}

const MEDIA_TYPES: [MediaType; 5] = [
    MediaType::Unknown,
    MediaType::Video,
    MediaType::Audio,
    MediaType::Photo,
    MediaType::Book,
];

const GENERAL_COMMAND_TYPES: [GeneralCommandType; 43] = [
    GeneralCommandType::MoveUp,
    GeneralCommandType::MoveDown,
    GeneralCommandType::MoveLeft,
    GeneralCommandType::MoveRight,
    GeneralCommandType::PageUp,
    GeneralCommandType::PageDown,
    GeneralCommandType::PreviousLetter,
    GeneralCommandType::NextLetter,
    GeneralCommandType::ToggleOsd,
    GeneralCommandType::ToggleContextMenu,
    GeneralCommandType::Select,
    GeneralCommandType::Back,
    GeneralCommandType::TakeScreenshot,
    GeneralCommandType::SendKey,
    GeneralCommandType::SendString,
    GeneralCommandType::GoHome,
    GeneralCommandType::GoToSettings,
    GeneralCommandType::VolumeUp,
    GeneralCommandType::VolumeDown,
    GeneralCommandType::Mute,
    GeneralCommandType::Unmute,
    GeneralCommandType::ToggleMute,
    GeneralCommandType::SetVolume,
    GeneralCommandType::SetAudioStreamIndex,
    GeneralCommandType::SetSubtitleStreamIndex,
    GeneralCommandType::ToggleFullscreen,
    GeneralCommandType::DisplayContent,
    GeneralCommandType::GoToSearch,
    GeneralCommandType::DisplayMessage,
    GeneralCommandType::SetRepeatMode,
    GeneralCommandType::ChannelUp,
    GeneralCommandType::ChannelDown,
    GeneralCommandType::Guide,
    GeneralCommandType::ToggleStats,
    GeneralCommandType::PlayMediaSource,
    GeneralCommandType::PlayTrailers,
    GeneralCommandType::SetShuffleQueue,
    GeneralCommandType::PlayState,
    GeneralCommandType::PlayNext,
    GeneralCommandType::ToggleOsdMenu,
    GeneralCommandType::Play,
    GeneralCommandType::SetMaxStreamingBitrate,
    GeneralCommandType::SetPlaybackOrder,
];

fn deserialize_official_enum<'de, D, T>(deserializer: D, variants: &[T]) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Copy + Serialize,
{
    let value = Value::deserialize(deserializer)?;
    parse_official_enum(&value, variants)
        .ok_or_else(|| D::Error::custom("unknown enum name or integer value"))
}

fn parse_official_enum<T>(value: &Value, variants: &[T]) -> Option<T>
where
    T: Copy + Serialize,
{
    let numeric = match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    };
    if let Some(numeric) = numeric {
        return usize::try_from(numeric)
            .ok()
            .and_then(|index| variants.get(index))
            .copied();
    }

    let Value::String(name) = value else {
        return None;
    };
    variants.iter().copied().find(|variant| {
        serde_json::to_value(variant)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .is_some_and(|variant_name| variant_name.eq_ignore_ascii_case(name.trim()))
    })
}

fn deserialize_media_type_collection<'de, D>(deserializer: D) -> Result<Vec<MediaType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_official_enum_collection(deserializer, &MEDIA_TYPES)
}

fn deserialize_general_command_collection<'de, D>(
    deserializer: D,
) -> Result<Vec<GeneralCommandType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_official_enum_collection(deserializer, &GENERAL_COMMAND_TYPES)
}

fn deserialize_official_enum_collection<'de, D, T>(
    deserializer: D,
    variants: &[T],
) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Copy + Serialize,
{
    match Value::deserialize(deserializer)? {
        Value::String(value) => Ok(value
            .split(',')
            .filter(|value| !value.is_empty())
            .filter_map(|value| {
                parse_official_enum(&Value::String(value.trim().to_owned()), variants)
            })
            .collect()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                parse_official_enum(value, variants)
                    .ok_or_else(|| D::Error::custom("unknown enum name or integer value"))
            })
            .collect(),
        _ => Err(D::Error::custom(
            "expected an array or comma-delimited string",
        )),
    }
}

fn normalize_client_capabilities_json(value: &mut Value) -> Result<(), &'static str> {
    let Value::Object(object) = value else {
        return Err("client capabilities must be a JSON object");
    };
    let original = std::mem::take(object);
    for (name, mut child) in original {
        let canonical_name = match name.to_ascii_lowercase().as_str() {
            "playablemediatypes" => "PlayableMediaTypes",
            "supportedcommands" => "SupportedCommands",
            "supportsmediacontrol" => "SupportsMediaControl",
            "supportspersistentidentifier" => "SupportsPersistentIdentifier",
            "deviceprofile" => "DeviceProfile",
            "appstoreurl" => "AppStoreUrl",
            "iconurl" => "IconUrl",
            _ => {
                object.insert(name, child);
                continue;
            }
        };
        if canonical_name == "DeviceProfile" {
            if !matches!(child, Value::Object(_) | Value::Null) {
                return Err("device profile must be a JSON object or null");
            }
            normalize_device_profile_json(&mut child, None);
        }
        object.insert(canonical_name.to_owned(), child);
    }
    Ok(())
}

fn normalize_device_profile_json(value: &mut Value, context: Option<&str>) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_device_profile_json(value, context);
            }
        }
        Value::Object(object) => {
            let original = std::mem::take(object);
            for (name, mut child) in original {
                let name = canonical_device_profile_key(&name)
                    .map(str::to_owned)
                    .unwrap_or(name);
                normalize_device_profile_scalar(&name, context, &mut child);
                normalize_device_profile_json(&mut child, Some(&name));
                object.insert(name, child);
            }
        }
        _ => {}
    }
}

fn normalize_device_profile_scalar(name: &str, context: Option<&str>, value: &mut Value) {
    match name {
        "MaxStreamingBitrate"
        | "MaxStaticBitrate"
        | "MusicStreamingTranscodingBitrate"
        | "MaxStaticMusicBitrate"
        | "MinSegments"
        | "SegmentLength" => normalize_json_number(value),
        "Type" if context == Some("CodecProfiles") => {
            normalize_json_enum(value, &[(0, "Video"), (1, "VideoAudio"), (2, "Audio")]);
        }
        "Type" => normalize_json_enum(
            value,
            &[
                (0, "Audio"),
                (1, "Video"),
                (2, "Photo"),
                (3, "Subtitle"),
                (4, "Lyric"),
            ],
        ),
        "Protocol" => normalize_json_enum(value, &[(0, "http"), (1, "hls")]),
        "Context" => normalize_json_enum(value, &[(0, "Streaming"), (1, "Static")]),
        "TranscodeSeekInfo" => normalize_json_enum(value, &[(0, "Auto"), (1, "Bytes")]),
        "Method" => normalize_json_enum(
            value,
            &[
                (0, "Encode"),
                (1, "Embed"),
                (2, "External"),
                (3, "Hls"),
                (4, "Drop"),
            ],
        ),
        "Condition" => normalize_json_enum(
            value,
            &[
                (0, "Equals"),
                (1, "NotEquals"),
                (2, "LessThanEqual"),
                (3, "GreaterThanEqual"),
                (4, "EqualsAny"),
            ],
        ),
        "Property" => normalize_json_enum(value, PROFILE_CONDITION_VALUES),
        _ => {}
    }
}

const PROFILE_CONDITION_VALUES: &[(i64, &str)] = &[
    (0, "AudioChannels"),
    (1, "AudioBitrate"),
    (2, "AudioProfile"),
    (3, "Width"),
    (4, "Height"),
    (5, "Has64BitOffsets"),
    (6, "PacketLength"),
    (7, "VideoBitDepth"),
    (8, "VideoBitrate"),
    (9, "VideoFramerate"),
    (10, "VideoLevel"),
    (11, "VideoProfile"),
    (12, "VideoTimestamp"),
    (13, "IsAnamorphic"),
    (14, "RefFrames"),
    (16, "NumAudioStreams"),
    (17, "NumVideoStreams"),
    (18, "IsSecondaryAudio"),
    (19, "VideoCodecTag"),
    (20, "IsAvc"),
    (21, "IsInterlaced"),
    (22, "AudioSampleRate"),
    (23, "AudioBitDepth"),
    (24, "VideoRangeType"),
    (25, "NumStreams"),
    (26, "VideoRotation"),
];

fn normalize_json_number(value: &mut Value) {
    if let Value::String(text) = value
        && let Ok(number) = text.parse::<i64>()
    {
        *value = Value::Number(number.into());
    }
}

fn normalize_json_enum(value: &mut Value, variants: &[(i64, &str)]) {
    let number = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse::<i64>().ok(),
        _ => None,
    };
    let variant = number
        .and_then(|number| {
            variants
                .iter()
                .find(|(value, _)| *value == number)
                .map(|(_, name)| *name)
        })
        .or_else(|| {
            value.as_str().and_then(|text| {
                variants
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(text))
                    .map(|(_, name)| *name)
            })
        });
    if let Some(variant) = variant {
        *value = Value::String(variant.to_owned());
    }
}

fn canonical_device_profile_key(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "name" => "Name",
        "id" => "Id",
        "maxstreamingbitrate" => "MaxStreamingBitrate",
        "maxstaticbitrate" => "MaxStaticBitrate",
        "musicstreamingtranscodingbitrate" => "MusicStreamingTranscodingBitrate",
        "maxstaticmusicbitrate" => "MaxStaticMusicBitrate",
        "directplayprofiles" => "DirectPlayProfiles",
        "transcodingprofiles" => "TranscodingProfiles",
        "containerprofiles" => "ContainerProfiles",
        "codecprofiles" => "CodecProfiles",
        "subtitleprofiles" => "SubtitleProfiles",
        "container" => "Container",
        "audiocodec" => "AudioCodec",
        "videocodec" => "VideoCodec",
        "type" => "Type",
        "protocol" => "Protocol",
        "estimatecontentlength" => "EstimateContentLength",
        "enablempegtsm2tsmode" => "EnableMpegtsM2TsMode",
        "transcodeseekinfo" => "TranscodeSeekInfo",
        "copytimestamps" => "CopyTimestamps",
        "context" => "Context",
        "enablesubtitlesinmanifest" => "EnableSubtitlesInManifest",
        "maxaudiochannels" => "MaxAudioChannels",
        "minsegments" => "MinSegments",
        "segmentlength" => "SegmentLength",
        "conditions" => "Conditions",
        "enableaudiovbrencoding" => "EnableAudioVbrEncoding",
        "applyconditions" => "ApplyConditions",
        "codec" => "Codec",
        "subcontainer" => "SubContainer",
        "format" => "Format",
        "method" => "Method",
        "language" => "Language",
        "condition" => "Condition",
        "property" => "Property",
        "value" => "Value",
        "isrequired" => "IsRequired",
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct GeneralCommand {
    pub name: GeneralCommandType,
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub controlling_user_id: Uuid,
    pub arguments: HashMap<String, String>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct GeneralCommandWire {
    name: GeneralCommandType,
    #[serde(default, deserialize_with = "crate::serde_guid::single::deserialize")]
    controlling_user_id: Uuid,
    arguments: HashMap<String, String>,
}

impl Default for GeneralCommandWire {
    fn default() -> Self {
        Self {
            name: GeneralCommandType::MoveUp,
            controlling_user_id: Uuid::nil(),
            arguments: HashMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for GeneralCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        normalize_command_body_json(
            &mut value,
            &[
                ("name", "Name"),
                ("controllinguserid", "ControllingUserId"),
                ("arguments", "Arguments"),
            ],
        )
        .map_err(D::Error::custom)?;
        let value: GeneralCommandWire = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            name: value.name,
            controlling_user_id: value.controlling_user_id,
            arguments: value.arguments,
        })
    }
}

impl Default for GeneralCommand {
    fn default() -> Self {
        Self {
            name: GeneralCommandType::MoveUp,
            controlling_user_id: Uuid::nil(),
            arguments: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum PlayCommand {
    PlayNow,
    PlayNext,
    PlayLast,
    PlayInstantMix,
    PlayShuffle,
}

const PLAY_COMMANDS: [PlayCommand; 5] = [
    PlayCommand::PlayNow,
    PlayCommand::PlayNext,
    PlayCommand::PlayLast,
    PlayCommand::PlayInstantMix,
    PlayCommand::PlayShuffle,
];

impl<'de> Deserialize<'de> for PlayCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_official_enum(deserializer, &PLAY_COMMANDS)
    }
}

impl std::str::FromStr for PlayCommand {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PlayRequest {
    #[serde(
        serialize_with = "crate::serde_guid::vec::serialize",
        deserialize_with = "crate::serde_guid::vec::deserialize"
    )]
    pub item_ids: Vec<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_position_ticks: Option<i64>,
    pub play_command: PlayCommand,
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub controlling_user_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_index: Option<i32>,
}

impl Default for PlayRequest {
    fn default() -> Self {
        Self {
            item_ids: Vec::new(),
            start_position_ticks: None,
            play_command: PlayCommand::PlayNow,
            controlling_user_id: Uuid::nil(),
            subtitle_stream_index: None,
            audio_stream_index: None,
            media_source_id: None,
            start_index: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum PlaystateCommand {
    #[default]
    Stop,
    Pause,
    Unpause,
    NextTrack,
    PreviousTrack,
    Seek,
    Rewind,
    FastForward,
    PlayPause,
}

const PLAYSTATE_COMMANDS: [PlaystateCommand; 9] = [
    PlaystateCommand::Stop,
    PlaystateCommand::Pause,
    PlaystateCommand::Unpause,
    PlaystateCommand::NextTrack,
    PlaystateCommand::PreviousTrack,
    PlaystateCommand::Seek,
    PlaystateCommand::Rewind,
    PlaystateCommand::FastForward,
    PlaystateCommand::PlayPause,
];

impl<'de> Deserialize<'de> for PlaystateCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_official_enum(deserializer, &PLAYSTATE_COMMANDS)
    }
}

impl std::str::FromStr for PlaystateCommand {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        deserialize_enum_name(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaystateRequest {
    pub command: PlaystateCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seek_position_ticks: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controlling_user_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum RepeatMode {
    #[default]
    RepeatNone,
    RepeatAll,
    RepeatOne,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum PlaybackOrder {
    #[default]
    Default,
    Shuffle,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct PlayerStateInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ticks: Option<i64>,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_method: Option<PlayMethod>,
    pub repeat_mode: RepeatMode,
    pub playback_order: PlaybackOrder,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_stream_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct SessionUserInfo {
    #[serde(
        default,
        serialize_with = "crate::serde_guid::single::serialize",
        deserialize_with = "crate::serde_guid::single::deserialize"
    )]
    pub user_id: Uuid,
    pub user_name: String,
}

impl Default for SessionUserInfo {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            user_name: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(rename_all = "PascalCase")]
pub struct MessageCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct MessageCommandWire {
    header: Option<String>,
    text: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_i64_from_number_or_string")]
    timeout_ms: Option<i64>,
}

impl<'de> Deserialize<'de> for MessageCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        normalize_command_body_json(
            &mut value,
            &[
                ("header", "Header"),
                ("text", "Text"),
                ("timeoutms", "TimeoutMs"),
            ],
        )
        .map_err(D::Error::custom)?;
        let value: MessageCommandWire = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            header: value.header,
            text: value.text,
            timeout_ms: value.timeout_ms,
        })
    }
}

fn normalize_command_body_json(
    value: &mut Value,
    properties: &[(&str, &str)],
) -> Result<(), &'static str> {
    let Value::Object(object) = value else {
        return Err("command body must be a JSON object");
    };
    let original = std::mem::take(object);
    for (name, value) in original {
        let canonical_name = properties
            .iter()
            .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
            .map(|(_, canonical)| *canonical);
        object.insert(canonical_name.unwrap_or(name.as_str()).to_owned(), value);
    }
    Ok(())
}

fn deserialize_optional_i64_from_number_or_string<'de, D>(
    deserializer: D,
) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Value::Number(value)) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| D::Error::custom("integer is outside Int64 range")),
        Some(Value::String(value)) => value.parse().map(Some).map_err(D::Error::custom),
        Some(_) => Err(D::Error::custom("expected an integer or numeric string")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct SessionInfoDto {
    pub play_state: PlayerStateInfo,
    pub additional_users: Vec<SessionUserInfo>,
    pub capabilities: ClientCapabilitiesDto,
    pub playable_media_types: Vec<MediaType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(with = "crate::serde_guid::single")]
    pub user_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_activity_date: DateTime<Utc>,
    #[serde(with = "crate::serde_datetime::required")]
    pub last_playback_check_in: DateTime<Utc>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_datetime::option"
    )]
    pub last_paused_date: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now_playing_item: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_version: Option<String>,
    pub is_active: bool,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub now_playing_queue: Vec<Value>,
    pub has_custom_device_name: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlist_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_primary_image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now_viewing_item: Option<Value>,
    pub supported_commands: Vec<GeneralCommandType>,
}

fn deserialize_enum_name<T>(value: &str) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::String(value.to_owned()))
}
