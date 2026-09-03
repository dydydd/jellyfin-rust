use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use jellyfin_model::{MetadataProvider, ProviderIdMap};
use roxmltree::{Document, Node};

const TICKS_PER_SECOND: i64 = 10_000_000;
const YOUTUBE_OLD_PREFIX: &str = "plugin://plugin.video.youtube/?action=play_video&videoid=";
const YOUTUBE_NEW_PREFIX: &str = "plugin://plugin.video.youtube/play/?video_id=";
const YOUTUBE_WATCH_URL: &str = "https://www.youtube.com/watch?v=";

/// Person role stored in an NFO file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PersonKind {
    Actor,
    Director,
    Writer,
    Lyricist,
    Other(String),
}

/// Person metadata parsed from an actor, director, writer, or credits node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NfoPerson {
    pub name: String,
    pub role: String,
    pub kind: PersonKind,
    pub sort_order: Option<i32>,
    pub image_url: Option<String>,
}

/// Image category normalized from Kodi's `aspect` attribute.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageType {
    Primary,
    Logo,
    Banner,
    Thumb,
    Art,
    Disc,
    Backdrop,
}

/// First remote image found for a normalized image category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NfoImage {
    pub url: String,
    pub image_type: ImageType,
}

/// Existing local artwork referenced by an NFO file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NfoLocalImage {
    pub path: String,
    pub image_type: ImageType,
}

/// Video stereoscopic layout from NFO stream details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Video3dFormat {
    HalfSideBySide,
    HalfTopAndBottom,
    FullSideBySide,
    FullTopAndBottom,
    Mvc,
}

/// User playback fields read without writing to Jellyfin's user database.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NfoUserData {
    pub play_count: Option<i32>,
    pub played: Option<bool>,
    pub last_played_date: Option<NaiveDateTime>,
}

/// Service-independent subset of movie NFO metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovieNfo {
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub production_locations: Vec<String>,
    pub premiere_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub date_created: Option<NaiveDateTime>,
    pub production_year: Option<i32>,
    pub community_rating: Option<f32>,
    pub critic_rating: Option<f32>,
    pub custom_rating: Option<String>,
    pub official_rating: Option<String>,
    pub is_locked: bool,
    pub locked_fields: Vec<String>,
    pub preferred_metadata_language: Option<String>,
    pub preferred_metadata_country_code: Option<String>,
    pub collection_name: Option<String>,
    pub sort_name: Option<String>,
    pub forced_sort_name: Option<String>,
    pub tags: Vec<String>,
    pub display_order: Option<String>,
    pub aspect_ratio: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub runtime_ticks: Option<i64>,
    pub has_subtitles: bool,
    pub video_3d_format: Option<Video3dFormat>,
    pub people: Vec<NfoPerson>,
    pub remote_trailers: Vec<String>,
    pub remote_images: Vec<NfoImage>,
    pub local_images: Vec<NfoLocalImage>,
    pub user_data: NfoUserData,
}

/// Failure while loading or parsing NFO XML.
#[derive(Debug)]
pub enum NfoParseError {
    Io(std::io::Error),
    Xml(roxmltree::Error),
    UnexpectedRoot(String),
}

impl fmt::Display for NfoParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "NFO I/O failed: {error}"),
            Self::Xml(error) => write!(formatter, "invalid NFO XML: {error}"),
            Self::UnexpectedRoot(root) => write!(formatter, "expected movie NFO, found {root}"),
        }
    }
}

impl Error for NfoParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Xml(error) => Some(error),
            Self::UnexpectedRoot(_) => None,
        }
    }
}

impl From<std::io::Error> for NfoParseError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<roxmltree::Error> for NfoParseError {
    fn from(error: roxmltree::Error) -> Self {
        Self::Xml(error)
    }
}

/// Parses movie metadata from XML NFO content or provider-only URL content.
///
/// Text after `</movie>` is ignored, matching Jellyfin's support for provider
/// URLs appended after the XML document.
///
/// # Errors
///
/// Returns [`NfoParseError::Xml`] for malformed XML and
/// [`NfoParseError::UnexpectedRoot`] for a non-movie XML root.
pub fn parse_movie_nfo(input: &str) -> Result<MovieNfo, NfoParseError> {
    parse_movie_nfo_with_file_lookup(input, |path| Path::new(path).is_file())
}

