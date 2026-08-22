//! Kodi/XBMC-compatible NFO metadata parsing.

mod location;
mod movie;
mod nfo;
mod writer;

pub use location::{MovieNfoLocation, MovieVideoType, movie_nfo_save_paths};
pub use movie::{
    ImageType, MovieNfo, NfoImage, NfoLocalImage, NfoParseError, NfoPerson, NfoUserData,
    PersonKind, Video3dFormat, parse_movie_nfo, parse_movie_nfo_file,
    parse_movie_nfo_with_file_lookup,
};
pub use writer::{movie_nfo_xml, save_movie_nfo};
pub use nfo::{
    MetadataNfoError, NfoDocumentKind, NfoFetchError, NfoMetadata, SeriesStatus, fetch_nfo_file,
    parse_nfo,
};
