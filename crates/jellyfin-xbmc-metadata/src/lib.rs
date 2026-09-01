//! Kodi/XBMC-compatible NFO metadata parsing.

pub mod boxset;
pub mod location;
pub mod movie;
mod nfo;
pub mod playlist;
mod writer;

pub use boxset::{
    BoxSetNfo, NfoLinkedChild, parse_box_set_xml, parse_box_set_xml_file,
    parse_box_set_xml_with_file_lookup,
};
pub use location::{
    MovieNfoLocation, MovieVideoType, album_nfo_save_paths, artist_nfo_save_paths,
    box_set_nfo_save_paths, episode_nfo_save_paths, movie_nfo_save_paths, playlist_nfo_save_paths,
    resolve_nfo_file, season_nfo_save_paths, series_nfo_save_paths,
};
pub use movie::{
    ImageType, MovieNfo, NfoImage, NfoLocalImage, NfoParseError, NfoPerson, NfoUserData,
    PersonKind, Video3dFormat, parse_movie_nfo, parse_movie_nfo_file,
    parse_movie_nfo_with_file_lookup,
};
pub use nfo::{
    MetadataNfoError, NfoDocumentKind, NfoFetchError, NfoMetadata, SeriesStatus, fetch_nfo_file,
    parse_nfo,
};
pub use playlist::{PlaylistNfo, PlaylistShare, parse_playlist_xml, parse_playlist_xml_file};
pub use writer::{NfoSaveKind, movie_nfo_xml, nfo_save_path, nfo_xml, save_movie_nfo, save_nfo};
