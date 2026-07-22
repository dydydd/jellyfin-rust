use jellyfin_server_implementations::{PlaylistIndexError, determine_adjusted_playlist_index};

#[test]
fn adjusted_index_matches_the_official_matrix() {
    for (playlist_len, prior_index, new_index, expected) in
        [(1, 0, 0, 0), (3, 2, 0, 1), (4, 2, 1, 3)]
    {
        assert_eq!(
            determine_adjusted_playlist_index(playlist_len, prior_index, new_index),
            Ok(expected)
        );
    }
}

#[test]
fn empty_playlist_returns_a_typed_error() {
    assert_eq!(
        determine_adjusted_playlist_index(0, 0, 0),
        Err(PlaylistIndexError::EmptyPlaylist)
    );
}

#[test]
fn invalid_indices_return_typed_errors() {
    assert_eq!(
        determine_adjusted_playlist_index(3, 3, 0),
        Err(PlaylistIndexError::PriorIndexOutOfBounds {
            index: 3,
            playlist_len: 3,
        })
    );
    assert_eq!(
        determine_adjusted_playlist_index(3, 0, 3),
        Err(PlaylistIndexError::TargetIndexOutOfBounds {
            index: 3,
            playlist_len: 3,
        })
    );
}

#[test]
fn target_boundaries_have_explicit_non_panicking_results() {
    assert_eq!(determine_adjusted_playlist_index(4, 0, 0), Ok(0));
    assert_eq!(determine_adjusted_playlist_index(4, 2, 3), Ok(3));
    assert_eq!(determine_adjusted_playlist_index(4, 3, 3), Ok(4));
}
