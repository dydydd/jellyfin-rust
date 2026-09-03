use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, Weekday};
use jellyfin_model::{MetadataProvider, ProviderIdMap};
use roxmltree::{Document, Node};

use crate::{ImageType, NfoImage, NfoPerson, PersonKind};

const TICKS_PER_SECOND: i64 = 10_000_000;

/// NFO document families supported by Jellyfin's XBMC metadata readers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfoDocumentKind {
    Episode,
    MusicAlbum,
    MusicArtist,
    MusicVideo,
    Season,
    Series,
}

impl NfoDocumentKind {
    const fn root(self) -> &'static str {
        match self {
            Self::Episode => "episodedetails",
            Self::MusicAlbum => "album",
            Self::MusicArtist => "artist",
            Self::MusicVideo => "musicvideo",
            Self::Season => "season",
            Self::Series => "tvshow",
        }
    }
}

/// Normalized series lifecycle value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeriesStatus {
    Continuing,
    Ended,
    Other(String),
}

/// Metadata shared by the non-movie XBMC NFO readers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NfoMetadata {
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: String,
    pub sort_name: Option<String>,
    pub display_order: Option<String>,
    pub series_name: Option<String>,
    pub album: Option<String>,
    pub artists: Vec<String>,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub studios: Vec<String>,
    pub provider_ids: ProviderIdMap,
    pub production_year: Option<i32>,
    pub premiere_date: Option<NaiveDate>,
    pub date_created: Option<NaiveDateTime>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub airs_after_season_number: Option<i32>,
    pub airs_before_season_number: Option<i32>,
    pub airs_before_episode_number: Option<i32>,
    pub runtime_ticks: i64,
    pub official_rating: Option<String>,
    pub people: Vec<NfoPerson>,
    pub remote_images: Vec<NfoImage>,
    pub remote_trailers: Vec<String>,
    pub air_time: Option<String>,
    pub air_days: Vec<Weekday>,
    pub status: Option<SeriesStatus>,
    pub is_locked: bool,
}

/// Failure while parsing a non-movie NFO document.
#[derive(Debug)]
pub enum MetadataNfoError {
    Xml(roxmltree::Error),
    UnexpectedRoot {
        expected: &'static str,
        found: String,
    },
}

impl fmt::Display for MetadataNfoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => write!(formatter, "invalid NFO XML: {error}"),
            Self::UnexpectedRoot { expected, found } => {
                write!(formatter, "expected {expected} NFO, found {found}")
            }
        }
    }
}

impl Error for MetadataNfoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::UnexpectedRoot { .. } => None,
        }
    }
}

impl From<roxmltree::Error> for MetadataNfoError {
    fn from(error: roxmltree::Error) -> Self {
        Self::Xml(error)
    }
}

/// Failure at the file-provider boundary.
#[derive(Debug)]
pub enum NfoFetchError {
    MissingTarget,
    EmptyPath,
    Io(std::io::Error),
    Parse(MetadataNfoError),
}

impl fmt::Display for NfoFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTarget => formatter.write_str("metadata target is required"),
            Self::EmptyPath => formatter.write_str("NFO path is required"),
            Self::Io(error) => write!(formatter, "NFO I/O failed: {error}"),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl Error for NfoFetchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::MissingTarget | Self::EmptyPath => None,
        }
    }
}

/// Parses a supported non-movie NFO document.
///
/// Episode files may contain adjacent `episodedetails` documents. Their sparse
/// text fields are joined in episode order, matching Jellyfin's multi-episode
/// reader behavior.
///
/// # Errors
///
/// Returns an XML error for malformed input or [`MetadataNfoError::UnexpectedRoot`]
/// when the document type does not match `kind`.
pub fn parse_nfo(input: &str, kind: NfoDocumentKind) -> Result<NfoMetadata, MetadataNfoError> {
    if !input.contains('<') {
        let mut metadata = NfoMetadata::default();
        parse_provider_links(input, kind, &mut metadata.provider_ids);
        return Ok(metadata);
    }

    if kind == NfoDocumentKind::Episode && input.match_indices("<episodedetails").count() > 1 {
        return parse_multi_episode(input);
    }

    let xml = if kind == NfoDocumentKind::Series {
        xml_document_prefix(input)
    } else {
        input
    };
    let document = Document::parse(xml)?;
    let mut metadata = parse_root(document.root_element(), kind)?;
    if kind == NfoDocumentKind::Series
        && let Some(suffix) = xml_document_suffix(input)
    {
        parse_provider_links(suffix, kind, &mut metadata.provider_ids);
    }
    Ok(metadata)
}

