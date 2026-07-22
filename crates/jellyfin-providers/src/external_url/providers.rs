use jellyfin_model::{MetadataProvider, ProviderIdMap};

use super::encoding::{encode_component, encode_relative_path};
use super::{ExternalUrlItem, ExternalUrlItemKind};

const AUDIO_DB_BASE_URL: &str = "https://www.theaudiodb.com/";
const COMIC_VINE_BASE_URL: &str = "https://comicvine.gamespot.com/";
const GOOGLE_BOOKS_URL: &str = "https://books.google.com/books?id=";
const IMDB_BASE_URL: &str = "https://www.imdb.com/";
const MUSIC_BRAINZ_DEFAULT_SERVER: &str = "https://musicbrainz.org";
const TMDB_BASE_URL: &str = "https://www.themoviedb.org/";
const WORLDCAT_ISBN_URL: &str = "https://search.worldcat.org/search?q=bn:";
const ZAP2IT_URL: &str = "http://tvlistings.zap2it.com/overview.html?programSeriesId=";

/// Produces related external URLs for one item projection.
pub trait ExternalUrlProvider {
    fn name(&self) -> &'static str;
    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioDbAlbumExternalUrlProvider;

impl ExternalUrlProvider for AudioDbAlbumExternalUrlProvider {
    fn name(&self) -> &'static str {
        "TheAudioDb Album"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        supported_url(
            item,
            &[ExternalUrlItemKind::MusicAlbum],
            MetadataProvider::AudioDbAlbum.as_str(),
            &format!("{AUDIO_DB_BASE_URL}album/"),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioDbArtistExternalUrlProvider;

impl ExternalUrlProvider for AudioDbArtistExternalUrlProvider {
    fn name(&self) -> &'static str {
        "TheAudioDb Artist"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        supported_url(
            item,
            &[
                ExternalUrlItemKind::MusicArtist,
                ExternalUrlItemKind::Person,
            ],
            MetadataProvider::AudioDbArtist.as_str(),
            &format!("{AUDIO_DB_BASE_URL}artist/"),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ComicVineExternalUrlProvider;

impl ExternalUrlProvider for ComicVineExternalUrlProvider {
    fn name(&self) -> &'static str {
        "Comic Vine"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        if !matches!(
            item.kind,
            ExternalUrlItemKind::Person | ExternalUrlItemKind::Book
        ) {
            return Vec::new();
        }
        one(provider_id(&item.provider_ids, "ComicVine")
            .map(|id| format!("{COMIC_VINE_BASE_URL}{}", encode_relative_path(id))))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GoogleBooksExternalUrlProvider;

impl ExternalUrlProvider for GoogleBooksExternalUrlProvider {
    fn name(&self) -> &'static str {
        "Google Books"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        supported_url(
            item,
            &[ExternalUrlItemKind::Book],
            "GoogleBooks",
            GOOGLE_BOOKS_URL,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IsbnExternalUrlProvider;

impl ExternalUrlProvider for IsbnExternalUrlProvider {
    fn name(&self) -> &'static str {
        "ISBN"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        supported_url(
            item,
            &[ExternalUrlItemKind::Book],
            "ISBN",
            WORLDCAT_ISBN_URL,
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ImdbExternalUrlProvider;

impl ExternalUrlProvider for ImdbExternalUrlProvider {
    fn name(&self) -> &'static str {
        "IMDb"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        if item.kind == ExternalUrlItemKind::Season {
            return one(
                provider_id(&item.series_provider_ids, MetadataProvider::Imdb.as_str())
                    .zip(item.index_number)
                    .map(|(id, season)| {
                        format!(
                            "{IMDB_BASE_URL}title/{}/episodes/?season={season}",
                            encode_component(id)
                        )
                    }),
            );
        }
        let resource = if item.kind == ExternalUrlItemKind::Person {
            "name"
        } else {
            "title"
        };
        one(
            provider_id(&item.provider_ids, MetadataProvider::Imdb.as_str())
                .map(|id| format!("{IMDB_BASE_URL}{resource}/{}", encode_component(id))),
        )
    }
}

macro_rules! music_brainz_provider {
    ($type:ident, $name:literal, $provider:ident, $resource:literal, [$($kind:ident),+]) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $type {
            server: String,
        }

        impl $type {
            #[must_use]
            pub fn new(server: impl Into<String>) -> Self {
                Self { server: server.into().trim_end_matches('/').to_owned() }
            }
        }

        impl Default for $type {
            fn default() -> Self {
                Self::new(MUSIC_BRAINZ_DEFAULT_SERVER)
            }
        }

        impl ExternalUrlProvider for $type {
            fn name(&self) -> &'static str {
                $name
            }

            fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
                supported_url(
                    item,
                    &[$(ExternalUrlItemKind::$kind),+],
                    MetadataProvider::$provider.as_str(),
                    &format!("{}/{}/", self.server, $resource),
                )
            }
        }
    };
}

music_brainz_provider!(
    MusicBrainzAlbumExternalUrlProvider,
    "MusicBrainz Album",
    MusicBrainzAlbum,
    "release",
    [MusicAlbum]
);
music_brainz_provider!(
    MusicBrainzAlbumArtistExternalUrlProvider,
    "MusicBrainz Album Artist",
    MusicBrainzAlbumArtist,
    "artist",
    [MusicAlbum]
);
music_brainz_provider!(
    MusicBrainzArtistExternalUrlProvider,
    "MusicBrainz Artist",
    MusicBrainzArtist,
    "artist",
    [MusicArtist, Person]
);
music_brainz_provider!(
    MusicBrainzReleaseGroupExternalUrlProvider,
    "MusicBrainz Release Group",
    MusicBrainzReleaseGroup,
    "release-group",
    [MusicAlbum]
);
music_brainz_provider!(
    MusicBrainzTrackExternalUrlProvider,
    "MusicBrainz Track",
    MusicBrainzTrack,
    "track",
    [Audio]
);

#[derive(Clone, Copy, Debug, Default)]
pub struct TmdbExternalUrlProvider;

impl ExternalUrlProvider for TmdbExternalUrlProvider {
    fn name(&self) -> &'static str {
        "TMDB"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        match item.kind {
            ExternalUrlItemKind::Series => tmdb_item_url(item, "tv"),
            ExternalUrlItemKind::Movie => tmdb_item_url(item, "movie"),
            ExternalUrlItemKind::Person => tmdb_item_url(item, "person"),
            ExternalUrlItemKind::BoxSet => tmdb_item_url(item, "collection"),
            ExternalUrlItemKind::Season => tmdb_season_url(item),
            ExternalUrlItemKind::Episode => tmdb_episode_url(item),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Zap2ItExternalUrlProvider;

impl ExternalUrlProvider for Zap2ItExternalUrlProvider {
    fn name(&self) -> &'static str {
        "Zap2It"
    }

    fn get_external_urls(&self, item: &ExternalUrlItem) -> Vec<String> {
        one(
            provider_id(&item.provider_ids, MetadataProvider::Zap2It.as_str())
                .map(|id| format!("{ZAP2IT_URL}{}", encode_component(id))),
        )
    }
}

fn tmdb_item_url(item: &ExternalUrlItem, resource: &str) -> Vec<String> {
    one(
        provider_id(&item.provider_ids, MetadataProvider::Tmdb.as_str())
            .map(|id| format!("{TMDB_BASE_URL}{resource}/{}", encode_component(id))),
    )
}

fn tmdb_season_url(item: &ExternalUrlItem) -> Vec<String> {
    one(tmdb_series_id(item)
        .zip(item.index_number)
        .filter(|_| uses_tmdb_air_date_order(item))
        .map(|(id, season)| format!("{TMDB_BASE_URL}tv/{}/season/{season}", encode_component(id))))
}

fn tmdb_episode_url(item: &ExternalUrlItem) -> Vec<String> {
    one(tmdb_series_id(item)
        .zip(item.season_index_number)
        .zip(item.index_number)
        .filter(|_| uses_tmdb_air_date_order(item))
        .map(|((id, season), episode)| {
            format!(
                "{TMDB_BASE_URL}tv/{}/season/{season}/episode/{episode}",
                encode_component(id)
            )
        }))
}

fn tmdb_series_id(item: &ExternalUrlItem) -> Option<&str> {
    provider_id(&item.series_provider_ids, MetadataProvider::Tmdb.as_str())
}

fn uses_tmdb_air_date_order(item: &ExternalUrlItem) -> bool {
    item.series_display_order
        .as_deref()
        .is_none_or(|order| order.is_empty() || order == "OriginalAirDate")
}

fn supported_url(
    item: &ExternalUrlItem,
    supported_kinds: &[ExternalUrlItemKind],
    provider: &str,
    prefix: &str,
) -> Vec<String> {
    if !supported_kinds.contains(&item.kind) {
        return Vec::new();
    }
    one(provider_id(&item.provider_ids, provider)
        .map(|id| format!("{prefix}{}", encode_component(id))))
}

fn provider_id<'a>(provider_ids: &'a ProviderIdMap, provider: &str) -> Option<&'a str> {
    provider_ids
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(provider))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn one(url: Option<String>) -> Vec<String> {
    url.into_iter().collect()
}