/// Parses movie metadata and resolves local artwork with `file_exists`.
///
/// The lookup receives the exact path stored in the NFO. This keeps Windows
/// paths usable when parsing on another operating system.
///
/// # Errors
///
/// Returns [`NfoParseError::Xml`] for malformed XML and
/// [`NfoParseError::UnexpectedRoot`] for a non-movie XML root.
pub fn parse_movie_nfo_with_file_lookup(
    input: &str,
    mut file_exists: impl FnMut(&str) -> bool,
) -> Result<MovieNfo, NfoParseError> {
    if !input.contains('<') {
        let mut movie = MovieNfo::default();
        parse_provider_links(input, &mut movie.provider_ids);
        return Ok(movie);
    }

    let xml = xml_document_prefix(input);
    let document = Document::parse(xml)?;
    let root = document.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("movie") {
        return Err(NfoParseError::UnexpectedRoot(
            root.tag_name().name().to_owned(),
        ));
    }

    let mut movie = MovieNfo::default();
    for node in root.children().filter(Node::is_element) {
        parse_movie_node(node, &mut movie, &mut file_exists);
    }
    if let Some(suffix) = xml_document_suffix(input) {
        parse_provider_links(suffix, &mut movie.provider_ids);
    }
    Ok(movie)
}

/// Loads and parses a movie NFO file.
///
/// # Errors
///
/// Returns file I/O errors or the parse errors documented by
/// [`parse_movie_nfo`].
pub fn parse_movie_nfo_file(path: impl AsRef<Path>) -> Result<MovieNfo, NfoParseError> {
    parse_movie_nfo(&fs::read_to_string(path)?)
}

fn parse_movie_node(
    node: Node<'_, '_>,
    movie: &mut MovieNfo,
    file_exists: &mut impl FnMut(&str) -> bool,
) {
    let tag = node.tag_name().name().to_ascii_lowercase();
    if parse_basic_movie_node(&tag, node, movie) {
        return;
    }

    match tag.as_str() {
        "genre" => extend_slash_values(node, &mut movie.genres),
        "studio" => push_unique_text(node, &mut movie.studios),
        "country" => extend_slash_values(node, &mut movie.production_locations),
        "credits" => parse_credits(node, movie),
        "director" => parse_person_array(node, || PersonKind::Director, movie),
        "writer" => parse_person_array(node, || PersonKind::Writer, movie),
        "actor" => parse_actor(node, movie),
        "set" => parse_set(node, movie),
        "id" => parse_id_node(node, movie),
        "uniqueid" => parse_unique_id(node, movie),
        "trailer" => parse_trailer(node, movie),
        "thumb" => parse_image(node, None, movie, file_exists),
        "fanart" => {
            for thumb in node.children().filter(|child| child.has_tag_name("thumb")) {
                parse_image(thumb, Some("fanart"), movie, file_exists);
            }
        }
        "art" => {
            for child in node.children().filter(Node::is_element) {
                parse_image(child, Some(child.tag_name().name()), movie, file_exists);
            }
        }
        "fileinfo" => parse_file_info(node, movie),
        "playcount" => {
            if let Some(play_count) = parse_i32(node) {
                movie.user_data.play_count = Some(play_count);
            }
        }
        "watched" => {
            movie.user_data.played =
                normalized_text(node).and_then(|value| value.parse::<bool>().ok());
        }
        "lastplayed" => {
            if let Some(date) = parse_date_time(node) {
                movie.user_data.last_played_date = Some(date);
            }
        }
        "tmdbid" => set_provider_id(movie, MetadataProvider::Tmdb, normalized_text(node)),
        "imdbid" | "imdb_id" => {
            set_provider_id(movie, MetadataProvider::Imdb, normalized_text(node));
        }
        "tmdbcolid" | "collectionnumber" => {
            set_provider_id(
                movie,
                MetadataProvider::TmdbCollection,
                normalized_text(node),
            );
        }
        other => {
            if let Some(key) = extract_provider_key(other)
                && let Some(val) = normalized_text(node)
            {
                movie.provider_ids.insert(normalize_provider_name(key), val);
            }
        }
    }
}

