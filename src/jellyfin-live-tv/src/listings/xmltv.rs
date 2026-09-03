use std::error::Error;
use std::fmt;
use std::fmt::Write;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use md5::{Digest, Md5};
use roxmltree::{Document, Node};

use super::{ProgramFlag, ProgramFlags, ProgramInfo, create_xmltv_program_etag};

/// Category mappings and language preference used by the XMLTV provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlTvOptions {
    pub preferred_language: Option<String>,
    pub kids_categories: Vec<String>,
    pub movie_categories: Vec<String>,
    pub news_categories: Vec<String>,
    pub sports_categories: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct XmlTvChannel {
    pub id: String,
    pub display_name: String,
    pub number: String,
    pub image_url: Option<String>,
}

pub fn parse_xmltv_channels(xml: &str) -> Result<Vec<XmlTvChannel>, XmlTvParseError> {
    let document = Document::parse(xml)?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("channel"))
        .filter_map(|node| {
            let id = node.attribute("id")?.to_owned();
            let names: Vec<_> = node
                .children()
                .filter(|child| child.has_tag_name("display-name"))
                .filter_map(|child| child.text())
                .collect();
            let display_name = names.first().copied().unwrap_or(&id).to_owned();
            let number = names
                .iter()
                .find(|name| {
                    name.chars()
                        .all(|character| character.is_ascii_digit() || character == '.')
                })
                .copied()
                .unwrap_or(&id)
                .to_owned();
            let image_url = node
                .children()
                .find(|child| child.has_tag_name("icon"))
                .and_then(|icon| icon.attribute("src"))
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            Some(XmlTvChannel {
                id,
                display_name,
                number,
                image_url,
            })
        })
        .collect())
}

impl Default for XmlTvOptions {
    fn default() -> Self {
        Self {
            preferred_language: None,
            kids_categories: strings(&["kids", "family", "children", "childrens", "disney"]),
            movie_categories: strings(&["movie"]),
            news_categories: strings(&["news", "journalism", "documentary", "current affairs"]),
            sports_categories: strings(&["sports", "basketball", "baseball", "football"]),
        }
    }
}

/// Failure while parsing an XMLTV document or programme timestamp.
#[derive(Debug)]
pub enum XmlTvParseError {
    Xml(roxmltree::Error),
    EmptyChannelId,
    InvalidRange,
    MissingAttribute(&'static str),
    InvalidDate {
        attribute: &'static str,
        value: String,
    },
}

impl fmt::Display for XmlTvParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(formatter, "invalid XMLTV document: {error}"),
            Self::EmptyChannelId => formatter.write_str("channel id is empty"),
            Self::InvalidRange => formatter.write_str("end date must be after start date"),
            Self::MissingAttribute(attribute) => {
                write!(formatter, "programme is missing {attribute} attribute")
            }
            Self::InvalidDate { attribute, value } => {
                write!(formatter, "invalid programme {attribute} date: {value}")
            }
        }
    }
}

impl Error for XmlTvParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            _ => None,
        }
    }
}

impl From<roxmltree::Error> for XmlTvParseError {
    fn from(error: roxmltree::Error) -> Self {
        Self::Xml(error)
    }
}

/// Parses programmes for one channel that overlap the requested UTC window.
///
/// # Errors
///
/// Returns [`XmlTvParseError`] for malformed XML, an invalid request window,
/// or malformed required programme attributes.
pub fn parse_xmltv_programs(
    xml: &str,
    channel_id: &str,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    options: &XmlTvOptions,
) -> Result<Vec<ProgramInfo>, XmlTvParseError> {
    if channel_id.trim().is_empty() {
        return Err(XmlTvParseError::EmptyChannelId);
    }
    if end_date <= start_date {
        return Err(XmlTvParseError::InvalidRange);
    }

    let document = Document::parse(xml)?;
    let mut programs = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("programme"))
    {
        if !node
            .attribute("channel")
            .is_some_and(|actual| actual.eq_ignore_ascii_case(channel_id))
        {
            continue;
        }
        let start = parse_program_date(node, "start")?;
        let end = parse_program_date(node, "stop")?;
        if start.utc >= end_date || end.utc < start_date {
            continue;
        }
        programs.push(map_program(node, start, end, options));
    }
    Ok(programs)
}

#[derive(Clone, Copy)]
struct ParsedDate {
    utc: DateTime<Utc>,
    offset_seconds: i32,
}

