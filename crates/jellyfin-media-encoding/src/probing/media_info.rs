use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde_json::{Map, Value};

use super::{estimated_audio_bitrate, frame_rate, is_near_square_pixel_sar};

const TICKS_PER_MILLISECOND: i64 = 10_000;

/// Context that is not present in captured `FFprobe` JSON.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeContext<'a> {
    pub path: &'a str,
    pub is_audio: bool,
}

/// Normalized media stream category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaStreamType {
    Video,
    Audio,
    Subtitle,
    EmbeddedImage,
    Data,
}

/// Spatial-audio extension detected from an audio profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioSpatialFormat {
    DolbyAtmos,
    DtsX,
}

/// Person role read from audio metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaPersonKind {
    Composer,
    Conductor,
    Lyricist,
    Actor,
    Writer,
    Arranger,
    Engineer,
    Mixer,
    Remixer,
}

/// Boolean stream properties stored compactly as flags.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaStreamFlags(u16);

impl MediaStreamFlags {
    const ANAMORPHIC: u16 = 1 << 0;
    const AVC: u16 = 1 << 1;
    const DEFAULT: u16 = 1 << 2;
    const EXTERNAL: u16 = 1 << 3;
    const FORCED: u16 = 1 << 4;
    const HEARING_IMPAIRED: u16 = 1 << 5;
    const INTERLACED: u16 = 1 << 6;
    const TEXT_SUBTITLE: u16 = 1 << 7;
    const ORIGINAL: u16 = 1 << 8;

    const fn contains(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    fn set(&mut self, flag: u16, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

/// Audio metadata person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaPerson {
    pub name: String,
    pub kind: MediaPersonKind,
    pub role: Option<String>,
}

/// Normalized stream fields used by playback and metadata consumers.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaStream {
    pub index: i32,
    pub stream_type: MediaStreamType,
    pub codec: String,
    pub profile: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub aspect_ratio: Option<String>,
    pub average_frame_rate: Option<f32>,
    pub real_frame_rate: Option<f32>,
    pub bit_depth: Option<i32>,
    pub bit_rate: Option<i64>,
    pub codec_time_base: Option<String>,
    pub time_base: Option<String>,
    pub flags: MediaStreamFlags,
    pub level: Option<f64>,
    pub nal_length_size: Option<String>,
    pub pixel_format: Option<String>,
    pub ref_frames: Option<i32>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: Option<u32>,
    pub audio_spatial_format: Option<AudioSpatialFormat>,
    pub dv_version_major: Option<i32>,
    pub dv_version_minor: Option<i32>,
    pub dv_profile: Option<i32>,
    pub dv_level: Option<i32>,
    pub rpu_present_flag: Option<i32>,
    pub el_present_flag: Option<i32>,
    pub bl_present_flag: Option<i32>,
    pub dv_bl_signal_compatibility_id: Option<i32>,
    pub rotation: Option<i32>,
}

impl MediaStream {
    #[must_use]
    pub const fn is_anamorphic(&self) -> bool {
        self.flags.contains(MediaStreamFlags::ANAMORPHIC)
    }

    #[must_use]
    pub const fn is_avc(&self) -> bool {
        self.flags.contains(MediaStreamFlags::AVC)
    }

    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.flags.contains(MediaStreamFlags::DEFAULT)
    }

    #[must_use]
    pub const fn is_external(&self) -> bool {
        self.flags.contains(MediaStreamFlags::EXTERNAL)
    }

    #[must_use]
    pub const fn is_forced(&self) -> bool {
        self.flags.contains(MediaStreamFlags::FORCED)
    }

    #[must_use]
    pub const fn is_hearing_impaired(&self) -> bool {
        self.flags.contains(MediaStreamFlags::HEARING_IMPAIRED)
    }

    #[must_use]
    pub const fn is_interlaced(&self) -> bool {
        self.flags.contains(MediaStreamFlags::INTERLACED)
    }

    #[must_use]
    pub const fn is_text_subtitle_stream(&self) -> bool {
        self.flags.contains(MediaStreamFlags::TEXT_SUBTITLE)
    }

    #[must_use]
    pub const fn is_original(&self) -> bool {
        self.flags.contains(MediaStreamFlags::ORIGINAL)
    }
}

