use std::fs;
use std::path::Path;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use jellyfin_model::ProviderIdMap;
use roxmltree::{Document, Node};

use crate::movie::{NfoImage, NfoLocalImage, NfoParseError};

/// Reference to an item inside a collection or playlist.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NfoLinkedChild {
    pub path: Option<String>,
    pub library_item_id: Option<String>,
    pub child_type: String,
}

/// Metadata parsed from a box set / collection XML or NFO file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoxSetNfo {
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub sort_name: Option<String>,
    pub forced_sort_name: Option<String>,
    pub display_order: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub genres: Vec<String>,
    pub studios: Vec<String>,
    pub tags: Vec<String>,
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
    pub collection_items: Vec<NfoLinkedChild>,
    pub remote_trailers: Vec<String>,
    pub remote_images: Vec<NfoImage>,
    pub local_images: Vec<NfoLocalImage>,
}

/// Parses box set metadata from XML or NFO content.
///
/// # Errors
///
/// Returns [`NfoParseError::Xml`] for malformed XML and
/// [`NfoParseError::UnexpectedRoot`] for a non-boxset XML root.
pub fn parse_box_set_xml(input: &str) -> Result<BoxSetNfo, NfoParseError> {
    parse_box_set_xml_with_file_lookup(input, |path| Path::new(path).is_file())
}

/// Parses box set metadata and resolves local artwork with `file_exists`.
pub fn parse_box_set_xml_with_file_lookup(
    input: &str,
    mut file_exists: impl FnMut(&str) -> bool,
) -> Result<BoxSetNfo, NfoParseError> {
    let document = Document::parse(input)?;
    let root = document.root_element();
    let root_name = root.tag_name().name().to_ascii_lowercase();
    if !matches!(
        root_name.as_str(),
        "boxset" | "collection" | "item" | "movie"
    ) {
        return Err(NfoParseError::UnexpectedRoot(root_name));
    }

    let mut box_set = BoxSetNfo::default();
    for node in root.children().filter(Node::is_element) {
        parse_box_set_node(node, &mut box_set, &mut file_exists);
    }
    Ok(box_set)
}

/// Loads and parses a box set XML or NFO file.
///
/// # Errors
///
/// Returns file I/O errors or parse errors.
pub fn parse_box_set_xml_file(path: impl AsRef<Path>) -> Result<BoxSetNfo, NfoParseError> {
    parse_box_set_xml(&fs::read_to_string(path)?)
}

fn parse_box_set_node(
    node: Node<'_, '_>,
    box_set: &mut BoxSetNfo,
    _file_exists: &mut impl FnMut(&str) -> bool,
) {
    let tag = node.tag_name().name().to_ascii_lowercase();
    match tag.as_str() {
        "title" | "name" | "localtitle" => box_set.name = normalized_text(node),
        "originaltitle" => box_set.original_title = normalized_text(node),
        "plot" | "biography" | "review" | "overview" => box_set.overview = normalized_text(node),
        "tagline" => box_set.tagline = normalized_text(node),
        "sortname" => box_set.sort_name = normalized_text(node),
        "sorttitle" => box_set.forced_sort_name = normalized_text(node),
        "displayorder" => box_set.display_order = normalized_text(node),
        "language" => box_set.preferred_metadata_language = normalized_text(node),
        "countrycode" => box_set.preferred_metadata_country_code = normalized_text(node),
        "customrating" => box_set.custom_rating = normalized_text(node),
        "mpaa" => box_set.official_rating = normalized_text(node),
        "lockdata" => {
            box_set.is_locked =
                normalized_text(node).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        }
        "lockedfields" => {
            if let Some(val) = normalized_text(node) {
                box_set.locked_fields = val
                    .split('|')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
        "genre" => {
            if let Some(val) = normalized_text(node) {
                for g in val.split('/') {
                    let trimmed = g.trim();
                    if !trimmed.is_empty() {
                        box_set.genres.push(trimmed.to_owned());
                    }
                }
            }
        }
        "studio" => {
            if let Some(val) = normalized_text(node)
                && !box_set.studios.contains(&val)
            {
                box_set.studios.push(val);
            }
        }
        "tag" | "style" => {
            if let Some(val) = normalized_text(node)
                && !box_set.tags.contains(&val)
            {
                box_set.tags.push(val);
            }
        }
        "country" => {
            if let Some(val) = normalized_text(node) {
                for c in val.split('/') {
                    let trimmed = c.trim();
                    if !trimmed.is_empty() {
                        box_set.production_locations.push(trimmed.to_owned());
                    }
                }
            }
        }
        "premiered" | "aired" | "releasedate" => {
            if let Some(date) = parse_date(node) {
                box_set.premiere_date = Some(date);
                if box_set.production_year.is_none() {
                    box_set.production_year = Some(date.year());
                }
            }
        }
        "year" => {
            if let Some(year) = normalized_text(node).and_then(|y| y.parse().ok()) {
                box_set.production_year = Some(year);
            }
        }
        "collectionitems" => {
            for child in node.children().filter(|c| c.has_tag_name("CollectionItem")) {
                if let Some(item) = parse_linked_child(child) {
                    box_set.collection_items.push(item);
                }
            }
        }
        "tmdbid" | "tmdbcolid" => {
            if let Some(id) = normalized_text(node) {
                box_set.provider_ids.insert("Tmdb".to_owned(), id);
            }
        }
        "imdbid" => {
            if let Some(id) = normalized_text(node) {
                box_set.provider_ids.insert("Imdb".to_owned(), id);
            }
        }
        _ => {}
    }
}

fn parse_linked_child(node: Node<'_, '_>) -> Option<NfoLinkedChild> {
    let mut child = NfoLinkedChild {
        child_type: "Manual".to_owned(),
        ..Default::default()
    };
    for item in node.children().filter(Node::is_element) {
        match item.tag_name().name().to_ascii_lowercase().as_str() {
            "path" => child.path = normalized_text(item),
            "itemid" | "libraryitemid" => child.library_item_id = normalized_text(item),
            _ => {}
        }
    }
    (child.path.is_some() || child.library_item_id.is_some()).then_some(child)
}

fn normalized_text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn parse_date(node: Node<'_, '_>) -> Option<NaiveDate> {
    normalized_text(node).and_then(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok())
}
