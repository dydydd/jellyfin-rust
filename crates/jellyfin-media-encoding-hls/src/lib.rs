//! Pure dynamic HLS playlist generation algorithms.

mod playlist;

pub use playlist::{
    CreateMainPlaylistRequest, HlsPlaylistError, compute_equal_length_segments, compute_segments,
    create_main_playlist, is_extraction_allowed_for_file,
};
