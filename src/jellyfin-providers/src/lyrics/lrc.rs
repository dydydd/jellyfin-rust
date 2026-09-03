use std::path::Path;

const TICKS_PER_MILLISECOND: i64 = 10_000;

/// Raw lyric file presented to a lyric parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricFile {
    pub name: String,
    pub content: String,
}

impl LyricFile {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
        }
    }
}

/// Parsed timed lyrics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricDto {
    pub lyrics: Vec<LyricLine>,
}

/// One timed lyric line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricLine {
    pub text: String,
    pub start: Option<i64>,
    pub cues: Vec<LyricLineCue>,
}

/// Timing and UTF-16 positions for a portion of a lyric line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricLineCue {
    pub position: usize,
    pub end_position: usize,
    pub start: i64,
    pub end: Option<i64>,
}

/// Parser for standard and enhanced LRC lyric files.
#[derive(Clone, Copy, Debug, Default)]
pub struct LrcLyricParser;

impl LrcLyricParser {
    pub const NAME: &'static str = "LrcLyricProvider";

    #[must_use]
    pub fn parse_lyrics(&self, file: &LyricFile) -> Option<LyricDto> {
        if !has_supported_extension(&file.name) {
            return None;
        }

        let content = file
            .content
            .strip_prefix('\u{feff}')
            .unwrap_or(&file.content);
        let offset = parse_offset(content);
        let mut parsed_lines = Vec::new();

        for raw_line in content
            .split(['\r', '\n'])
            .filter(|line| !line.trim().is_empty())
        {
            let mut line_tags = find_time_tags(raw_line, b'[', b']');
            if line_tags.is_empty() {
                continue;
            }

            let lyric = remove_matches(raw_line, &line_tags).trim().to_owned();
            if line_tags.len() == 1 {
                let start = apply_offset(line_tags[0].milliseconds, offset);
                let (text, time_tags) = parse_enhanced_text(&lyric, start, offset);
                parsed_lines.push(ParsedLine {
                    text,
                    start,
                    time_tags,
                });
            } else {
                let last_tag = line_tags.pop().expect("multiple tags checked above");
                for tag in line_tags {
                    parsed_lines.push(ParsedLine {
                        // ALLOW: each timestamp expands into an independently owned DTO line.
                        text: lyric.clone(),
                        start: apply_offset(tag.milliseconds, offset),
                        time_tags: Vec::new(),
                    });
                }
                parsed_lines.push(ParsedLine {
                    text: lyric,
                    start: apply_offset(last_tag.milliseconds, offset),
                    time_tags: Vec::new(),
                });
            }
        }

        if parsed_lines.is_empty() {
            return None;
        }

        parsed_lines.sort_by_key(|line| line.start);
        let mut lyrics = Vec::with_capacity(parsed_lines.len());
        let mut parsed_lines = parsed_lines.into_iter().peekable();

        while let Some(line) = parsed_lines.next() {
            let next_line_start = parsed_lines.peek().map(|next| to_ticks(next.start));
            let cues = build_cues(&line, next_line_start);
            lyrics.push(LyricLine {
                text: line.text,
                start: Some(to_ticks(line.start)),
                cues,
            });
        }

        Some(LyricDto { lyrics })
    }
}

#[derive(Clone, Debug)]
struct ParsedLine {
    text: String,
    start: i64,
    time_tags: Vec<IndexedTimeTag>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimeTagMatch {
    start: usize,
    end: usize,
    milliseconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum IndexState {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IndexedTimeTag {
    index: isize,
    state: IndexState,
    milliseconds: i64,
}

impl IndexedTimeTag {
    fn position(self) -> usize {
        let position = match self.state {
            IndexState::Start => self.index,
            IndexState::End => self.index.saturating_add(1),
        };
        usize::try_from(position).unwrap_or(0)
    }
}

fn has_supported_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("lrc") || extension.eq_ignore_ascii_case("elrc")
        })
}

fn parse_offset(content: &str) -> i64 {
    let mut offset = 0;
    for line in content.split(['\r', '\n']) {
        let line = line.trim().strip_prefix('\u{feff}').unwrap_or(line.trim());
        let Some(metadata) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        else {
            continue;
        };
        let Some((name, value)) = metadata.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("offset") {
            continue;
        }

        if let Ok(parsed) = value.trim().parse::<i64>() {
            offset = parsed;
        }
    }
    offset
}

fn find_time_tags(text: &str, opening: u8, closing: u8) -> Vec<TimeTagMatch> {
    let bytes = text.as_bytes();
    let mut matches = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        let Some(relative_start) = bytes[cursor..].iter().position(|byte| *byte == opening) else {
            break;
        };
        let start = cursor + relative_start;
        let Some(relative_end) = bytes[start + 1..].iter().position(|byte| *byte == closing) else {
            break;
        };
        let end = start + relative_end + 2;

        if let Some(milliseconds) = parse_timestamp(&text[start + 1..end - 1]) {
            matches.push(TimeTagMatch {
                start,
                end,
                milliseconds,
            });
            cursor = end;
        } else {
            cursor = start + 1;
        }
    }

    matches
}