fn parse_program_date(
    node: Node<'_, '_>,
    attribute: &'static str,
) -> Result<ParsedDate, XmlTvParseError> {
    let value = node
        .attribute(attribute)
        .ok_or(XmlTvParseError::MissingAttribute(attribute))?;
    for format in ["%Y%m%d%H%M%S %z", "%Y%m%d%H%M %z"] {
        if let Ok(value) = DateTime::parse_from_str(value, format) {
            return Ok(ParsedDate {
                utc: value.with_timezone(&Utc),
                offset_seconds: value.offset().local_minus_utc(),
            });
        }
    }
    for format in ["%Y%m%d%H%M%S", "%Y%m%d%H%M"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(ParsedDate {
                utc: Utc.from_utc_datetime(&value),
                offset_seconds: 0,
            });
        }
    }
    Err(XmlTvParseError::InvalidDate {
        attribute,
        value: value.to_owned(),
    })
}

fn map_program(
    node: Node<'_, '_>,
    start: ParsedDate,
    end: ParsedDate,
    options: &XmlTvOptions,
) -> ProgramInfo {
    let channel_id = node.attribute("channel").unwrap_or_default().to_owned();
    let name = localized_child_text(node, "title", options.preferred_language.as_deref());
    let episode_title =
        localized_child_text(node, "sub-title", options.preferred_language.as_deref());
    let overview = localized_child_text(node, "desc", options.preferred_language.as_deref());
    let genres = localized_child_texts(node, "category", options.preferred_language.as_deref());
    let (season_number, episode_number) = parse_xmltv_ns_episode(node);
    let program_id = child_text_with_attribute(node, "episode-num", "system", "dd_progid");
    let image_url = node
        .children()
        .find(|child| child.has_tag_name("icon"))
        .and_then(|icon| icon.attribute("src"))
        .filter(|source| !source.is_empty())
        .map(str::to_owned);
    let official_rating = node
        .children()
        .find(|child| child.has_tag_name("rating"))
        .and_then(|rating| child_text(rating, "value"));
    let is_repeat = node
        .children()
        .any(|child| child.has_tag_name("previously-shown"))
        && !node.children().any(|child| child.has_tag_name("new"));
    let is_movie = category_matches(&genres, &options.movie_categories);
    let mut flags = ProgramFlags::default();
    flags.set(ProgramFlag::Repeat, is_repeat);
    flags.set(ProgramFlag::Movie, is_movie);
    flags.set(
        ProgramFlag::Sports,
        category_matches(&genres, &options.sports_categories),
    );
    flags.set(
        ProgramFlag::News,
        category_matches(&genres, &options.news_categories),
    );
    flags.set(
        ProgramFlag::Kids,
        category_matches(&genres, &options.kids_categories),
    );
    flags.set(ProgramFlag::Series, episode_number.is_some());
    flags.set(
        ProgramFlag::Live,
        node.children().any(|child| child.has_tag_name("live")),
    );
    flags.set(
        ProgramFlag::Premiere,
        node.children().any(|child| child.has_tag_name("premiere")),
    );
    let mut program = ProgramInfo {
        id: Some(format_program_id(&channel_id, start)),
        channel_id: Some(channel_id),
        name,
        official_rating,
        overview,
        start_date: Some(start.utc),
        end_date: Some(end.utc),
        genres,
        flags,
        episode_title,
        has_image: Some(image_url.is_some()),
        production_year: child_text(node, "date").and_then(|date| parse_year(&date)),
        series_id: episode_number
            .and_then(|_| name_for_hash(node, options).map(|title| jellyfin_md5(&title))),
        show_id: program_id,
        season_number,
        episode_number,
        ..ProgramInfo::default()
    };
    program.image_url = image_url;
    if program
        .show_id
        .as_ref()
        .is_none_or(|id| id.trim().is_empty())
    {
        program.show_id = Some(fallback_show_id(&program));
    }
    if program.flags.contains(ProgramFlag::Movie) {
        program.flags.remove(ProgramFlag::Series);
        program.episode_number = None;
        program.episode_title = None;
    }
    program.etag = create_xmltv_program_etag(&program).ok();
    program
}

fn localized_child_text(
    node: Node<'_, '_>,
    tag: &str,
    preferred_language: Option<&str>,
) -> Option<String> {
    let matches: Vec<_> = node
        .children()
        .filter(|child| child.has_tag_name(tag))
        .collect();
    preferred_language
        .and_then(|language| {
            matches
                .iter()
                .find(|child| {
                    child
                        .attribute("lang")
                        .is_some_and(|value| value.eq_ignore_ascii_case(language))
                })
                .and_then(|child| normalized_text(*child))
        })
        .or_else(|| {
            matches
                .iter()
                .find(|child| child.attribute("lang").is_none_or(str::is_empty))
                .and_then(|child| normalized_text(*child))
        })
        .or_else(|| matches.iter().find_map(|child| normalized_text(*child)))
}

