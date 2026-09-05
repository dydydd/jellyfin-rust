/// Canonical Jellyfin item kinds and the CLR names used by older databases.
///
/// Keep this table aligned with the official server's `ItemTypeLookup`. It is
/// shared by persistence filters and controller hydration so the two paths do
/// not drift apart.
pub const OFFICIAL_ITEM_TYPE_ALIASES: &[(&str, &str)] = &[
    (
        "AggregateFolder",
        "MediaBrowser.Controller.Entities.AggregateFolder",
    ),
    ("Audio", "MediaBrowser.Controller.Entities.Audio.Audio"),
    ("AudioBook", "MediaBrowser.Controller.Entities.AudioBook"),
    (
        "BasePluginFolder",
        "MediaBrowser.Controller.Entities.BasePluginFolder",
    ),
    ("Book", "MediaBrowser.Controller.Entities.Book"),
    ("BoxSet", "MediaBrowser.Controller.Entities.Movies.BoxSet"),
    ("Channel", "MediaBrowser.Controller.Channels.Channel"),
    (
        "CollectionFolder",
        "MediaBrowser.Controller.Entities.CollectionFolder",
    ),
    ("Episode", "MediaBrowser.Controller.Entities.TV.Episode"),
    ("Folder", "MediaBrowser.Controller.Entities.Folder"),
    ("Genre", "MediaBrowser.Controller.Entities.Genre"),
    ("Movie", "MediaBrowser.Controller.Entities.Movies.Movie"),
    (
        "LiveTvChannel",
        "MediaBrowser.Controller.LiveTv.LiveTvChannel",
    ),
    (
        "LiveTvProgram",
        "MediaBrowser.Controller.LiveTv.LiveTvProgram",
    ),
    (
        "MusicAlbum",
        "MediaBrowser.Controller.Entities.Audio.MusicAlbum",
    ),
    (
        "MusicArtist",
        "MediaBrowser.Controller.Entities.Audio.MusicArtist",
    ),
    (
        "MusicGenre",
        "MediaBrowser.Controller.Entities.Audio.MusicGenre",
    ),
    ("MusicVideo", "MediaBrowser.Controller.Entities.MusicVideo"),
    ("Person", "MediaBrowser.Controller.Entities.Person"),
    ("Photo", "MediaBrowser.Controller.Entities.Photo"),
    ("PhotoAlbum", "MediaBrowser.Controller.Entities.PhotoAlbum"),
    ("Playlist", "MediaBrowser.Controller.Playlists.Playlist"),
    (
        "PlaylistsFolder",
        "Emby.Server.Implementations.Playlists.PlaylistsFolder",
    ),
    ("Season", "MediaBrowser.Controller.Entities.TV.Season"),
    ("Series", "MediaBrowser.Controller.Entities.TV.Series"),
    ("Studio", "MediaBrowser.Controller.Entities.Studio"),
    ("Trailer", "MediaBrowser.Controller.Entities.Trailer"),
    (
        "UserRootFolder",
        "MediaBrowser.Controller.Entities.UserRootFolder",
    ),
    ("UserView", "MediaBrowser.Controller.Entities.UserView"),
    ("Video", "MediaBrowser.Controller.Entities.Video"),
    ("Year", "MediaBrowser.Controller.Entities.Year"),
];

/// Expands API-facing canonical item kinds to every supported persisted name.
///
/// ASP.NET binds `BaseItemKind` values case-insensitively before the official
/// repository translates them to CLR names. Accept both the canonical and CLR
/// spellings here because existing Rust clients may already send either form.
/// Unknown plugin-defined values remain unchanged.
pub(crate) fn expand_item_type_aliases(item_types: &[String]) -> Vec<String> {
    let mut expanded = Vec::with_capacity(item_types.len().saturating_mul(2));
    for item_type in item_types {
        let aliases = OFFICIAL_ITEM_TYPE_ALIASES
            .iter()
            .find(|(canonical, persisted)| {
                item_type.eq_ignore_ascii_case(canonical)
                    || item_type.eq_ignore_ascii_case(persisted)
            });
        if let Some(&(canonical, persisted)) = aliases {
            // Live TV has its own behavior and is intentionally outside this
            // library-query compatibility change.
            if matches!(canonical, "LiveTvChannel" | "LiveTvProgram") {
                push_unique(&mut expanded, item_type);
            } else {
                push_unique(&mut expanded, canonical);
                push_unique(&mut expanded, persisted);
            }
        } else {
            push_unique(&mut expanded, item_type);
        }
    }
    expanded
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::expand_item_type_aliases;

    #[test]
    fn expands_known_names_case_insensitively_and_preserves_plugins() {
        let expanded = expand_item_type_aliases(&[
            "movie".to_owned(),
            "MEDIABROWSER.CONTROLLER.ENTITIES.TV.EPISODE".to_owned(),
            "Plugin.Media.SpecialItem".to_owned(),
            "Movie".to_owned(),
        ]);

        assert_eq!(
            expanded,
            [
                "Movie",
                "MediaBrowser.Controller.Entities.Movies.Movie",
                "Episode",
                "MediaBrowser.Controller.Entities.TV.Episode",
                "Plugin.Media.SpecialItem",
            ]
        );
    }

    #[test]
    fn leaves_live_tv_filters_unchanged() {
        assert_eq!(
            expand_item_type_aliases(&["livetvchannel".to_owned()]),
            ["livetvchannel"]
        );
    }
}