/// Loads an NFO into an existing metadata target.
///
/// # Errors
///
/// Distinguishes missing targets and paths from file I/O and XML failures.
pub fn fetch_nfo_file(
    kind: NfoDocumentKind,
    target: Option<&mut NfoMetadata>,
    path: impl AsRef<Path>,
) -> Result<(), NfoFetchError> {
    let target = target.ok_or(NfoFetchError::MissingTarget)?;
    if path.as_ref().as_os_str().is_empty() {
        return Err(NfoFetchError::EmptyPath);
    }
    let input = fs::read_to_string(path).map_err(NfoFetchError::Io)?;
    *target = parse_nfo(&input, kind).map_err(NfoFetchError::Parse)?;
    Ok(())
}

fn parse_multi_episode(input: &str) -> Result<NfoMetadata, MetadataNfoError> {
    let wrapped = format!("<episodes>{input}</episodes>");
    let document = Document::parse(&wrapped)?;
    let mut episodes = document
        .root_element()
        .children()
        .filter(Node::is_element)
        .map(|node| parse_root(node, NfoDocumentKind::Episode))
        .collect::<Result<Vec<_>, _>>()?;
    let mut combined = episodes.remove(0);
    combined.name = joined(&episodes_with_first(&combined, &episodes, |item| {
        item.name.as_deref()
    }));
    combined.original_title = joined(&episodes_with_first(&combined, &episodes, |item| {
        item.original_title.as_deref()
    }));
    combined.overview = joined(&episodes_with_first(&combined, &episodes, |item| {
        item.overview.as_deref()
    }));
    let last_index = std::iter::once(&combined)
        .chain(&episodes)
        .filter_map(|episode| episode.index_number_end.or(episode.index_number))
        .max();
    combined.index_number_end = last_index.or(combined.index_number_end);
    Ok(combined)
}

fn episodes_with_first<'a>(
    first: &'a NfoMetadata,
    rest: &'a [NfoMetadata],
    get: impl Fn(&'a NfoMetadata) -> Option<&'a str>,
) -> Vec<&'a str> {
    std::iter::once(first).chain(rest).filter_map(get).collect()
}

fn joined(values: &[&str]) -> Option<String> {
    (!values.is_empty()).then(|| values.join(" / "))
}

fn parse_root(root: Node<'_, '_>, kind: NfoDocumentKind) -> Result<NfoMetadata, MetadataNfoError> {
    if !root.tag_name().name().eq_ignore_ascii_case(kind.root()) {
        return Err(MetadataNfoError::UnexpectedRoot {
            expected: kind.root(),
            found: root.tag_name().name().to_owned(),
        });
    }
    let mut metadata = NfoMetadata::default();
    for node in root.children().filter(Node::is_element) {
        parse_node(node, kind, &mut metadata);
    }
    Ok(metadata)
}

