use jellyfin_model::{ExternalIdInfo, ExternalIdMediaType};

#[derive(Clone, Copy)]
struct ExternalIdDescriptor {
    name: &'static str,
    key: &'static str,
    media_type: Option<ExternalIdMediaType>,
    item_types: &'static [&'static str],
}

const DESCRIPTORS: &[ExternalIdDescriptor] = &[
    descriptor("Comic Vine", "ComicVine", None, &["Book"]),
    descriptor(
        "Comic Vine",
        "ComicVine",
        Some(ExternalIdMediaType::Person),
        &["Person"],
    ),
    descriptor("Google Books", "GoogleBooks", None, &["Book"]),
    descriptor("ISBN", "ISBN", None, &["Book"]),
    descriptor(
        "IMDb",
        "Imdb",
        None,
        &["Movie", "MusicVideo", "Series", "Episode", "Trailer"],
    ),
    descriptor(
        "IMDb",
        "Imdb",
        Some(ExternalIdMediaType::Person),
        &["Person"],
    ),
    descriptor("IMVDb", "Imvdb", None, &["MusicVideo"]),
    descriptor(
        "MusicBrainz",
        "MusicBrainzAlbumArtist",
        Some(ExternalIdMediaType::AlbumArtist),
        &["Audio"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzAlbum",
        Some(ExternalIdMediaType::Album),
        &["Audio", "MusicAlbum"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzArtist",
        Some(ExternalIdMediaType::Artist),
        &["MusicArtist"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzArtist",
        Some(ExternalIdMediaType::OtherArtist),
        &["Audio", "MusicAlbum"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzRecording",
        Some(ExternalIdMediaType::Recording),
        &["Audio"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzReleaseGroup",
        Some(ExternalIdMediaType::ReleaseGroup),
        &["Audio", "MusicAlbum"],
    ),
    descriptor(
        "MusicBrainz",
        "MusicBrainzTrack",
        Some(ExternalIdMediaType::Track),
        &["Audio"],
    ),
    descriptor("TheAudioDb", "AudioDbAlbum", None, &["MusicAlbum"]),
    descriptor(
        "TheAudioDb",
        "AudioDbAlbum",
        Some(ExternalIdMediaType::Album),
        &["Audio"],
    ),
    descriptor(
        "TheAudioDb",
        "AudioDbArtist",
        Some(ExternalIdMediaType::Artist),
        &["MusicArtist"],
    ),
    descriptor(
        "TheAudioDb",
        "AudioDbArtist",
        Some(ExternalIdMediaType::OtherArtist),
        &["Audio", "MusicAlbum"],
    ),
    descriptor(
        "TheMovieDb",
        "TmdbCollection",
        Some(ExternalIdMediaType::BoxSet),
        &["Movie", "MusicVideo", "Trailer"],
    ),
    descriptor(
        "TheMovieDb",
        "TmdbCollection",
        Some(ExternalIdMediaType::BoxSet),
        &["BoxSet"],
    ),
    descriptor(
        "TheMovieDb",
        "Tmdb",
        Some(ExternalIdMediaType::Episode),
        &["Episode"],
    ),
    descriptor(
        "TheMovieDb",
        "Tmdb",
        Some(ExternalIdMediaType::Movie),
        &["Movie"],
    ),
    descriptor(
        "TheMovieDb",
        "Tmdb",
        Some(ExternalIdMediaType::Person),
        &["Person"],
    ),
    descriptor(
        "TheMovieDb",
        "Tmdb",
        Some(ExternalIdMediaType::Season),
        &["Season"],
    ),
    descriptor(
        "TheMovieDb",
        "Tmdb",
        Some(ExternalIdMediaType::Series),
        &["Series"],
    ),
    descriptor(
        "TheTVDB",
        "Tvdb",
        Some(ExternalIdMediaType::Series),
        &["Series"],
    ),
    descriptor(
        "TheTVDB",
        "Tvdb",
        Some(ExternalIdMediaType::Season),
        &["Season"],
    ),
    descriptor(
        "TheTVDB",
        "Tvdb",
        Some(ExternalIdMediaType::Episode),
        &["Episode"],
    ),
    descriptor(
        "TVmaze",
        "TvMaze",
        Some(ExternalIdMediaType::Series),
        &["Series"],
    ),
    descriptor(
        "TV.com",
        "Tvcom",
        Some(ExternalIdMediaType::Series),
        &["Series"],
    ),
    descriptor(
        "TVRage",
        "TvRage",
        Some(ExternalIdMediaType::Series),
        &["Series"],
    ),
    descriptor("Zap2It", "Zap2It", None, &["Series"]),
];

const fn descriptor(
    name: &'static str,
    key: &'static str,
    media_type: Option<ExternalIdMediaType>,
    item_types: &'static [&'static str],
) -> ExternalIdDescriptor {
    ExternalIdDescriptor {
        name,
        key,
        media_type,
        item_types,
    }
}

/// Returns the registered external identifiers supported by a persisted item
/// type, ordered deterministically by provider name and key.
#[must_use]
pub fn external_id_infos(item_type: &str) -> Vec<ExternalIdInfo> {
    let mut infos = DESCRIPTORS
        .iter()
        .filter(|descriptor| descriptor.item_types.contains(&item_type))
        .map(|descriptor| ExternalIdInfo {
            name: descriptor.name.to_owned(),
            key: descriptor.key.to_owned(),
            media_type: descriptor.media_type,
        })
        .collect::<Vec<_>>();
    infos.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.key.cmp(&right.key))
            .then_with(|| left.media_type.cmp(&right.media_type))
    });
    infos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_registry_exposes_real_supported_provider_contracts() {
        assert_eq!(
            external_id_infos("Movie"),
            vec![
                ExternalIdInfo {
                    name: "IMDb".to_owned(),
                    key: "Imdb".to_owned(),
                    media_type: None,
                },
                ExternalIdInfo {
                    name: "TheMovieDb".to_owned(),
                    key: "Tmdb".to_owned(),
                    media_type: Some(ExternalIdMediaType::Movie),
                },
                ExternalIdInfo {
                    name: "TheMovieDb".to_owned(),
                    key: "TmdbCollection".to_owned(),
                    media_type: Some(ExternalIdMediaType::BoxSet),
                },
            ]
        );
    }

    #[test]
    fn book_box_set_and_series_registries_cover_supported_providers() {
        let book_keys = external_id_infos("Book")
            .into_iter()
            .map(|info| info.key)
            .collect::<Vec<_>>();
        assert_eq!(book_keys, ["ComicVine", "GoogleBooks", "ISBN"]);

        let box_set = external_id_infos("BoxSet");
        assert_eq!(
            box_set,
            vec![ExternalIdInfo {
                name: "TheMovieDb".to_owned(),
                key: "TmdbCollection".to_owned(),
                media_type: Some(ExternalIdMediaType::BoxSet),
            }]
        );

        let series_keys = external_id_infos("Series")
            .into_iter()
            .map(|info| info.key)
            .collect::<Vec<_>>();
        assert!(series_keys.contains(&"Imdb".to_owned()));
        assert!(series_keys.contains(&"Tmdb".to_owned()));
        assert!(series_keys.contains(&"Tvdb".to_owned()));
        assert!(series_keys.contains(&"TvMaze".to_owned()));
        assert!(series_keys.contains(&"Tvcom".to_owned()));
        assert!(series_keys.contains(&"TvRage".to_owned()));
        assert!(series_keys.contains(&"Zap2It".to_owned()));
    }
}