fn parse_timestamp(timestamp: &str) -> Option<i64> {
    let (minutes, remainder) = timestamp.split_once(':')?;
    let (seconds, decimal) = remainder.split_once('.')?;
    if minutes.is_empty()
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
        || seconds.len() != 2
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !(2..=3).contains(&decimal.len())
        || !decimal.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let minutes = minutes.parse::<i64>().ok()?;
    let seconds = seconds.parse::<i64>().ok()?;
    let decimal = decimal.parse::<i64>().ok()?;
    let milliseconds = if timestamp.rsplit_once('.')?.1.len() == 2 {
        decimal.checked_mul(10)?
    } else {
        decimal
    };

    minutes
        .checked_mul(60)?
        .checked_add(seconds)?
        .checked_mul(1_000)?
        .checked_add(milliseconds)
}

fn remove_matches(text: &str, matches: &[TimeTagMatch]) -> String {
    let removed_bytes = matches.iter().map(|tag| tag.end - tag.start).sum::<usize>();
    let mut result = String::with_capacity(text.len().saturating_sub(removed_bytes));
    let mut cursor = 0;
    for tag in matches {
        result.push_str(&text[cursor..tag.start]);
        cursor = tag.end;
    }
    result.push_str(&text[cursor..]);
    result
}

fn parse_enhanced_text(
    timed_text: &str,
    line_start: i64,
    offset: i64,
) -> (String, Vec<IndexedTimeTag>) {
    if timed_text.trim().is_empty() {
        return (String::new(), Vec::new());
    }

    let matches = find_time_tags(timed_text, b'<', b'>');
    if matches.is_empty() {
        return (timed_text.to_owned(), Vec::new());
    }

    let mut text = String::new();
    let mut tags = Vec::new();
    let mut last_time = line_start;
    let mut segment_start = 0;
    let mut insert_space = false;
    let mut last_tag_was_start = false;

    for tag in matches {
        let segment = &timed_text[segment_start..tag.start];
        segment_start = tag.end;

        if segment.trim().is_empty() {
            if last_tag_was_start {
                try_add_tag(
                    &mut tags,
                    utf16_index(&text) - 1,
                    IndexState::End,
                    last_time,
                );
                last_tag_was_start = false;
            }
            last_time = apply_offset(tag.milliseconds, offset);
            if !segment.is_empty() {
                insert_space = true;
            }
            continue;
        }

        if (starts_with_whitespace(segment) || insert_space) && !text.is_empty() {
            text.push(' ');
        }
        try_add_tag(&mut tags, utf16_index(&text), IndexState::Start, last_time);
        last_tag_was_start = true;
        text.push_str(segment.trim());
        last_time = apply_offset(tag.milliseconds, offset);
        insert_space = ends_with_whitespace(segment);
    }

    let remaining = &timed_text[segment_start..];
    if remaining.trim().is_empty() {
        try_add_tag(
            &mut tags,
            utf16_index(&text) - 1,
            IndexState::End,
            last_time,
        );
    } else {
        if (starts_with_whitespace(remaining) || insert_space) && !text.is_empty() {
            text.push(' ');
        }
        try_add_tag(&mut tags, utf16_index(&text), IndexState::Start, last_time);
        text.push_str(remaining.trim());
    }

    tags.sort_by_key(|tag| (tag.index, tag.state));
    (text, tags)
}

fn try_add_tag(tags: &mut Vec<IndexedTimeTag>, index: isize, state: IndexState, milliseconds: i64) {
    if tags
        .iter()
        .any(|tag| tag.index == index && tag.state == state)
    {
        return;
    }
    tags.push(IndexedTimeTag {
        index,
        state,
        milliseconds,
    });
}

fn build_cues(line: &ParsedLine, next_line_start: Option<i64>) -> Vec<LyricLineCue> {
    let mut cues = Vec::new();
    for pair in line.time_tags.windows(2) {
        let current = pair[0];
        let next = pair[1];
        let position = current.position();
        let end_position = next.position();
        if utf16_slice(&line.text, position, end_position)
            .is_none_or(|slice| slice.trim().is_empty())
        {
            continue;
        }
        cues.push(LyricLineCue {
            position,
            end_position,
            start: to_ticks(current.milliseconds),
            end: Some(to_ticks(next.milliseconds)),
        });
    }

    if let Some(last) = line.time_tags.last().copied() {
        let position = last.position();
        let end_position = utf16_len(&line.text);
        if utf16_slice(&line.text, position, end_position)
            .is_some_and(|slice| !slice.trim().is_empty())
        {
            cues.push(LyricLineCue {
                position,
                end_position,
                start: to_ticks(last.milliseconds),
                end: next_line_start,
            });
        }
    }
    cues
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utf16_index(text: &str) -> isize {
    isize::try_from(utf16_len(text)).unwrap_or(isize::MAX)
}

fn utf16_slice(text: &str, start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }
    let start = byte_index_at_utf16(text, start)?;
    let end = byte_index_at_utf16(text, end)?;
    text.get(start..end)
}

fn byte_index_at_utf16(text: &str, target: usize) -> Option<usize> {
    let mut utf16_index = 0;
    for (byte_index, character) in text.char_indices() {
        if utf16_index == target {
            return Some(byte_index);
        }
        utf16_index += character.len_utf16();
        if utf16_index > target {
            return None;
        }
    }
    (utf16_index == target).then_some(text.len())
}

fn starts_with_whitespace(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_whitespace)
}

fn ends_with_whitespace(text: &str) -> bool {
    text.chars().next_back().is_some_and(char::is_whitespace)
}

fn apply_offset(milliseconds: i64, offset: i64) -> i64 {
    milliseconds.saturating_add(offset).max(0)
}

fn to_ticks(milliseconds: i64) -> i64 {
    milliseconds.saturating_mul(TICKS_PER_MILLISECOND)
}
