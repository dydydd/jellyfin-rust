use thiserror::Error;

/// Invalid indices supplied while calculating a playlist move position.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlaylistIndexError {
    #[error("cannot determine a move position for an empty playlist")]
    EmptyPlaylist,
    #[error("prior item index {index} is outside playlist length {playlist_len}")]
    PriorIndexOutOfBounds { index: usize, playlist_len: usize },
    #[error("target index {index} is outside playlist length {playlist_len}")]
    TargetIndexOutOfBounds { index: usize, playlist_len: usize },
}

/// Determines the full-playlist insertion index for a visible-item move.
///
/// `prior_index_all_children` is the position in the complete playlist of the
/// visible item selected as the target's predecessor. `new_index` is the
/// requested position among visible items. The returned index is expressed
/// against the complete all-children list before the moved item is removed. A
/// value equal to `playlist_len` denotes the append boundary.
///
/// # Errors
///
/// Returns [`PlaylistIndexError::EmptyPlaylist`] for an empty playlist, or an
/// out-of-bounds variant when either supplied index is not in the original
/// playlist. These checks make the function safe to call before indexing a
/// playlist or calculating the insertion position.
pub fn determine_adjusted_playlist_index(
    playlist_len: usize,
    prior_index_all_children: usize,
    new_index: usize,
) -> Result<usize, PlaylistIndexError> {
    if playlist_len == 0 {
        return Err(PlaylistIndexError::EmptyPlaylist);
    }
    if prior_index_all_children >= playlist_len {
        return Err(PlaylistIndexError::PriorIndexOutOfBounds {
            index: prior_index_all_children,
            playlist_len,
        });
    }
    if new_index >= playlist_len {
        return Err(PlaylistIndexError::TargetIndexOutOfBounds {
            index: new_index,
            playlist_len,
        });
    }

    if new_index == 0 {
        Ok(prior_index_all_children.saturating_sub(1))
    } else {
        // Validation above proves this cannot overflow: prior_index < playlist_len.
        Ok(prior_index_all_children + 1)
    }
}