/// File attachment or attached cover stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaAttachment {
    pub codec: String,
    pub index: i32,
    pub codec_tag: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub comment: Option<String>,
}

/// Chapter normalized to Jellyfin ticks at millisecond precision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaChapter {
    pub name: Option<String>,
    pub start_position_ticks: i64,
}

/// Service-independent media information normalized from `FFprobe` JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    pub container: Option<String>,
    pub media_streams: Vec<MediaStream>,
    pub media_attachments: Vec<MediaAttachment>,
    pub chapters: Vec<MediaChapter>,
    pub bitrate: Option<i64>,
    pub runtime_ticks: Option<i64>,
    pub name: Option<String>,
    pub forced_sort_name: Option<String>,
    pub overview: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub production_year: Option<i32>,
    pub premiere_date: Option<DateTime<Utc>>,
    pub genres: Vec<String>,
    pub people: Vec<MediaPerson>,
}

impl MediaInfo {
    #[must_use]
    pub fn video_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|stream| stream.stream_type == MediaStreamType::Video)
    }
}

/// Failure while loading or decoding captured probe output.
#[derive(Debug)]
pub enum ProbeError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidRoot,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "probe fixture I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "invalid probe JSON: {error}"),
            Self::InvalidRoot => formatter.write_str("probe JSON root must be an object"),
        }
    }
}

impl Error for ProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidRoot => None,
        }
    }
}

/// Normalizes captured `FFprobe` JSON without launching an external process.
///
/// # Errors
///
/// Returns a JSON error for malformed input or [`ProbeError::InvalidRoot`] for
/// a non-object JSON document.
pub fn normalize_probe_json(
    input: &str,
    context: ProbeContext<'_>,
) -> Result<MediaInfo, ProbeError> {
    let value: Value = serde_json::from_str(input).map_err(ProbeError::Json)?;
    let root = value.as_object().ok_or(ProbeError::InvalidRoot)?;
    Ok(normalize_root(root, context))
}

/// Loads and normalizes a captured `FFprobe` JSON file.
///
/// # Errors
///
/// Returns file I/O errors or the parse errors documented by
/// [`normalize_probe_json`].
pub fn normalize_probe_file(
    fixture: impl AsRef<Path>,
    context: ProbeContext<'_>,
) -> Result<MediaInfo, ProbeError> {
    let input = fs::read_to_string(fixture).map_err(ProbeError::Io)?;
    normalize_probe_json(&input, context)
}

fn normalize_root(root: &Map<String, Value>, context: ProbeContext<'_>) -> MediaInfo {
    let format = root.get("format").and_then(Value::as_object);
    let stream_values = root
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut streams = stream_values
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|stream| normalize_stream(stream, format, context.is_audio))
        .collect::<Vec<_>>();
    let attachments = stream_values
        .iter()
        .filter_map(Value::as_object)
        .filter_map(normalize_attachment)
        .collect();
    let bitrate = format.and_then(|value| int64(value, "bit_rate"));
    if !context.is_audio {
        estimate_missing_bitrates(&mut streams, bitrate);
    }
    let tags = combined_tags(stream_values, format, context.is_audio);
    let premiere_date = premiere_date(&tags);
    let production_year = premiere_date
        .map(|date| chrono::Datelike::year(&date))
        .or_else(|| tag(&tags, "date").and_then(parse_year));
    MediaInfo {
        path: context.path.to_owned(),
        container: format
            .and_then(|value| string(value, "format_name"))
            .map(|value| normalize_container(&value, &streams)),
        media_streams: streams,
        media_attachments: attachments,
        chapters: normalize_chapters(root.get("chapters")),
        bitrate,
        runtime_ticks: format.and_then(format_runtime_ticks),
        name: first_tag(&tags, &["title", "title-eng"]),
        forced_sort_name: first_tag(&tags, &["sort_name", "title-sort", "titlesort"]),
        overview: first_tag(&tags, &["synopsis", "description", "desc", "comment"]),
        artists: artists(&tags),
        album: tag(&tags, "album").map(str::to_owned),
        production_year,
        premiere_date,
        genres: split_distinct(tag(&tags, "genre"), &['/', ';', ',']),
        people: if context.is_audio {
            audio_people(&tags)
        } else {
            Vec::new()
        },
    }
}