fn parse_node(node: Node<'_, '_>, kind: NfoDocumentKind, metadata: &mut NfoMetadata) {
    let tag = node.tag_name().name().to_ascii_lowercase();
    match tag.as_str() {
        "title" | "name" => metadata.name = text(node),
        "originaltitle" => metadata.original_title = text(node),
        "plot" | "review" | "biography" => metadata.overview = text(node),
        "tagline" => metadata.tagline = text(node).unwrap_or_default(),
        "sorttitle" | "sortname" => metadata.sort_name = text(node),
        "displayorder" => metadata.display_order = text(node),
        "showtitle" => metadata.series_name = text(node),
        "album" if kind == NfoDocumentKind::MusicVideo => metadata.album = text(node),
        "artist" if kind == NfoDocumentKind::MusicVideo => push_text(node, &mut metadata.artists),
        "genre" => extend_slash_values(node, &mut metadata.genres),
        "style" => push_text(node, &mut metadata.tags),
        "studio" => push_text(node, &mut metadata.studios),
        "year" => metadata.production_year = parse_i32(node).filter(|year| *year > 1850),
        "premiered" | "aired" | "releasedate" => set_premiere_date(node, metadata),
        "dateadded" => metadata.date_created = parse_date_time(node),
        "episode" => metadata.index_number = parse_i32(node),
        "episodenumberend" => metadata.index_number_end = parse_i32(node),
        "season" if kind == NfoDocumentKind::Episode => {
            metadata.parent_index_number = parse_i32(node);
        }
        "seasonnumber" if kind == NfoDocumentKind::Season => {
            metadata.index_number = parse_i32(node);
        }
        "airsafter_season" => metadata.airs_after_season_number = parse_i32(node),
        "airsbefore_season" => metadata.airs_before_season_number = parse_i32(node),
        "airsbefore_episode" => metadata.airs_before_episode_number = parse_i32(node),
        "runtime" => {
            metadata.runtime_ticks = parse_i64(node)
                .and_then(|minutes| minutes.checked_mul(60 * TICKS_PER_SECOND))
                .unwrap_or_default();
        }
        "mpaa" => metadata.official_rating = text(node),
        "credits" | "writer" => parse_people(node, || PersonKind::Writer, metadata),
        "director" => parse_people(node, || PersonKind::Director, metadata),
        "actor" => parse_actor(node, metadata),
        "thumb" => parse_image(node, None, metadata),
        "fanart" => {
            for thumb in node.children().filter(|child| child.has_tag_name("thumb")) {
                parse_image(thumb, Some("fanart"), metadata);
            }
        }
        "art" => {
            for child in node.children().filter(Node::is_element) {
                parse_image(child, Some(child.tag_name().name()), metadata);
            }
        }
        "fileinfo" => parse_file_info(node, metadata),
        "id" => parse_id(node, metadata),
        "uniqueid" => parse_unique_id(node, metadata),
        "imdbid" | "imdb_id" => set_provider(metadata, MetadataProvider::Imdb.as_str(), text(node)),
        "tmdbid" => set_provider(metadata, MetadataProvider::Tmdb.as_str(), text(node)),
        "tvdbid" => set_provider(metadata, MetadataProvider::Tvdb.as_str(), text(node)),
        "tmdbcolid" | "collectionnumber" => {
            set_provider(
                metadata,
                MetadataProvider::TmdbCollection.as_str(),
                text(node),
            );
        }
        "musicbrainzalbumid" => {
            set_provider(
                metadata,
                MetadataProvider::MusicBrainzAlbum.as_str(),
                text(node),
            );
        }
        "musicbrainzalbumartistid" => {
            set_provider(
                metadata,
                MetadataProvider::MusicBrainzAlbumArtist.as_str(),
                text(node),
            );
        }
        "musicbrainzartistid" => set_provider(
            metadata,
            MetadataProvider::MusicBrainzArtist.as_str(),
            text(node),
        ),
        "musicbrainzreleasegroupid" => {
            set_provider(
                metadata,
                MetadataProvider::MusicBrainzReleaseGroup.as_str(),
                text(node),
            );
        }
        "musicbrainztrackid" => {
            set_provider(
                metadata,
                MetadataProvider::MusicBrainzTrack.as_str(),
                text(node),
            );
        }
        "musicbrainzrecordingid" => {
            set_provider(
                metadata,
                MetadataProvider::MusicBrainzRecording.as_str(),
                text(node),
            );
        }
        "audiodbalbumid" => {
            set_provider(
                metadata,
                MetadataProvider::AudioDbAlbum.as_str(),
                text(node),
            );
        }
        "audiodbartistid" => {
            set_provider(
                metadata,
                MetadataProvider::AudioDbArtist.as_str(),
                text(node),
            );
        }
        "zap2itid" => set_provider(metadata, MetadataProvider::Zap2It.as_str(), text(node)),
        "tvmazeid" => set_provider(metadata, MetadataProvider::TvMaze.as_str(), text(node)),
        "tvrageid" => set_provider(metadata, MetadataProvider::TvRage.as_str(), text(node)),
        "tvcomid" => set_provider(metadata, MetadataProvider::Tvcom.as_str(), text(node)),
        "airs_time" => metadata.air_time = text(node),
        "airs_dayofweek" => parse_air_days(node, metadata),
        "status" => metadata.status = text(node).map(|value| parse_status(&value)),
        "trailer" => {
            if let Some(value) = text(node).filter(|value| !value.is_empty()) {
                metadata.remote_trailers.push(value);
            }
        }
        "lockdata" => {
            metadata.is_locked = text(node).is_some_and(|value| value.eq_ignore_ascii_case("true"));
        }
        _ => {}
    }
}

fn set_premiere_date(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    if metadata.premiere_date.is_none()
        && let Some(date) = parse_date(node)
    {
        metadata.premiere_date = Some(date);
        metadata
            .production_year
            .get_or_insert(chrono::Datelike::year(&date));
    }
}

