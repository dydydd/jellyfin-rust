use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};

use jellyfin_extensions::remove_diacritics;
use thiserror::Error;
use uuid::Uuid;

use super::UserItemData;

const VERSION_DELIMITERS: [char; 3] = ['-', '_', '.'];

/// Pads digit runs so lexicographic ordering follows numeric ordering.
#[must_use]
pub fn modify_sort_chunks(name: &str) -> String {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };

    let mut output = String::with_capacity(name.len());
    let mut chunk = String::new();
    chunk.push(first);
    let mut digit_chunk = first.is_numeric();

    for character in characters {
        let is_digit = character.is_numeric();
        if is_digit != digit_chunk {
            append_sort_chunk(&mut output, &chunk, digit_chunk);
            chunk.clear();
            digit_chunk = is_digit;
        }
        chunk.push(character);
    }
    append_sort_chunk(&mut output, &chunk, digit_chunk);

    remove_diacritics(&output)
}

fn append_sort_chunk(output: &mut String, chunk: &str, is_digit: bool) {
    let character_count = chunk.chars().count();
    if is_digit && character_count < 10 {
        output.extend(std::iter::repeat_n('0', 10 - character_count));
    }
    output.push_str(chunk);
}

/// Finds the case-insensitive common prefix and retreats to a version delimiter.
#[must_use]
pub fn get_common_version_prefix(file_names: &[&str]) -> String {
    let Some(first) = file_names.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();

    for file_name in &file_names[1..] {
        let common = prefix
            .iter()
            .copied()
            .zip(file_name.chars())
            .take_while(|(left, right)| chars_equal_ignore_case(*left, *right))
            .count();
        prefix.truncate(common);
        if prefix.is_empty() {
            break;
        }
    }

    let prefix_is_whole_name = file_names
        .iter()
        .any(|file_name| file_name.chars().count() == prefix.len());
    if !prefix_is_whole_name {
        let structural_cut = prefix
            .iter()
            .rposition(|character| VERSION_DELIMITERS.contains(character))
            .map(|index| index + 1);
        let cut = structural_cut.or_else(|| {
            prefix
                .iter()
                .rposition(|character| *character == ' ')
                .map(|index| index + 1)
        });
        prefix.truncate(cut.unwrap_or_default());
    }

    prefix.into_iter().collect()
}

/// Builds the display label for a file-backed media source.
#[must_use]
pub fn get_media_source_name(
    path: &str,
    has_local_alternates: bool,
    common_prefix: Option<&str>,
) -> String {
    let display_name = file_stem(path);

    if let Some(prefix) = common_prefix
        && let Some(suffix) = strip_prefix_ignore_case(display_name, prefix)
        && !suffix.is_empty()
        && let Some(label) = trim_version_delimiters(suffix)
    {
        return label.to_owned();
    }

    if has_local_alternates
        && let Some(folder_name) = containing_folder_name(path)
        && let Some(suffix) = strip_prefix_ignore_case(display_name, folder_name)
        && !suffix.is_empty()
        && let Some(label) = trim_version_delimiters(suffix)
    {
        return label.to_owned();
    }

    display_name.to_owned()
}

fn chars_equal_ignore_case(left: char, right: char) -> bool {
    left == right
        || (left.is_ascii() && right.is_ascii() && left.eq_ignore_ascii_case(&right))
        || left.to_lowercase().eq(right.to_lowercase())
}

fn strip_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let mut value_indices = value.char_indices();
    for prefix_character in prefix.chars() {
        let (_, value_character) = value_indices.next()?;
        if !chars_equal_ignore_case(value_character, prefix_character) {
            return None;
        }
    }
    let byte_index = value_indices.next().map_or(value.len(), |(index, _)| index);
    Some(&value[byte_index..])
}

fn trim_version_delimiters(value: &str) -> Option<&str> {
    let trimmed = value.trim_start_matches(|character: char| {
        character == ' ' || VERSION_DELIMITERS.contains(&character)
    });
    (!trimmed.trim().is_empty()).then_some(trimmed)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or_default()
}

fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn containing_folder_name(path: &str) -> Option<&str> {
    let (parent, _) = path.rsplit_once(['/', '\\'])?;
    Some(file_name(parent))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoItem {
    pub id: Uuid,
    pub path: String,
    pub primary_version_id: Option<Uuid>,
    pub width: Option<i32>,
}

impl VideoItem {
    #[must_use]
    pub fn new(id: Uuid, path: impl Into<String>) -> Self {
        Self {
            id,
            path: path.into(),
            primary_version_id: None,
            width: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSourceVersion {
    pub id: String,
    pub item_id: Uuid,
    pub name: String,
    pub path: String,
    pub width: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VersionPlaybackUpdate {
    MarkPlayed {
        item_id: Uuid,
        playback_position_ticks: Option<i64>,
    },
    MarkUnplayed {
        item_id: Uuid,
        user_data: UserItemData,
    },
}

impl VersionPlaybackUpdate {
    #[must_use]
    pub const fn item_id(&self) -> Uuid {
        match self {
            Self::MarkPlayed { item_id, .. } | Self::MarkUnplayed { item_id, .. } => *item_id,
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum VersionGroupError {
    #[error("version {0} is not in the group")]
    UnknownVersion(Uuid),
    #[error("version {0} is already in the group")]
    DuplicateVersion(Uuid),
}

/// A media-version graph with local and linked alternates represented as edges.
#[derive(Debug, Default, Clone)]
pub struct VersionGroup {
    items: HashMap<Uuid, VideoItem>,
    links: HashMap<Uuid, Vec<Uuid>>,
}

impl VersionGroup {
    #[must_use]
    pub fn new(primary: VideoItem) -> Self {
        let id = primary.id;
        Self {
            items: HashMap::from([(id, primary)]),
            links: HashMap::from([(id, Vec::new())]),
        }
    }

    /// Adds a version without connecting it to another version yet.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::DuplicateVersion`] for a duplicate id.
    pub fn insert(&mut self, item: VideoItem) -> Result<(), VersionGroupError> {
        let id = item.id;
        let primary_version_id = item.primary_version_id;
        if let Some(primary_id) = primary_version_id
            && !self.items.contains_key(&primary_id)
        {
            return Err(VersionGroupError::UnknownVersion(primary_id));
        }
        match self.items.entry(id) {
            Entry::Occupied(_) => return Err(VersionGroupError::DuplicateVersion(id)),
            Entry::Vacant(entry) => {
                entry.insert(item);
            }
        }
        self.links.entry(id).or_default();
        if let Some(primary_id) = primary_version_id {
            push_unique(self.links.entry(id).or_default(), primary_id);
            push_unique(self.links.entry(primary_id).or_default(), id);
        }
        Ok(())
    }

    /// Connects two local or linked alternate versions.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::UnknownVersion`] if either id is absent.
    pub fn link(&mut self, left: Uuid, right: Uuid) -> Result<(), VersionGroupError> {
        if !self.items.contains_key(&left) {
            return Err(VersionGroupError::UnknownVersion(left));
        }
        if !self.items.contains_key(&right) {
            return Err(VersionGroupError::UnknownVersion(right));
        }
        push_unique(self.links.entry(left).or_default(), right);
        push_unique(self.links.entry(right).or_default(), left);
        Ok(())
    }

    /// Returns one matching version visible from `source_id`.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::UnknownVersion`] when `source_id` is absent.
    pub fn alternate_version(
        &self,
        source_id: Uuid,
        item_id: Uuid,
    ) -> Result<Option<&VideoItem>, VersionGroupError> {
        Ok(self
            .all_versions(source_id)?
            .into_iter()
            .find(|item| item.id == item_id))
    }

    /// Returns every connected version exactly once, starting with the source.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::UnknownVersion`] when `source_id` is absent.
    pub fn all_versions(&self, source_id: Uuid) -> Result<Vec<&VideoItem>, VersionGroupError> {
        if !self.items.contains_key(&source_id) {
            return Err(VersionGroupError::UnknownVersion(source_id));
        }

        let mut visited = HashSet::new();
        let mut pending = VecDeque::from([source_id]);
        let mut versions = Vec::new();
        while let Some(id) = pending.pop_front() {
            if !visited.insert(id) {
                continue;
            }
            if let Some(item) = self.items.get(&id) {
                versions.push(item);
            }
            if let Some(neighbors) = self.links.get(&id) {
                pending.extend(neighbors.iter().copied());
            }
        }
        Ok(versions)
    }

    /// Builds media sources with the queried version ordered first.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::UnknownVersion`] when `source_id` is absent.
    pub fn media_sources(
        &self,
        source_id: Uuid,
    ) -> Result<Vec<MediaSourceVersion>, VersionGroupError> {
        let versions = self.all_versions(source_id)?;
        let names: Vec<_> = versions
            .iter()
            .map(|version| file_stem(&version.path))
            .collect();
        let common_prefix = (names.len() >= 2)
            .then(|| get_common_version_prefix(&names))
            .filter(|prefix| !prefix.is_empty());

        let mut sources: Vec<_> = versions
            .into_iter()
            .map(|item| MediaSourceVersion {
                id: item.id.simple().to_string(),
                item_id: item.id,
                name: get_media_source_name(&item.path, true, common_prefix.as_deref()),
                // ALLOW: the returned DTO owns its path while the version graph remains reusable.
                path: item.path.clone(),
                width: item.width,
            })
            .collect();
        sources.sort_by(|left, right| {
            (right.item_id == source_id)
                .cmp(&(left.item_id == source_id))
                .then_with(|| right.width.cmp(&left.width))
        });
        Ok(sources)
    }

    /// Produces updates for every alternate version, excluding the source.
    ///
    /// # Errors
    ///
    /// Returns [`VersionGroupError::UnknownVersion`] when `source_id` is absent.
    pub fn propagate_played_state(
        &self,
        source_id: Uuid,
        played: bool,
        reset_position: bool,
        mut existing_user_data: HashMap<Uuid, UserItemData>,
    ) -> Result<Vec<VersionPlaybackUpdate>, VersionGroupError> {
        let mut updates = Vec::new();
        for item in self.all_versions(source_id)? {
            if item.id == source_id {
                continue;
            }
            if played {
                updates.push(VersionPlaybackUpdate::MarkPlayed {
                    item_id: item.id,
                    playback_position_ticks: reset_position.then_some(0),
                });
            } else if let Some(mut user_data) = existing_user_data.remove(&item.id) {
                user_data.reset_played_state();
                updates.push(VersionPlaybackUpdate::MarkUnplayed {
                    item_id: item.id,
                    user_data,
                });
            }
        }
        Ok(updates)
    }
}

fn push_unique(values: &mut Vec<Uuid>, value: Uuid) {
    if !values.contains(&value) {
        values.push(value);
    }
}