fn normalize_stream(
    stream: &Map<String, Value>,
    format: Option<&Map<String, Value>>,
    is_audio_file: bool,
) -> Option<MediaStream> {
    let codec_type = string(stream, "codec_type")?;
    let codec = string(stream, "codec_name").unwrap_or_default();
    let tags = tags(stream.get("tags"));
    let codec_tag = string(stream, "codec_tag_string").filter(|value| !value.contains("[0]"));
    let stream_type = match codec_type.as_str() {
        "audio" => MediaStreamType::Audio,
        "subtitle" if !codec.is_empty() => MediaStreamType::Subtitle,
        "video" if is_audio_file => MediaStreamType::EmbeddedImage,
        "video" if matches!(codec.as_str(), "bmp" | "gif" | "png" | "webp") => {
            MediaStreamType::EmbeddedImage
        }
        "video" if codec == "mjpeg" && codec_tag.is_none() => MediaStreamType::EmbeddedImage,
        "video" => MediaStreamType::Video,
        "data" => MediaStreamType::Data,
        _ => return None,
    };
    let profile = string(stream, "profile");
    let width = int32(stream, "width");
    let height = int32(stream, "height");
    let display_aspect = string(stream, "display_aspect_ratio");
    let pixel_format = string(stream, "pix_fmt");
    let bit_depth = int32(stream, "bits_per_sample")
        .filter(|value| *value > 0)
        .or_else(|| int32(stream, "bits_per_raw_sample").filter(|value| *value > 0))
        .or_else(|| pixel_bit_depth(pixel_format.as_deref()));
    let bit_rate = stream_bit_rate(stream, &tags, format, stream_type, is_audio_file);
    let title = stream_title(&tags, stream_type);
    let normalized_codec = normalize_subtitle_codec(&codec, stream_type);
    let flags = stream_flags(stream, stream_type, &normalized_codec);
    let mut result = MediaStream {
        index: int32(stream, "index").unwrap_or_default(),
        stream_type,
        codec: normalized_codec,
        profile: profile.clone(),
        width,
        height,
        aspect_ratio: (stream_type == MediaStreamType::Video)
            .then(|| aspect_ratio(display_aspect.as_deref(), width, height))
            .flatten(),
        average_frame_rate: string(stream, "avg_frame_rate")
            .as_deref()
            .and_then(frame_rate),
        real_frame_rate: string(stream, "r_frame_rate")
            .as_deref()
            .and_then(frame_rate),
        bit_depth,
        bit_rate,
        codec_time_base: string(stream, "codec_time_base"),
        time_base: string(stream, "time_base"),
        flags,
        level: float64(stream, "level"),
        nal_length_size: string(stream, "nal_length_size"),
        pixel_format,
        ref_frames: int32(stream, "refs").filter(|value| *value > 0),
        language: tag(&tags, "language").map(str::to_owned),
        title,
        channels: uint32(stream, "channels"),
        audio_spatial_format: spatial_format(profile.as_deref()),
        dv_version_major: None,
        dv_version_minor: None,
        dv_profile: None,
        dv_level: None,
        rpu_present_flag: None,
        el_present_flag: None,
        bl_present_flag: None,
        dv_bl_signal_compatibility_id: None,
        rotation: None,
    };
    apply_side_data(&mut result, stream.get("side_data_list"));
    Some(result)
}

fn stream_flags(
    stream: &Map<String, Value>,
    stream_type: MediaStreamType,
    codec: &str,
) -> MediaStreamFlags {
    let disposition = stream.get("disposition").and_then(Value::as_object);
    let mut flags = MediaStreamFlags::default();
    flags.set(
        MediaStreamFlags::ANAMORPHIC,
        stream_type == MediaStreamType::Video
            && anamorphic(
                string(stream, "sample_aspect_ratio").as_deref(),
                string(stream, "display_aspect_ratio").as_deref(),
                int32(stream, "width"),
                int32(stream, "height"),
            ),
    );
    flags.set(MediaStreamFlags::AVC, bool_value(stream.get("is_avc")));
    for (flag, name) in [
        (MediaStreamFlags::DEFAULT, "default"),
        (MediaStreamFlags::FORCED, "forced"),
        (MediaStreamFlags::HEARING_IMPAIRED, "hearing_impaired"),
        (MediaStreamFlags::ORIGINAL, "original"),
    ] {
        flags.set(flag, disposition_flag(disposition, name));
    }
    flags.set(
        MediaStreamFlags::INTERLACED,
        string(stream, "field_order")
            .as_deref()
            .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("progressive")),
    );
    flags.set(
        MediaStreamFlags::TEXT_SUBTITLE,
        stream_type == MediaStreamType::Subtitle
            && matches!(codec, "subrip" | "mov_text" | "ass" | "ssa"),
    );
    flags
}

