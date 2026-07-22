use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const TICKS_PER_SECOND: i64 = 10_000_000;

/// A normalized text subtitle event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtitleEvent {
    pub id: String,
    pub text: String,
    pub start_position_ticks: i64,
    pub end_position_ticks: i64,
}

impl SubtitleEvent {
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            start_position_ticks: 0,
            end_position_ticks: 0,
        }
    }
}

/// A parsed subtitle track with events in source order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubtitleTrack {
    pub events: Vec<SubtitleEvent>,
}

/// Text subtitle formats supported by the first media-encoding phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubtitleFormat {
    Srt,
    Ssa,
    Ass,
}

impl SubtitleFormat {
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "srt" | "subrip" => Some(Self::Srt),
            "ssa" => Some(Self::Ssa),
            "ass" => Some(Self::Ass),
            _ => None,
        }
    }
}

/// Failure to load or recognize a subtitle stream.
#[derive(Debug)]
pub enum SubtitleParseError {
    Io(io::Error),
    UnsupportedFormat(String),
    NoEvents(SubtitleFormat),
}

impl fmt::Display for SubtitleParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "subtitle I/O failed: {error}"),
            Self::UnsupportedFormat(extension) => {
                write!(formatter, "unsupported subtitle extension: {extension}")
            }
            Self::NoEvents(format) => write!(formatter, "no {format:?} subtitle events found"),
        }
    }
}

impl Error for SubtitleParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::UnsupportedFormat(_) | Self::NoEvents(_) => None,
        }
    }
}

impl From<io::Error> for SubtitleParseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Parses a UTF-8 subtitle stream selected by file extension.
///
/// # Errors
///
/// Returns an I/O error when the stream cannot be read, `UnsupportedFormat`
/// for an unknown extension, or `NoEvents` when no valid events are found.
pub fn parse_subtitle(
    mut reader: impl Read,
    file_extension: &str,
) -> Result<SubtitleTrack, SubtitleParseError> {
    let mut input = String::new();
    reader.read_to_string(&mut input)?;
    parse_subtitle_str(&input, file_extension)
}

/// Parses a subtitle file selected by its explicit extension.
///
/// # Errors
///
/// Returns the same errors as [`parse_subtitle`] plus file-open errors.
pub fn parse_subtitle_file(
    path: impl AsRef<Path>,
    file_extension: &str,
) -> Result<SubtitleTrack, SubtitleParseError> {
    parse_subtitle(File::open(path)?, file_extension)
}

/// Parses subtitle text selected by file extension.
///
/// # Errors
///
/// Returns `UnsupportedFormat` for an unknown extension or `NoEvents` when no
/// valid events are found.
pub fn parse_subtitle_str(
    input: &str,
    file_extension: &str,
) -> Result<SubtitleTrack, SubtitleParseError> {
    let format = SubtitleFormat::from_extension(file_extension)
        .ok_or_else(|| SubtitleParseError::UnsupportedFormat(file_extension.to_owned()))?;
    let track = match format {
        SubtitleFormat::Srt => SrtParser::parse(input),
        SubtitleFormat::Ssa => SsaParser::parse(input),
        SubtitleFormat::Ass => AssParser::parse(input),
    };
    if track.events.is_empty() {
        Err(SubtitleParseError::NoEvents(format))
    } else {
        Ok(track)
    }
}

/// `SubRip` subtitle parser.
pub struct SrtParser;

impl SrtParser {
    #[must_use]
    pub fn parse(input: &str) -> SubtitleTrack {
        let lines = normalized_lines(input);
        let mut events = Vec::new();
        let mut cursor = 0;

        while let Some((header, number, start, end)) = find_next_srt_header(&lines, cursor) {
            cursor = header + 2;
            let text_start = cursor;
            while cursor < lines.len() && !is_srt_header_at(&lines, cursor) {
                cursor += 1;
            }
            let mut text_end = cursor;
            while text_end > text_start && lines[text_end - 1].is_empty() {
                text_end -= 1;
            }
            let text = lines[text_start..text_end].join("\n");
            events.push(SubtitleEvent {
                id: number.to_owned(),
                text,
                start_position_ticks: start,
                end_position_ticks: end,
            });
        }

        SubtitleTrack { events }
    }
}

/// `SubStation Alpha` subtitle parser.
pub struct SsaParser;

impl SsaParser {
    #[must_use]
    pub fn parse(input: &str) -> SubtitleTrack {
        parse_substation_alpha(input)
    }
}

