use std::path::{Path, PathBuf};

/// Physical layout used by a movie item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovieVideoType {
    File,
    Dvd,
}

/// Inputs needed to calculate all compatible movie NFO save locations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MovieNfoLocation {
    pub path: PathBuf,
    pub is_in_mixed_folder: bool,
    pub video_type: MovieVideoType,
}

/// Returns the movie NFO paths Jellyfin writes for the supplied layout.
#[must_use]
pub fn movie_nfo_save_paths(movie: &MovieNfoLocation) -> Vec<PathBuf> {
    let raw = movie.path.to_string_lossy();
    let separator = if raw.contains('\\') { '\\' } else { '/' };
    match movie.video_type {
        MovieVideoType::File => file_movie_paths(&raw, separator, movie.is_in_mixed_folder),
        MovieVideoType::Dvd => dvd_movie_paths(&raw, separator),
    }
}

/// Returns the NFO paths for an episode file.
#[must_use]
pub fn episode_nfo_save_paths(path: &Path) -> Vec<PathBuf> {
    vec![path.with_extension("nfo")]
}

/// Returns the NFO paths for a series folder.
#[must_use]
pub fn series_nfo_save_paths(series_path: &Path) -> Vec<PathBuf> {
    vec![series_path.join("tvshow.nfo")]
}

/// Returns the NFO paths for a season.
#[must_use]
pub fn season_nfo_save_paths(
    season_path: &Path,
    season_number: Option<i32>,
) -> Vec<PathBuf> {
    let mut paths = vec![season_path.join("season.nfo")];
    if let Some(num) = season_number {
        if num == 0 {
            paths.push(season_path.join("season-specials.nfo"));
        } else {
            paths.push(season_path.join(format!("season{num:02}.nfo")));
        }
    }
    paths
}

/// Returns the NFO paths for a music artist folder.
#[must_use]
pub fn artist_nfo_save_paths(artist_path: &Path) -> Vec<PathBuf> {
    vec![artist_path.join("artist.nfo")]
}

/// Returns the NFO paths for a music album folder.
#[must_use]
pub fn album_nfo_save_paths(album_path: &Path) -> Vec<PathBuf> {
    vec![album_path.join("album.nfo")]
}

/// Returns the XML/NFO paths for a box set collection folder.
#[must_use]
pub fn box_set_nfo_save_paths(box_set_path: &Path) -> Vec<PathBuf> {
    vec![
        box_set_path.join("collection.xml"),
        box_set_path.join("boxset.xml"),
    ]
}

/// Returns the XML paths for a playlist.
#[must_use]
pub fn playlist_nfo_save_paths(playlist_path: &Path) -> Vec<PathBuf> {
    vec![playlist_path.join("playlist.xml")]
}

/// Resolves the first existing NFO/XML file among candidates.
pub fn resolve_nfo_file<F>(candidates: &[PathBuf], mut file_exists: F) -> Option<PathBuf>
where
    F: FnMut(&Path) -> bool,
{
    candidates.iter().find(|p| file_exists(p.as_path())).cloned()
}

fn file_movie_paths(path: &str, separator: char, mixed: bool) -> Vec<PathBuf> {
    let file_stem = path
        .rsplit_once(separator)
        .map_or(path, |(_, file)| file)
        .rsplit_once('.')
        .map_or_else(
            || path.rsplit_once(separator).map_or(path, |(_, file)| file),
            |(stem, _)| stem,
        );
    let parent = path.rsplit_once(separator).map_or("", |(parent, _)| parent);
    let adjacent = join(parent, file_stem, separator, ".nfo");
    if mixed {
        vec![PathBuf::from(adjacent)]
    } else {
        vec![
            PathBuf::from(adjacent),
            PathBuf::from(join(parent, "movie", separator, ".nfo")),
        ]
    }
}

fn dvd_movie_paths(path: &str, separator: char) -> Vec<PathBuf> {
    let folder = path.trim_end_matches(separator);
    let name = folder
        .rsplit_once(separator)
        .map_or(folder, |(_, name)| name);
    vec![
        PathBuf::from(join(folder, name, separator, ".nfo")),
        PathBuf::from(format!(
            "{folder}{separator}VIDEO_TS{separator}VIDEO_TS.nfo"
        )),
        PathBuf::from(join(folder, "movie", separator, ".nfo")),
    ]
}

fn join(parent: &str, stem: &str, separator: char, extension: &str) -> String {
    if parent.is_empty() {
        format!("{stem}{extension}")
    } else {
        format!("{parent}{separator}{stem}{extension}")
    }
}