fn stream_bit_rate(
    stream: &Map<String, Value>,
    tags: &HashMap<String, String>,
    format: Option<&Map<String, Value>>,
    stream_type: MediaStreamType,
    is_audio_file: bool,
) -> Option<i64> {
    let direct = int64(stream, "bit_rate").filter(|value| *value > 0);
    let tagged = matches!(stream_type, MediaStreamType::Audio | MediaStreamType::Video)
        .then(|| bitrate_from_tags(tags))
        .flatten();
    direct.or(tagged).or_else(|| {
        (is_audio_file && stream_type == MediaStreamType::Audio)
            .then(|| format.and_then(|value| int64(value, "bit_rate")))
            .flatten()
    })
}

fn stream_title(tags: &HashMap<String, String>, stream_type: MediaStreamType) -> Option<String> {
    let mut title = tag(tags, "title").map(str::to_owned);
    if title.is_none() {
        let handler = tag(tags, "handler_name");
        let ignored = match stream_type {
            MediaStreamType::Audio => "SoundHandler",
            MediaStreamType::Subtitle => "SubtitleHandler",
            _ => "VideoHandler",
        };
        if handler.is_some_and(|value| !value.eq_ignore_ascii_case(ignored)) {
            title = handler.map(str::to_owned);
        }
    }
    if stream_type == MediaStreamType::EmbeddedImage
        || title
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("cc"))
    {
        None
    } else {
        title
    }
}

fn normalize_attachment(stream: &Map<String, Value>) -> Option<MediaAttachment> {
    let disposition = stream.get("disposition").and_then(Value::as_object);
    let is_attachment = string(stream, "codec_type").as_deref() == Some("attachment")
        || disposition_flag(disposition, "attached_pic");
    if !is_attachment {
        return None;
    }
    let stream_tags = tags(stream.get("tags"));
    Some(MediaAttachment {
        codec: string(stream, "codec_name").unwrap_or_default(),
        index: int32(stream, "index").unwrap_or_default(),
        codec_tag: string(stream, "codec_tag_string").filter(|value| !value.trim().is_empty()),
        file_name: tag(&stream_tags, "filename").map(str::to_owned),
        mime_type: tag(&stream_tags, "mimetype").map(str::to_owned),
        comment: tag(&stream_tags, "comment").map(str::to_owned),
    })
}

fn apply_side_data(stream: &mut MediaStream, side_data: Option<&Value>) {
    let Some(entries) = side_data.and_then(Value::as_array) else {
        return;
    };
    for entry in entries.iter().filter_map(Value::as_object) {
        match string(entry, "side_data_type").as_deref() {
            Some(value) if value.eq_ignore_ascii_case("DOVI configuration record") => {
                stream.dv_version_major = int32(entry, "dv_version_major");
                stream.dv_version_minor = int32(entry, "dv_version_minor");
                stream.dv_profile = int32(entry, "dv_profile");
                stream.dv_level = int32(entry, "dv_level");
                stream.rpu_present_flag = int32(entry, "rpu_present_flag");
                stream.el_present_flag = int32(entry, "el_present_flag");
                stream.bl_present_flag = int32(entry, "bl_present_flag");
                stream.dv_bl_signal_compatibility_id =
                    int32(entry, "dv_bl_signal_compatibility_id");
            }
            Some(value) if value.eq_ignore_ascii_case("Display Matrix") => {
                stream.rotation = int32(entry, "rotation");
            }
            Some(value) if value.eq_ignore_ascii_case("Frame Cropping") => {
                stream.flags.set(MediaStreamFlags::ANAMORPHIC, false);
            }
            _ => {}
        }
    }
}