/// Advanced `SubStation Alpha` subtitle parser.
pub struct AssParser;

impl AssParser {
    #[must_use]
    pub fn parse(input: &str) -> SubtitleTrack {
        parse_substation_alpha(input)
    }
}

fn normalized_lines(input: &str) -> Vec<&str> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    input
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn find_next_srt_header<'a>(
    lines: &'a [&'a str],
    start: usize,
) -> Option<(usize, &'a str, i64, i64)> {
    (start..lines.len()).find_map(|index| {
        let number = lines[index].trim();
        let timeline = lines.get(index + 1)?;
        if number.parse::<u64>().is_err() {
            return None;
        }
        let (start, end) = parse_srt_timeline(timeline)?;
        Some((index, number, start, end))
    })
}

fn is_srt_header_at(lines: &[&str], index: usize) -> bool {
    let Some(number) = lines.get(index) else {
        return false;
    };
    let Some(timeline) = lines.get(index + 1) else {
        return false;
    };
    number.trim().parse::<u64>().is_ok() && parse_srt_timeline(timeline).is_some()
}

fn parse_srt_timeline(line: &str) -> Option<(i64, i64)> {
    let (start, end) = line.split_once("-->")?;
    let end = end.split_ascii_whitespace().next()?;
    Some((parse_srt_time(start.trim())?, parse_srt_time(end)?))
}

fn parse_srt_time(value: &str) -> Option<i64> {
    let (hours, rest) = value.split_once(':')?;
    let (minutes, rest) = rest.split_once(':')?;
    let (seconds, milliseconds) = rest.split_once([',', '.'])?;
    if milliseconds.len() != 3 {
        return None;
    }
    time_parts_to_ticks(hours, minutes, seconds, milliseconds)
}

fn parse_substation_alpha(input: &str) -> SubtitleTrack {
    let mut in_events = false;
    let mut columns: Option<Vec<String>> = None;
    let mut events = Vec::new();

    for raw_line in normalized_lines(input) {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(format) = strip_prefix_ignore_ascii_case(line, "Format:") {
            columns = Some(
                format
                    .split(',')
                    .map(|column| column.trim().to_ascii_lowercase())
                    .collect(),
            );
            continue;
        }
        let Some(dialogue) = strip_prefix_ignore_ascii_case(line, "Dialogue:") else {
            continue;
        };
        let Some(columns) = columns.as_ref() else {
            continue;
        };
        let Some(start_index) = columns.iter().position(|column| column == "start") else {
            continue;
        };
        let Some(end_index) = columns.iter().position(|column| column == "end") else {
            continue;
        };
        let Some(text_index) = columns.iter().position(|column| column == "text") else {
            continue;
        };
        let fields: Vec<_> = dialogue.splitn(columns.len(), ',').collect();
        if fields.len() != columns.len() {
            continue;
        }
        let Some(start) = parse_ssa_time(fields[start_index].trim()) else {
            continue;
        };
        let Some(end) = parse_ssa_time(fields[end_index].trim()) else {
            continue;
        };
        events.push(SubtitleEvent {
            id: (events.len() + 1).to_string(),
            text: normalize_ssa_text(fields[text_index]),
            start_position_ticks: start,
            end_position_ticks: end,
        });
    }

    SubtitleTrack { events }
}

fn parse_ssa_time(value: &str) -> Option<i64> {
    let (hours, rest) = value.split_once(':')?;
    let (minutes, rest) = rest.split_once(':')?;
    let (seconds, fraction) = rest.split_once('.')?;
    if fraction.is_empty() || fraction.len() > 7 {
        return None;
    }
    time_parts_to_ticks(hours, minutes, seconds, fraction)
}

fn time_parts_to_ticks(hours: &str, minutes: &str, seconds: &str, fraction: &str) -> Option<i64> {
    let hours = hours.parse::<i64>().ok()?;
    let minutes = minutes.parse::<i64>().ok()?;
    let seconds = seconds.parse::<i64>().ok()?;
    if hours < 0 || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return None;
    }
    let fraction_value = fraction.parse::<i64>().ok()?;
    let fraction_scale = 10_i64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let whole_seconds = hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    whole_seconds
        .checked_mul(TICKS_PER_SECOND)?
        .checked_add(fraction_value.checked_mul(TICKS_PER_SECOND / fraction_scale)?)
}

fn normalize_ssa_text(text: &str) -> String {
    text.replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", "\u{a0}")
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}
