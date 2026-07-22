//! Kodi/XBMC-compatible NFO metadata parsing.

mod location;
mod movie;
mod nfo;

pub use location::{MovieNfoLocation, MovieVideoType, movie_nfo_save_paths};
pub use movie::{
    ImageType, MovieNfo, NfoImage, NfoParseError, NfoPerson, NfoUserData, PersonKind,
    Video3dFormat, parse_movie_nfo, parse_movie_nfo_file,
};
pub use nfo::{
    MetadataNfoError, NfoDocumentKind, NfoFetchError, NfoMetadata, SeriesStatus, fetch_nfo_file,
    parse_nfo,
};