fn estimate_missing_bitrates(streams: &mut [MediaStream], container_bitrate: Option<i64>) {
    for stream in streams
        .iter_mut()
        .filter(|stream| stream.stream_type == MediaStreamType::Audio)
    {
        if stream.bit_rate.is_none() {
            stream.bit_rate =
                estimated_audio_bitrate(&stream.codec, stream.profile.as_deref(), stream.channels)
                    .map(i64::from);
        }
    }
    let video_indices = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| stream.stream_type == MediaStreamType::Video)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(video_index) = video_indices
        .first()
        .copied()
        .filter(|_| video_indices.len() == 1)
    else {
        return;
    };
    if streams[video_index].bit_rate.is_some() {
        return;
    }
    if streams
        .iter()
        .filter(|stream| stream.stream_type == MediaStreamType::Audio)
        .any(|stream| stream.bit_rate.is_none())
    {
        return;
    }
    let other_bitrates = streams
        .iter()
        .enumerate()
        .filter(|(index, stream)| *index != video_index && !stream.is_external())
        .map(|(_, stream)| stream.bit_rate.unwrap_or_default())
        .sum::<i64>();
    streams[video_index].bit_rate = container_bitrate
        .and_then(|bitrate| bitrate.checked_sub(other_bitrates))
        .filter(|bitrate| *bitrate > 0);
}

fn normalize_container(format: &str, streams: &[MediaStream]) -> String {
    let webm_compatible = streams.iter().all(|stream| match stream.stream_type {
        MediaStreamType::Video => matches!(stream.codec.as_str(), "av1" | "vp8" | "vp9"),
        MediaStreamType::Audio => matches!(stream.codec.as_str(), "opus" | "vorbis"),
        _ => false,
    });
    format
        .split(',')
        .filter_map(|part| match part.to_ascii_lowercase().as_str() {
            "mpegvideo" => Some("mpeg"),
            "mpegts" => Some("ts"),
            "matroska" => Some("mkv"),
            "webm" if webm_compatible => Some("webm"),
            "webm" => None,
            _ => Some(part),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn combined_tags(
    streams: &[Value],
    format: Option<&Map<String, Value>>,
    is_audio: bool,
) -> HashMap<String, String> {
    let desired = if is_audio { "audio" } else { "video" };
    let mut result = streams
        .iter()
        .filter_map(Value::as_object)
        .find(|stream| string(stream, "codec_type").as_deref() == Some(desired))
        .map(|stream| tags(stream.get("tags")))
        .unwrap_or_default();
    if let Some(format_tags) = format.map(|format| tags(format.get("tags"))) {
        result.extend(format_tags);
    }
    result
}

fn tags(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    scalar_string(value).map(|value| (key.to_ascii_lowercase(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn first_tag(tags: &HashMap<String, String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| tag(tags, name).filter(|value| !value.trim().is_empty()))
        .map(str::to_owned)
}

fn tag<'a>(tags: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    tags.get(&name.to_ascii_lowercase()).map(String::as_str)
}

fn artists(tags: &HashMap<String, String>) -> Vec<String> {
    tag(tags, "artists").map_or_else(
        || split_distinct(tag(tags, "artist"), &['/', ';', '|', '\\']),
        |value| split_distinct(Some(value), &['/', ';']),
    )
}

fn split_distinct(value: Option<&str>, delimiters: &[char]) -> Vec<String> {
    let mut result = Vec::new();
    for item in value
        .into_iter()
        .flat_map(|value| value.split(delimiters))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !result
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(item))
        {
            result.push(item.to_owned());
        }
    }
    result
}

fn premiere_date(tags: &HashMap<String, String>) -> Option<DateTime<Utc>> {
    [
        "originaldate",
        "retaildate",
        "retail date",
        "retail_date",
        "date_released",
        "date",
        "creation_time",
    ]
    .iter()
    .find_map(|name| tag(tags, name).and_then(parse_date))
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(date_time) = DateTime::parse_from_rfc3339(value) {
        return Some(date_time.with_timezone(&Utc));
    }
    let date = match value.len() {
        4 => value
            .parse()
            .ok()
            .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1)),
        8 if value.bytes().all(|byte| byte.is_ascii_digit()) => {
            NaiveDate::parse_from_str(value, "%Y%m%d").ok()
        }
        _ => NaiveDate::parse_from_str(value, "%Y-%m-%d").ok(),
    }?;
    Utc.from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()
}

fn parse_year(value: &str) -> Option<i32> {
    value.get(..4)?.parse().ok()
}

fn audio_people(tags: &HashMap<String, String>) -> Vec<MediaPerson> {
    let mut people = Vec::new();
    for (tag_name, kind) in [
        ("composer", MediaPersonKind::Composer),
        ("conductor", MediaPersonKind::Conductor),
        ("lyricist", MediaPersonKind::Lyricist),
    ] {
        add_people(&mut people, tag(tags, tag_name), kind);
    }
    if let Some(performers) = tag(tags, "performer") {
        for performer in performers.split(['/', ';']).map(str::trim) {
            if let Some((name, instrument)) = performer.rsplit_once(" (")
                && let Some(instrument) = instrument.strip_suffix(')')
            {
                people.push(MediaPerson {
                    name: name.to_owned(),
                    kind: MediaPersonKind::Actor,
                    role: Some(title_case(instrument)),
                });
            }
        }
    }
    for (tag_name, kind) in [
        ("writer", MediaPersonKind::Writer),
        ("arranger", MediaPersonKind::Arranger),
        ("engineer", MediaPersonKind::Engineer),
        ("mixer", MediaPersonKind::Mixer),
        ("remixer", MediaPersonKind::Remixer),
    ] {
        add_people(&mut people, tag(tags, tag_name), kind);
    }
    people
}

fn add_people(people: &mut Vec<MediaPerson>, value: Option<&str>, kind: MediaPersonKind) {
    for name in value
        .into_iter()
        .flat_map(|value| value.split(['/', ';']))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        people.push(MediaPerson {
            name: name.to_owned(),
            kind,
            role: None,
        });
    }
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_chapters(value: Option<&Value>) -> Vec<MediaChapter> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|chapter| {
            let chapter_tags = tags(chapter.get("tags"));
            let milliseconds = string(chapter, "start_time")
                .as_deref()
                .and_then(decimal_seconds_to_milliseconds)
                .unwrap_or_default();
            MediaChapter {
                name: tag(&chapter_tags, "title").map(str::to_owned),
                start_position_ticks: milliseconds * TICKS_PER_MILLISECOND,
            }
        })
        .collect()
}

