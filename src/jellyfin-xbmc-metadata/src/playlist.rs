use std::fs;
use std::path::Path;

use jellyfin_model::ProviderIdMap;
use roxmltree::{Document, Node};

use crate::boxset::NfoLinkedChild;
use crate::movie::{NfoImage, NfoLocalImage, NfoParseError};

/// User permission entry in a playlist XML file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlaylistShare {
    pub user_id: Option<String>,
    pub can_edit: bool,
}

/// Metadata parsed from a playlist XML file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistNfo {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub playlist_media_type: Option<String>,
    pub provider_ids: ProviderIdMap,
    pub is_locked: bool,
    pub locked_fields: Vec<String>,
    pub playlist_items: Vec<NfoLinkedChild>,
    pub shares: Vec<PlaylistShare>,
    pub remote_trailers: Vec<String>,
    pub remote_images: Vec<NfoImage>,
    pub local_images: Vec<NfoLocalImage>,
}

/// Parses playlist metadata from XML content.
///
/// # Errors
///
/// Returns [`NfoParseError::Xml`] for malformed XML and
/// [`NfoParseError::UnexpectedRoot`] for a non-playlist XML root.
pub fn parse_playlist_xml(input: &str) -> Result<PlaylistNfo, NfoParseError> {
    let document = Document::parse(input)?;
    let root = document.root_element();
    let root_name = root.tag_name().name().to_ascii_lowercase();
    if !matches!(root_name.as_str(), "playlist" | "item") {
        return Err(NfoParseError::UnexpectedRoot(root_name));
    }

    let mut playlist = PlaylistNfo::default();
    for node in root.children().filter(Node::is_element) {
        parse_playlist_node(node, &mut playlist);
    }
    Ok(playlist)
}

/// Loads and parses a playlist XML file.
///
/// # Errors
///
/// Returns file I/O errors or parse errors.
pub fn parse_playlist_xml_file(path: impl AsRef<Path>) -> Result<PlaylistNfo, NfoParseError> {
    parse_playlist_xml(&fs::read_to_string(path)?)
}

fn parse_playlist_node(node: Node<'_, '_>, playlist: &mut PlaylistNfo) {
    let tag = node.tag_name().name().to_ascii_lowercase();
    match tag.as_str() {
        "title" | "name" => playlist.name = normalized_text(node),
        "plot" | "biography" | "review" | "overview" => playlist.overview = normalized_text(node),
        "playlistmediatype" => playlist.playlist_media_type = normalized_text(node),
        "lockdata" => {
            playlist.is_locked =
                normalized_text(node).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        }
        "lockedfields" => {
            if let Some(val) = normalized_text(node) {
                playlist.locked_fields = val
                    .split('|')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
        "playlistitems" => {
            for child in node.children().filter(|c| c.has_tag_name("PlaylistItem")) {
                if let Some(item) = parse_linked_child(child) {
                    playlist.playlist_items.push(item);
                }
            }
        }
        "shares" => {
            for child in node.children().filter(|c| c.has_tag_name("Share")) {
                if let Some(share) = parse_share(child) {
                    playlist.shares.push(share);
                }
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

fn parse_share(node: Node<'_, '_>) -> Option<PlaylistShare> {
    let mut user_id = None;
    let mut can_edit = false;
    for item in node.children().filter(Node::is_element) {
        match item.tag_name().name().to_ascii_lowercase().as_str() {
            "userid" => user_id = normalized_text(item),
            "canedit" => {
                can_edit = normalized_text(item).is_some_and(|v| v.eq_ignore_ascii_case("true"));
            }
            _ => {}
        }
    }
    user_id.map(|uid| PlaylistShare {
        user_id: Some(uid),
        can_edit,
    })
}

fn normalized_text(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