fn parse_actor(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    let Some(name) = child_text(node, "name") else {
        return;
    };
    let kind = match child_text(node, "type").as_deref() {
        Some(value) if value.eq_ignore_ascii_case("director") => PersonKind::Director,
        Some(value) if value.eq_ignore_ascii_case("writer") => PersonKind::Writer,
        Some(value) if value.eq_ignore_ascii_case("lyricist") => PersonKind::Lyricist,
        Some(value) if !value.eq_ignore_ascii_case("actor") => PersonKind::Other(value.to_owned()),
        _ => PersonKind::Actor,
    };
    metadata.people.push(NfoPerson {
        name,
        role: child_text(node, "role").unwrap_or_default(),
        kind,
        sort_order: child_text(node, "order")
            .or_else(|| child_text(node, "sortorder"))
            .and_then(|value| value.parse().ok()),
        image_url: child_text(node, "thumb"),
    });
}

fn parse_people(node: Node<'_, '_>, kind: impl Fn() -> PersonKind, metadata: &mut NfoMetadata) {
    let Some(value) = text(node) else {
        return;
    };
    for name in value
        .split(['/', '|', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        metadata.people.push(NfoPerson {
            name: name.to_owned(),
            role: String::new(),
            kind: kind(),
            sort_order: None,
            image_url: None,
        });
    }
}

fn parse_image(node: Node<'_, '_>, parent_aspect: Option<&str>, metadata: &mut NfoMetadata) {
    let Some(url) =
        text(node).filter(|url| url.starts_with("http://") || url.starts_with("https://"))
    else {
        return;
    };
    let aspect = node
        .attribute("aspect")
        .or(parent_aspect)
        .unwrap_or("poster");
    let image_type = match aspect.to_ascii_lowercase().as_str() {
        "clearlogo" | "logo" => ImageType::Logo,
        "banner" => ImageType::Banner,
        "landscape" | "thumb" => ImageType::Thumb,
        "clearart" | "art" => ImageType::Art,
        "discart" | "disc" => ImageType::Disc,
        "fanart" | "backdrop" => ImageType::Backdrop,
        _ => ImageType::Primary,
    };
    if !metadata
        .remote_images
        .iter()
        .any(|image| image.image_type == image_type)
    {
        metadata.remote_images.push(NfoImage { url, image_type });
    }
}

fn parse_file_info(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    let seconds = node
        .descendants()
        .find(|child| child.has_tag_name("durationinseconds"))
        .and_then(parse_i64);
    if let Some(ticks) = seconds.and_then(|value| value.checked_mul(TICKS_PER_SECOND)) {
        metadata.runtime_ticks = ticks;
    }
}

fn parse_id(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    for (attribute, provider) in [
        ("IMDB", MetadataProvider::Imdb.as_str()),
        ("TMDB", MetadataProvider::Tmdb.as_str()),
        ("TVDB", MetadataProvider::Tvdb.as_str()),
    ] {
        set_provider(
            metadata,
            provider,
            attribute_value(node, attribute).map(str::to_owned),
        );
    }
    let content = text(node).filter(|id| id.starts_with("tt"));
    if !metadata
        .provider_ids
        .contains_key(MetadataProvider::Imdb.as_str())
        && let Some(id) = content
    {
        set_provider(metadata, MetadataProvider::Imdb.as_str(), Some(id));
    }
}

fn parse_unique_id(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    let Some(provider) = attribute_value(node, "type") else {
        return;
    };
    let normalized = match provider.to_ascii_lowercase().as_str() {
        "imdb" | "imdb_id" => MetadataProvider::Imdb.as_str(),
        "tmdb" => MetadataProvider::Tmdb.as_str(),
        "tvdb" => MetadataProvider::Tvdb.as_str(),
        "tmdbcol" | "tmdbcolid" | "collectionnumber" => MetadataProvider::TmdbCollection.as_str(),
        "musicbrainzalbum" | "musicbrainzalbumid" => MetadataProvider::MusicBrainzAlbum.as_str(),
        "musicbrainzalbumartist" | "musicbrainzalbumartistid" => {
            MetadataProvider::MusicBrainzAlbumArtist.as_str()
        }
        "musicbrainzartist" | "musicbrainzartistid" => MetadataProvider::MusicBrainzArtist.as_str(),
        "musicbrainzreleasegroup" | "musicbrainzreleasegroupid" => {
            MetadataProvider::MusicBrainzReleaseGroup.as_str()
        }
        "musicbrainztrack" | "musicbrainztrackid" => MetadataProvider::MusicBrainzTrack.as_str(),
        "musicbrainzrecording" | "musicbrainzrecordingid" => {
            MetadataProvider::MusicBrainzRecording.as_str()
        }
        "audiodbalbum" | "audiodbalbumid" => MetadataProvider::AudioDbAlbum.as_str(),
        "audiodbartist" | "audiodbartistid" => MetadataProvider::AudioDbArtist.as_str(),
        "zap2it" | "zap2itid" => MetadataProvider::Zap2It.as_str(),
        "tvmaze" | "tvmazeid" => MetadataProvider::TvMaze.as_str(),
        "tvrage" | "tvrageid" => MetadataProvider::TvRage.as_str(),
        "tvcom" | "tvcomid" => MetadataProvider::Tvcom.as_str(),
        _ => provider,
    };
    set_provider(metadata, normalized, text(node));
}

fn parse_provider_links(input: &str, kind: NfoDocumentKind, ids: &mut ProviderIdMap) {
    if kind != NfoDocumentKind::Series {
        return;
    }
    let lowercase = input.to_ascii_lowercase();
    for marker in [
        "thetvdb.com/?tab=series&id=",
        "thetvdb.com/index.php?tab=series&id=",
    ] {
        if let Some(start) = lowercase.find(marker).map(|index| index + marker.len()) {
            let id = input[start..]
                .split(|character: char| !character.is_ascii_digit())
                .next()
                .unwrap_or_default();
            if !id.is_empty() {
                ids.insert(MetadataProvider::Tvdb.as_str().to_owned(), id.to_owned());
                return;
            }
        }
    }
}

fn xml_document_prefix(input: &str) -> &str {
    let lowercase = input.to_ascii_lowercase();
    lowercase
        .find("</tvshow>")
        .map_or(input, |index| &input[..index + "</tvshow>".len()])
}

fn xml_document_suffix(input: &str) -> Option<&str> {
    let lowercase = input.to_ascii_lowercase();
    let index = lowercase.find("</tvshow>")? + "</tvshow>".len();
    (!input[index..].trim().is_empty()).then_some(&input[index..])
}

fn parse_air_days(node: Node<'_, '_>, metadata: &mut NfoMetadata) {
    let Some(value) = text(node) else {
        return;
    };
    for day in value.split([',', '|']) {
        let parsed = match day.trim().to_ascii_lowercase().as_str() {
            "monday" => Some(Weekday::Mon),
            "tuesday" => Some(Weekday::Tue),
            "wednesday" => Some(Weekday::Wed),
            "thursday" => Some(Weekday::Thu),
            "friday" => Some(Weekday::Fri),
            "saturday" => Some(Weekday::Sat),
            "sunday" => Some(Weekday::Sun),
            _ => None,
        };
        if let Some(day) = parsed
            && !metadata.air_days.contains(&day)
        {
            metadata.air_days.push(day);
        }
    }
}

fn parse_status(value: &str) -> SeriesStatus {
    match value.to_ascii_lowercase().as_str() {
        "continuing" => SeriesStatus::Continuing,
        "ended" => SeriesStatus::Ended,
        _ => SeriesStatus::Other(value.to_owned()),
    }
}

fn set_provider(metadata: &mut NfoMetadata, provider: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        metadata.provider_ids.insert(provider.to_owned(), value);
    }
}

fn extend_slash_values(node: Node<'_, '_>, values: &mut Vec<String>) {
    let Some(value) = text(node) else {
        return;
    };
    for part in value
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if !values.iter().any(|existing| existing == part) {
            values.push(part.to_owned());
        }
    }
}

fn push_text(node: Node<'_, '_>, values: &mut Vec<String>) {
    if let Some(value) = text(node)
        && !values.contains(&value)
    {
        values.push(value);
    }
}

fn text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn child_text(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name().eq_ignore_ascii_case(name))
        .and_then(text)
}

fn attribute_value<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name().eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value())
}

fn parse_i32(node: Node<'_, '_>) -> Option<i32> {
    text(node).and_then(|value| value.parse().ok())
}

fn parse_i64(node: Node<'_, '_>) -> Option<i64> {
    text(node)
        .and_then(|value| value.split_ascii_whitespace().next().map(str::to_owned))
        .and_then(|value| value.parse().ok())
}

fn parse_date(node: Node<'_, '_>) -> Option<NaiveDate> {
    text(node).and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok())
}

fn parse_date_time(node: Node<'_, '_>) -> Option<NaiveDateTime> {
    let value = text(node)?;
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&value, format).ok())
}