fn format_runtime_ticks(format: &Map<String, Value>) -> Option<i64> {
    string(format, "duration")
        .as_deref()
        .and_then(decimal_seconds_to_milliseconds)
        .and_then(|milliseconds| milliseconds.checked_mul(TICKS_PER_MILLISECOND))
        .filter(|ticks| *ticks > 0)
}

fn bitrate_from_tags(tags: &HashMap<String, String>) -> Option<i64> {
    for name in ["bps-eng", "bps"] {
        if let Some(value) = tag(tags, name)
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
        {
            return Some(value);
        }
    }
    let bytes = ["number_of_bytes-eng", "number_of_bytes"]
        .iter()
        .find_map(|name| tag(tags, name).and_then(|value| value.parse::<i64>().ok()))?;
    let duration = ["duration-eng", "duration"]
        .iter()
        .find_map(|name| tag(tags, name).and_then(duration_nanoseconds))?;
    if duration < 1_000_000_000 {
        return None;
    }
    let numerator = i128::from(bytes)
        .checked_mul(8)?
        .checked_mul(1_000_000_000)?;
    i64::try_from(numerator.checked_add(duration / 2)?.checked_div(duration)?).ok()
}

fn duration_nanoseconds(value: &str) -> Option<i128> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<i128>().ok()?;
    let minutes = parts.next()?.parse::<i128>().ok()?;
    let seconds = decimal_seconds_to_nanoseconds(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    hours
        .checked_mul(3_600_000_000_000)?
        .checked_add(minutes.checked_mul(60_000_000_000)?)?
        .checked_add(seconds)
}

fn decimal_seconds_to_milliseconds(value: &str) -> Option<i64> {
    i64::try_from(parse_decimal_seconds(value, 3)?).ok()
}

fn decimal_seconds_to_nanoseconds(value: &str) -> Option<i128> {
    parse_decimal_seconds(value, 9)
}

fn parse_decimal_seconds(value: &str, precision: usize) -> Option<i128> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = whole.parse::<i128>().ok()?;
    if whole < 0 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let scale = 10_i128.checked_pow(u32::try_from(precision).ok()?)?;
    let mut digits = fraction
        .bytes()
        .take(precision)
        .fold(0_i128, |value, digit| value * 10 + i128::from(digit - b'0'));
    for _ in fraction.len().min(precision)..precision {
        digits *= 10;
    }
    if fraction
        .as_bytes()
        .get(precision)
        .is_some_and(|digit| *digit >= b'5')
    {
        digits += 1;
    }
    whole.checked_mul(scale)?.checked_add(digits)
}