fn parse_basic_movie_node(tag: &str, node: Node<'_, '_>, movie: &mut MovieNfo) -> bool {
    match tag {
        "title" | "name" | "localtitle" => movie.name = normalized_text(node),
        "originaltitle" => movie.original_title = normalized_text(node),
        "plot" | "biography" | "review" => movie.overview = normalized_text(node),
        "tagline" => movie.tagline = normalized_text(node),
        "language" => movie.preferred_metadata_language = normalized_text(node),
        "countrycode" => movie.preferred_metadata_country_code = normalized_text(node),
        "customrating" => movie.custom_rating = normalized_text(node),
        "mpaa" => movie.official_rating = normalized_text(node),
        "lockdata" => {
            movie.is_locked =
                normalized_text(node).is_some_and(|value| value.eq_ignore_ascii_case("true"));
        }
        "lockedfields" => parse_locked_fields(node, movie),
        "criticrating" => {
            if let Some(rating) = parse_f32(node) {
                movie.critic_rating = Some(rating);
            }
        }
        "communityrating" => {
            if let Some(rating) = parse_f32(node).filter(|rating| (0.0..=10.0).contains(rating)) {
                movie.community_rating = Some(rating);
            }
        }
        "rating" => {
            if let Some(rating) = parse_f32(node) {
                movie.community_rating = Some(rating);
            }
        }
        "ratings" => parse_ratings(node, movie),
        "premiered" | "aired" | "formed" | "releasedate" => {
            if let Some(date) = parse_date(node) {
                movie.premiere_date = Some(date);
                if movie.production_year.is_none() {
                    movie.production_year = Some(date.year());
                }
            }
        }
        "enddate" => {
            if let Some(date) = parse_date(node) {
                movie.end_date = Some(date);
            }
        }
        "dateadded" => {
            if let Some(date) = parse_date_time(node) {
                movie.date_created = Some(date);
            }
        }
        "year" => {
            if let Some(year) = normalized_text(node)
                .and_then(|year| year.parse().ok())
                .filter(|year| *year > 1850)
            {
                movie.production_year = Some(year);
            }
        }
        "runtime" => {
            if let Some(runtime) = normalized_text(node)
                .and_then(|runtime| runtime.split_ascii_whitespace().next().map(str::to_owned))
                .and_then(|runtime| runtime.parse::<i64>().ok())
                .and_then(|minutes| minutes.checked_mul(60 * TICKS_PER_SECOND))
            {
                movie.runtime_ticks = Some(runtime);
            }
        }
        "aspectratio" => movie.aspect_ratio = normalized_text(node),
        "sortname" => movie.sort_name = normalized_text(node),
        "sorttitle" => movie.forced_sort_name = normalized_text(node),
        "displayorder" => movie.display_order = normalized_text(node),
        "tag" | "style" => push_unique_text(node, &mut movie.tags),
        _ => return false,
    }

    true
}

fn parse_set(node: Node<'_, '_>, movie: &mut MovieNfo) {
    set_provider_id(
        movie,
        MetadataProvider::TmdbCollection,
        attribute_ignore_ascii_case(node, "tmdbcolid").map(str::to_owned),
    );
    movie.collection_name = node
        .children()
        .find(|child| child.is_element() && child.tag_name().name().eq_ignore_ascii_case("name"))
        .and_then(normalized_text)
        .or_else(|| normalized_text(node));
}

fn parse_id_node(node: Node<'_, '_>, movie: &mut MovieNfo) {
    set_provider_id(
        movie,
        MetadataProvider::Tmdb,
        attribute_ignore_ascii_case(node, "TMDB").map(str::to_owned),
    );
    set_provider_id(
        movie,
        MetadataProvider::Tvdb,
        attribute_ignore_ascii_case(node, "TVDB").map(str::to_owned),
    );
    let imdb_attribute = attribute_ignore_ascii_case(node, "IMDB").map(str::to_owned);
    let imdb = imdb_attribute.or_else(|| normalized_text(node).filter(|id| id.starts_with("tt")));
    set_provider_id(movie, MetadataProvider::Imdb, imdb);
}

