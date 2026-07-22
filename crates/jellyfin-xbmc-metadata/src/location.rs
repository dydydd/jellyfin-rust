use std::path::PathBuf;

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