fn localized_child_texts(
    node: Node<'_, '_>,
    tag: &str,
    preferred_language: Option<&str>,
) -> Vec<String> {
    let children: Vec<_> = node
        .children()
        .filter(|child| child.has_tag_name(tag))
        .collect();
    let has_preferred = preferred_language.is_some_and(|language| {
        children
            .iter()
            .any(|child| child.attribute("lang") == Some(language))
    });
    children
        .into_iter()
        .filter(|child| !has_preferred || child.attribute("lang") == preferred_language)
        .filter_map(normalized_text)
        .collect()
}

fn name_for_hash(node: Node<'_, '_>, options: &XmlTvOptions) -> Option<String> {
    localized_child_text(node, "title", options.preferred_language.as_deref())
}

fn child_text(node: Node<'_, '_>, tag: &str) -> Option<String> {
    node.children()
        .find(|child| child.has_tag_name(tag))
        .and_then(normalized_text)
}

fn child_text_with_attribute(
    node: Node<'_, '_>,
    tag: &str,
    attribute: &str,
    value: &str,
) -> Option<String> {
    node.children()
        .find(|child| {
            child.has_tag_name(tag)
                && child
                    .attribute(attribute)
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(value))
        })
        .and_then(normalized_text)
}

fn normalized_text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn parse_xmltv_ns_episode(node: Node<'_, '_>) -> (Option<i32>, Option<i32>) {
    let Some(value) = child_text_with_attribute(node, "episode-num", "system", "xmltv_ns") else {
        return (None, None);
    };
    let mut parts = value.split('.').map(str::trim);
    let season = parts.next().and_then(parse_xmltv_index);
    let episode = parts.next().and_then(parse_xmltv_index);
    (season, episode)
}

fn parse_xmltv_index(value: &str) -> Option<i32> {
    value
        .split('/')
        .next()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .and_then(|value| value.checked_add(1))
}

fn category_matches(genres: &[String], categories: &[String]) -> bool {
    genres.iter().any(|genre| {
        categories
            .iter()
            .any(|category| genre.eq_ignore_ascii_case(category))
    })
}

fn parse_year(value: &str) -> Option<i32> {
    value.get(..4)?.parse().ok()
}

fn format_program_id(channel_id: &str, date: ParsedDate) -> String {
    let local = date.utc + chrono::Duration::seconds(i64::from(date.offset_seconds));
    let sign = if date.offset_seconds < 0 { '-' } else { '+' };
    let offset = date.offset_seconds.unsigned_abs();
    let hours = offset / 3600;
    let minutes = (offset % 3600) / 60;
    format!(
        "{channel_id}_{}.0000000{sign}{hours:02}:{minutes:02}",
        local.format("%Y-%m-%dT%H:%M:%S")
    )
}

fn fallback_show_id(program: &ProgramInfo) -> String {
    let mut unique = format!(
        "{}{}",
        program.name.as_deref().unwrap_or_default(),
        program.episode_title.as_deref().unwrap_or_default()
    );
    if let Some(season) = program.season_number {
        unique = format!("-{season}");
    }
    if let Some(episode) = program.episode_number {
        unique = format!("-{episode}");
    }
    let mut show_id = jellyfin_md5(&unique);
    if program.flags.contains(ProgramFlag::Series)
        && !program.flags.contains(ProgramFlag::Repeat)
        && program.episode_number.unwrap_or_default() == 0
        && let Some(start) = program.start_date
    {
        show_id.push_str(&dotnet_ticks(start).to_string());
    }
    show_id
}

fn jellyfin_md5(value: &str) -> String {
    let mut hasher = Md5::new();
    for unit in value.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    let digest = hasher.finalize();
    let bytes = digest.as_slice();
    let mut result = format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[3], bytes[2], bytes[1], bytes[0], bytes[5], bytes[4], bytes[7], bytes[6]
    );
    for byte in &bytes[8..] {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

fn dotnet_ticks(value: DateTime<Utc>) -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
    UNIX_EPOCH_TICKS
        + value.timestamp() * 10_000_000
        + i64::from(value.timestamp_subsec_nanos() / 100)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