fn parse_unique_id(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(id) = normalized_text(node) else {
        return;
    };
    let provider = attribute_ignore_ascii_case(node, "type")
        .or_else(|| attribute_ignore_ascii_case(node, "name"))
        .unwrap_or("Imdb");
    let normalized = normalize_provider_name(provider);
    movie.provider_ids.insert(normalized, id);
}

fn extract_provider_key(tag: &str) -> Option<&str> {
    if tag.len() <= 2 {
        return None;
    }
    let lower = tag.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "uniqueid"
            | "fileinfo"
            | "aspectratio"
            | "id"
            | "thumb"
            | "art"
            | "fanart"
            | "set"
            | "actor"
            | "director"
            | "writer"
            | "credits"
            | "genre"
            | "studio"
            | "country"
            | "trailer"
            | "ratings"
    ) {
        return None;
    }
    if tag.len() > 3 && tag[tag.len() - 3..].eq_ignore_ascii_case("_id") {
        let prefix = &tag[..tag.len() - 3];
        if !prefix.is_empty() {
            return Some(prefix);
        }
    }
    if tag.len() > 2 && tag[tag.len() - 2..].eq_ignore_ascii_case("id") {
        let prefix = &tag[..tag.len() - 2];
        if !prefix.is_empty() && !prefix.eq_ignore_ascii_case("val") {
            return Some(prefix);
        }
    }
    None
}

fn normalize_provider_name(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "imdb" | "imdb_id" => MetadataProvider::Imdb.as_str().to_owned(),
        "tmdb" => MetadataProvider::Tmdb.as_str().to_owned(),
        "tvdb" => MetadataProvider::Tvdb.as_str().to_owned(),
        "tmdbcol" | "tmdbcolid" | "collectionnumber" => {
            MetadataProvider::TmdbCollection.as_str().to_owned()
        }
        "tvmaze" => "Tvmaze".to_owned(),
        "tvrage" => "TvRage".to_owned(),
        "zap2it" => "Zap2It".to_owned(),
        "anidb" => "AniDB".to_owned(),
        "audiodb" => "AudioDb".to_owned(),
        "musicbrainz" => "MusicBrainz".to_owned(),
        "musicbrainzartist" => "MusicBrainzArtist".to_owned(),
        "musicbrainzalbum" => "MusicBrainzAlbum".to_owned(),
        "musicbrainzreleasegroup" => "MusicBrainzReleaseGroup".to_owned(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

fn parse_ratings(node: Node<'_, '_>, movie: &mut MovieNfo) {
    for rating in node.children().filter(|child| child.has_tag_name("rating")) {
        let Some(value) = rating
            .children()
            .find(|child| child.has_tag_name("value"))
            .and_then(parse_f32)
        else {
            continue;
        };
        let name = rating.attribute("name").unwrap_or_default();
        if name.to_ascii_lowercase().contains("tomato")
            && !name.to_ascii_lowercase().contains("audience")
            && !name.to_ascii_lowercase().contains("avg")
        {
            movie.critic_rating = Some(value);
        } else {
            movie.community_rating = Some(value);
        }
    }
}

fn parse_locked_fields(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(value) = normalized_text(node) else {
        return;
    };
    movie.locked_fields = value
        .split('|')
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(normalize_locked_field)
        .collect();
}

fn normalize_locked_field(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "cast" => "Cast".to_owned(),
        "genres" => "Genres".to_owned(),
        "productionlocations" => "ProductionLocations".to_owned(),
        "studios" => "Studios".to_owned(),
        "tags" => "Tags".to_owned(),
        "name" => "Name".to_owned(),
        "overview" => "Overview".to_owned(),
        "runtime" => "Runtime".to_owned(),
        "officialrating" => "OfficialRating".to_owned(),
        _ => value.to_owned(),
    }
}

fn parse_actor(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(name) = child_text(node, "name") else {
        return;
    };
    let kind = child_text(node, "type").map_or(PersonKind::Actor, |kind| parse_person_kind(&kind));
    movie.people.push(NfoPerson {
        name,
        role: child_text(node, "role").unwrap_or_default(),
        kind,
        sort_order: child_text(node, "order")
            .or_else(|| child_text(node, "sortorder"))
            .and_then(|order| order.parse().ok()),
        image_url: child_text(node, "thumb"),
    });
}

fn parse_person_array(node: Node<'_, '_>, kind: impl Fn() -> PersonKind, movie: &mut MovieNfo) {
    let Some(value) = normalized_text(node) else {
        return;
    };
    for name in split_person_array(&value) {
        movie.people.push(NfoPerson {
            name,
            role: String::new(),
            kind: kind(),
            sort_order: None,
            image_url: None,
        });
    }
}

fn parse_credits(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(value) = normalized_text(node) else {
        return;
    };
    for name in value
        .split('/')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        movie.people.push(NfoPerson {
            name: name.to_owned(),
            role: String::new(),
            kind: PersonKind::Writer,
            sort_order: None,
            image_url: None,
        });
    }
}

