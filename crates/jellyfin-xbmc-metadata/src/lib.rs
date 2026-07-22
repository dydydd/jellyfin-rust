//! Kodi/XBMC-compatible NFO metadata parsing.

mod movie;

pub use movie::{
    ImageType, MovieNfo, NfoImage, NfoParseError, NfoPerson, NfoUserData, PersonKind,
    Video3dFormat, parse_movie_nfo, parse_movie_nfo_file,
};