fn spatial_format(profile: Option<&str>) -> Option<AudioSpatialFormat> {
    let profile = profile?.to_ascii_lowercase();
    if profile.contains("atmos") {
        Some(AudioSpatialFormat::DolbyAtmos)
    } else if profile.contains("dts:x") {
        Some(AudioSpatialFormat::DtsX)
    } else {
        None
    }
}

fn normalize_subtitle_codec(codec: &str, stream_type: MediaStreamType) -> String {
    if stream_type != MediaStreamType::Subtitle {
        return codec.to_owned();
    }
    match codec.to_ascii_lowercase().as_str() {
        "dvb_subtitle" => "DVBSUB".to_owned(),
        "dvb_teletext" => "DVBTXT".to_owned(),
        "dvd_subtitle" => "DVDSUB".to_owned(),
        "hdmv_pgs_subtitle" => "PGSSUB".to_owned(),
        _ => codec.to_owned(),
    }
}

fn aspect_ratio(display: Option<&str>, width: Option<i32>, height: Option<i32>) -> Option<String> {
    let ratio = display
        .and_then(parse_ratio)
        .or_else(|| Some((f64::from(width?), f64::from(height?))))?;
    if ratio.0 <= 0.0 || ratio.1 <= 0.0 {
        return display.map(str::to_owned);
    }
    let value = ratio.0 / ratio.1;
    for (target, variance, label) in [
        (1.777_777_778, 0.03, "16:9"),
        (1.333_333_333_3, 0.05, "4:3"),
        (1.41, 0.005, "1.41:1"),
        (1.5, 0.005, "1.5:1"),
        (1.6, 0.005, "1.6:1"),
        (1.666_666_666_67, 0.005, "5:3"),
        (1.85, 0.02, "1.85:1"),
        (2.35, 0.025, "2.35:1"),
        (2.4, 0.025, "2.40:1"),
    ] {
        if (value - target).abs() <= variance {
            return Some(label.to_owned());
        }
    }
    display.map(str::to_owned)
}

fn anamorphic(
    sample: Option<&str>,
    display: Option<&str>,
    width: Option<i32>,
    height: Option<i32>,
) -> bool {
    if sample.is_none() && display.is_none() || is_near_square_pixel_sar(sample) {
        return false;
    }
    if sample != Some("0:1") {
        return true;
    }
    if display == Some("0:1") {
        return false;
    }
    display != aspect_ratio(None, width, height).as_deref()
}

fn parse_ratio(value: &str) -> Option<(f64, f64)> {
    let (width, height) = value.split_once(':')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn pixel_bit_depth(pixel_format: Option<&str>) -> Option<i32> {
    match pixel_format?.to_ascii_lowercase().as_str() {
        "yuv420p" | "yuv444p" | "yuvj420p" => Some(8),
        "yuv420p10le" | "yuv444p10le" => Some(10),
        "yuv420p12le" | "yuv444p12le" => Some(12),
        _ => None,
    }
}

fn disposition_flag(disposition: Option<&Map<String, Value>>, name: &str) -> bool {
    disposition
        .and_then(|value| value.get(name))
        .is_some_and(|value| value.as_i64() == Some(1) || value.as_str() == Some("1"))
}

fn string(object: &Map<String, Value>, name: &str) -> Option<String> {
    object.get(name).and_then(scalar_string)
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn int32(object: &Map<String, Value>, name: &str) -> Option<i32> {
    object.get(name).and_then(|value| {
        value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .or_else(|| value.as_str()?.parse().ok())
    })
}

fn uint32(object: &Map<String, Value>, name: &str) -> Option<u32> {
    object.get(name).and_then(|value| {
        value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| value.as_str()?.parse().ok())
    })
}

fn int64(object: &Map<String, Value>, name: &str) -> Option<i64> {
    object
        .get(name)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
}

fn float64(object: &Map<String, Value>, name: &str) -> Option<f64> {
    object
        .get(name)
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
}

fn bool_value(value: Option<&Value>) -> bool {
    value.is_some_and(|value| {
        value.as_bool().unwrap_or_else(|| {
            value
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        })
    })
}