fn parse_trailer(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(value) = normalized_text(node) else {
        return;
    };
    if let Some(id) = strip_prefix_ignore_ascii_case(&value, YOUTUBE_OLD_PREFIX)
        .or_else(|| strip_prefix_ignore_ascii_case(&value, YOUTUBE_NEW_PREFIX))
    {
        movie
            .remote_trailers
            .push(format!("{YOUTUBE_WATCH_URL}{id}"));
    }
}

fn parse_image(
    node: Node<'_, '_>,
    parent_aspect: Option<&str>,
    movie: &mut MovieNfo,
    file_exists: &mut impl FnMut(&str) -> bool,
) {
    let aspect = node
        .attribute("aspect")
        .or(parent_aspect)
        .unwrap_or("poster");
    if aspect.contains('.') {
        return;
    }
    let Some(location) = normalized_text(node) else {
        return;
    };
    let image_type = match aspect.to_ascii_lowercase().as_str() {
        "clearlogo" | "logo" => ImageType::Logo,
        "banner" => ImageType::Banner,
        "landscape" | "thumb" => ImageType::Thumb,
        "clearart" | "art" => ImageType::Art,
        "discart" | "disc" => ImageType::Disc,
        "fanart" | "backdrop" => ImageType::Backdrop,
        _ => ImageType::Primary,
    };

    if is_remote_url(&location) {
        if !movie
            .remote_images
            .iter()
            .any(|image| image.image_type == image_type)
        {
            movie.remote_images.push(NfoImage {
                url: location,
                image_type,
            });
        }
    } else if is_absolute_file_location(&location)
        && !movie
            .local_images
            .iter()
            .any(|image| image.image_type == image_type)
        && file_exists(&location)
    {
        movie.local_images.push(NfoLocalImage {
            path: location,
            image_type,
        });
    }
}

fn parse_file_info(node: Node<'_, '_>, movie: &mut MovieNfo) {
    let Some(stream_details) = node
        .descendants()
        .find(|child| child.has_tag_name("streamdetails"))
    else {
        return;
    };
    if let Some(video) = stream_details
        .children()
        .find(|child| child.has_tag_name("video"))
    {
        movie.aspect_ratio = child_text(video, "aspect").or(movie.aspect_ratio.take());
        movie.width = child_text(video, "width").and_then(|value| value.parse().ok());
        movie.height = child_text(video, "height").and_then(|value| value.parse().ok());
        movie.runtime_ticks = child_text(video, "durationinseconds")
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|seconds| seconds.checked_mul(TICKS_PER_SECOND))
            .or(movie.runtime_ticks);
        movie.video_3d_format = child_text(video, "format3d")
            .or_else(|| child_text(video, "stereomode"))
            .and_then(|value| parse_3d_format(&value));
    }
    movie.has_subtitles = stream_details
        .children()
        .any(|child| child.has_tag_name("subtitle"));
}

fn parse_provider_links(input: &str, ids: &mut ProviderIdMap) {
    if let Some(id) = provider_url_id(input, "imdb.com/title/")
        .filter(|id| id.starts_with("tt") && id[2..].bytes().all(|byte| byte.is_ascii_digit()))
    {
        ids.insert(MetadataProvider::Imdb.as_str().to_owned(), id.to_owned());
    }
    if let Some(id) = provider_url_id(input, "themoviedb.org/movie/")
        .and_then(|value| value.split('-').next())
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
    {
        ids.insert(MetadataProvider::Tmdb.as_str().to_owned(), id.to_owned());
    }
}

fn provider_url_id<'a>(input: &'a str, marker: &str) -> Option<&'a str> {
    let lowercase = input.to_ascii_lowercase();
    let start = lowercase.find(marker)? + marker.len();
    input[start..]
        .split(|character: char| character == '/' || character.is_ascii_whitespace())
        .next()
}

fn xml_document_prefix(input: &str) -> &str {
    let lowercase = input.to_ascii_lowercase();
    lowercase
        .find("</movie>")
        .map_or(input, |index| &input[..index + "</movie>".len()])
}

fn xml_document_suffix(input: &str) -> Option<&str> {
    let lowercase = input.to_ascii_lowercase();
    let index = lowercase.find("</movie>")? + "</movie>".len();
    (!input[index..].trim().is_empty()).then_some(&input[index..])
}

fn normalized_text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn child_text(node: Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name().eq_ignore_ascii_case(name))
        .and_then(normalized_text)
}

fn attribute_ignore_ascii_case<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name().eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value())
}

fn parse_f32(node: Node<'_, '_>) -> Option<f32> {
    normalized_text(node).and_then(|value| value.replace(',', ".").parse().ok())
}

fn parse_i32(node: Node<'_, '_>) -> Option<i32> {
    normalized_text(node).and_then(|value| value.parse().ok())
}

fn parse_date(node: Node<'_, '_>) -> Option<NaiveDate> {
    normalized_text(node).and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok())
}

fn parse_date_time(node: Node<'_, '_>) -> Option<NaiveDateTime> {
    let value = normalized_text(node)?;
    ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&value, format).ok())
}

fn set_provider_id(movie: &mut MovieNfo, provider: MetadataProvider, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        movie
            .provider_ids
            .insert(provider.as_str().to_owned(), value);
    }
}

fn split_person_array(value: &str) -> Vec<String> {
    let use_comma = !value.contains('|') && !value.contains(';');
    value
        .split(|character| {
            if use_comma {
                character == ','
            } else {
                character == '|' || character == ';'
            }
        })
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_person_kind(value: &str) -> PersonKind {
    match value.to_ascii_lowercase().as_str() {
        "actor" => PersonKind::Actor,
        "director" => PersonKind::Director,
        "writer" => PersonKind::Writer,
        "lyricist" => PersonKind::Lyricist,
        _ => PersonKind::Other(value.to_owned()),
    }
}

fn parse_3d_format(value: &str) -> Option<Video3dFormat> {
    match value.to_ascii_lowercase().as_str() {
        "hsbs" | "half side by side" => Some(Video3dFormat::HalfSideBySide),
        "htab" | "half top and bottom" => Some(Video3dFormat::HalfTopAndBottom),
        "fsbs" | "full side by side" => Some(Video3dFormat::FullSideBySide),
        "ftab" | "full top and bottom" => Some(Video3dFormat::FullTopAndBottom),
        "mvc" => Some(Video3dFormat::Mvc),
        _ => None,
    }
}

fn extend_slash_values(node: Node<'_, '_>, values: &mut Vec<String>) {
    let Some(value) = normalized_text(node) else {
        return;
    };
    values.extend(
        value
            .split('/')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned),
    );
}

fn push_unique_text(node: Node<'_, '_>, values: &mut Vec<String>) {
    if let Some(value) = normalized_text(node)
        && !values.contains(&value)
    {
        values.push(value);
    }
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn is_remote_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn is_absolute_file_location(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || matches!(
            value.as_bytes(),
            [drive, b':', b'\\' | b'/', ..] if drive.is_ascii_alphabetic()
        )
        || value
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
}

trait DateYear {
    fn year(self) -> i32;
}

impl DateYear for NaiveDate {
    fn year(self) -> i32 {
        chrono::Datelike::year(&self)
    }
}
