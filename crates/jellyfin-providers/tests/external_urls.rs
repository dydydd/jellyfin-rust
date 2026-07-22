use jellyfin_model::MetadataProvider;
use jellyfin_providers::external_url::{
    AudioDbAlbumExternalUrlProvider, AudioDbArtistExternalUrlProvider,
    ComicVineExternalUrlProvider, ExternalUrlItem, ExternalUrlItemKind, ExternalUrlProvider,
    GoogleBooksExternalUrlProvider, ImdbExternalUrlProvider, IsbnExternalUrlProvider,
    MusicBrainzAlbumArtistExternalUrlProvider, MusicBrainzAlbumExternalUrlProvider,
    MusicBrainzArtistExternalUrlProvider, MusicBrainzReleaseGroupExternalUrlProvider,
    MusicBrainzTrackExternalUrlProvider, TmdbExternalUrlProvider, Zap2ItExternalUrlProvider,
};

const MBID: &str = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";

fn with_id(kind: ExternalUrlItemKind, provider: &str, value: &str) -> ExternalUrlItem {
    ExternalUrlItem::new(kind).with_provider_id(provider, value)
}

fn assert_only<P: ExternalUrlProvider>(provider: &P, item: &ExternalUrlItem, expected: &str) {
    assert_eq!(provider.get_external_urls(item), [expected]);
}

#[test]
fn audio_db_official_matrix() {
    let albums = AudioDbAlbumExternalUrlProvider;
    assert_only(
        &albums,
        &with_id(
            ExternalUrlItemKind::MusicAlbum,
            MetadataProvider::AudioDbAlbum.as_str(),
            "12345",
        ),
        "https://www.theaudiodb.com/album/12345",
    );
    assert!(
        albums
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicAlbum))
            .is_empty()
    );
    assert!(
        albums
            .get_external_urls(&with_id(
                ExternalUrlItemKind::MusicArtist,
                MetadataProvider::AudioDbAlbum.as_str(),
                "12345",
            ))
            .is_empty()
    );

    let artists = AudioDbArtistExternalUrlProvider;
    for kind in [
        ExternalUrlItemKind::MusicArtist,
        ExternalUrlItemKind::Person,
    ] {
        assert_only(
            &artists,
            &with_id(kind, MetadataProvider::AudioDbArtist.as_str(), "67890"),
            "https://www.theaudiodb.com/artist/67890",
        );
    }
    assert!(
        artists
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicArtist))
            .is_empty()
    );
    assert!(
        artists
            .get_external_urls(&with_id(
                ExternalUrlItemKind::MusicAlbum,
                MetadataProvider::AudioDbArtist.as_str(),
                "67890",
            ))
            .is_empty()
    );
}

#[test]
fn comic_vine_official_matrix() {
    let provider = ComicVineExternalUrlProvider;
    assert_only(
        &provider,
        &with_id(ExternalUrlItemKind::Person, "ComicVine", "person/4005-1234"),
        "https://comicvine.gamespot.com/person/4005-1234",
    );
    assert_only(
        &provider,
        &with_id(ExternalUrlItemKind::Book, "ComicVine", "issue/4000-5678"),
        "https://comicvine.gamespot.com/issue/4000-5678",
    );
    assert!(
        provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Person))
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(&with_id(
                ExternalUrlItemKind::Series,
                "ComicVine",
                "volume/4050-9999",
            ))
            .is_empty()
    );
}

#[test]
fn google_books_official_matrix() {
    let provider = GoogleBooksExternalUrlProvider;
    assert_only(
        &provider,
        &with_id(ExternalUrlItemKind::Book, "GoogleBooks", "buc0AAAAMAAJ"),
        "https://books.google.com/books?id=buc0AAAAMAAJ",
    );
    assert!(
        provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Book))
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(&with_id(
                ExternalUrlItemKind::Series,
                "GoogleBooks",
                "buc0AAAAMAAJ",
            ))
            .is_empty()
    );
}

#[test]
fn isbn_official_matrix() {
    let provider = IsbnExternalUrlProvider;
    assert_only(
        &provider,
        &with_id(ExternalUrlItemKind::Book, "ISBN", "9780306406157"),
        "https://search.worldcat.org/search?q=bn:9780306406157",
    );
    assert!(
        provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Book))
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(&with_id(
                ExternalUrlItemKind::Series,
                "ISBN",
                "9780306406157",
            ))
            .is_empty()
    );
}

#[test]
fn imdb_official_matrix() {
    let provider = ImdbExternalUrlProvider;
    for (kind, id, expected) in [
        (
            ExternalUrlItemKind::Movie,
            "tt1234567",
            "https://www.imdb.com/title/tt1234567",
        ),
        (
            ExternalUrlItemKind::Series,
            "tt7654321",
            "https://www.imdb.com/title/tt7654321",
        ),
        (
            ExternalUrlItemKind::Episode,
            "tt9999999",
            "https://www.imdb.com/title/tt9999999",
        ),
        (
            ExternalUrlItemKind::Person,
            "nm0000001",
            "https://www.imdb.com/name/nm0000001",
        ),
    ] {
        assert_only(
            &provider,
            &with_id(kind, MetadataProvider::Imdb.as_str(), id),
            expected,
        );
    }

    let season = ExternalUrlItem::new(ExternalUrlItemKind::Season)
        .with_index_number(2)
        .with_series_provider_id(MetadataProvider::Imdb.as_str(), "tt1234567");
    assert_only(
        &provider,
        &season,
        "https://www.imdb.com/title/tt1234567/episodes/?season=2",
    );
    assert!(
        provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Movie))
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(
                &ExternalUrlItem::new(ExternalUrlItemKind::Season).with_index_number(1)
            )
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(
                &ExternalUrlItem::new(ExternalUrlItemKind::Season)
                    .with_series_provider_id(MetadataProvider::Imdb.as_str(), "tt1234567")
            )
            .is_empty()
    );
}

#[test]
fn music_brainz_album_official_matrix() {
    let album = with_id(
        ExternalUrlItemKind::MusicAlbum,
        MetadataProvider::MusicBrainzAlbum.as_str(),
        MBID,
    );
    assert_only(
        &MusicBrainzAlbumExternalUrlProvider::default(),
        &album,
        &format!("https://musicbrainz.org/release/{MBID}"),
    );
    assert!(
        MusicBrainzAlbumExternalUrlProvider::default()
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicAlbum))
            .is_empty()
    );
    assert!(
        MusicBrainzAlbumExternalUrlProvider::default()
            .get_external_urls(&with_id(
                ExternalUrlItemKind::MusicArtist,
                MetadataProvider::MusicBrainzAlbum.as_str(),
                MBID,
            ))
            .is_empty()
    );

    assert_only(
        &MusicBrainzAlbumArtistExternalUrlProvider::default(),
        &with_id(
            ExternalUrlItemKind::MusicAlbum,
            MetadataProvider::MusicBrainzAlbumArtist.as_str(),
            MBID,
        ),
        &format!("https://musicbrainz.org/artist/{MBID}"),
    );
    assert!(
        MusicBrainzAlbumArtistExternalUrlProvider::default()
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicAlbum))
            .is_empty()
    );

    let release_group = MusicBrainzReleaseGroupExternalUrlProvider::default();
    assert_only(
        &release_group,
        &with_id(
            ExternalUrlItemKind::MusicAlbum,
            MetadataProvider::MusicBrainzReleaseGroup.as_str(),
            MBID,
        ),
        &format!("https://musicbrainz.org/release-group/{MBID}"),
    );
    assert!(
        release_group
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicAlbum))
            .is_empty()
    );
}

#[test]
fn music_brainz_artist_and_track_official_matrix() {
    let artist_provider = MusicBrainzArtistExternalUrlProvider::default();
    for kind in [
        ExternalUrlItemKind::MusicArtist,
        ExternalUrlItemKind::Person,
    ] {
        assert_only(
            &artist_provider,
            &with_id(kind, MetadataProvider::MusicBrainzArtist.as_str(), MBID),
            &format!("https://musicbrainz.org/artist/{MBID}"),
        );
    }
    assert!(
        artist_provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::MusicArtist))
            .is_empty()
    );
    assert!(
        artist_provider
            .get_external_urls(&with_id(
                ExternalUrlItemKind::MusicAlbum,
                MetadataProvider::MusicBrainzArtist.as_str(),
                MBID,
            ))
            .is_empty()
    );

    let tracks = MusicBrainzTrackExternalUrlProvider::default();
    assert_only(
        &tracks,
        &with_id(
            ExternalUrlItemKind::Audio,
            MetadataProvider::MusicBrainzTrack.as_str(),
            MBID,
        ),
        &format!("https://musicbrainz.org/track/{MBID}"),
    );
    assert!(
        tracks
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Audio))
            .is_empty()
    );
    assert!(
        tracks
            .get_external_urls(&with_id(
                ExternalUrlItemKind::MusicAlbum,
                MetadataProvider::MusicBrainzTrack.as_str(),
                MBID,
            ))
            .is_empty()
    );
}

#[test]
fn tmdb_official_matrix() {
    let provider = TmdbExternalUrlProvider;
    for (kind, id, expected) in [
        (
            ExternalUrlItemKind::Series,
            "1399",
            "https://www.themoviedb.org/tv/1399",
        ),
        (
            ExternalUrlItemKind::Movie,
            "550",
            "https://www.themoviedb.org/movie/550",
        ),
        (
            ExternalUrlItemKind::Person,
            "6384",
            "https://www.themoviedb.org/person/6384",
        ),
        (
            ExternalUrlItemKind::BoxSet,
            "10",
            "https://www.themoviedb.org/collection/10",
        ),
    ] {
        assert_only(
            &provider,
            &with_id(kind, MetadataProvider::Tmdb.as_str(), id),
            expected,
        );
        assert!(
            provider
                .get_external_urls(&ExternalUrlItem::new(kind))
                .is_empty()
        );
    }

    let season = ExternalUrlItem::new(ExternalUrlItemKind::Season)
        .with_index_number(3)
        .with_series_provider_id(MetadataProvider::Tmdb.as_str(), "1399");
    assert_only(
        &provider,
        &season,
        "https://www.themoviedb.org/tv/1399/season/3",
    );
    assert!(
        provider
            .get_external_urls(
                &ExternalUrlItem::new(ExternalUrlItemKind::Season).with_index_number(1)
            )
            .is_empty()
    );
    assert!(
        provider
            .get_external_urls(
                &ExternalUrlItem::new(ExternalUrlItemKind::Season)
                    .with_series_provider_id(MetadataProvider::Tmdb.as_str(), "1399")
            )
            .is_empty()
    );

    let episode = ExternalUrlItem::new(ExternalUrlItemKind::Episode)
        .with_index_number(5)
        .with_season_index_number(2)
        .with_series_provider_id(MetadataProvider::Tmdb.as_str(), "1399");
    assert_only(
        &provider,
        &episode,
        "https://www.themoviedb.org/tv/1399/season/2/episode/5",
    );
    assert!(
        provider
            .get_external_urls(
                &ExternalUrlItem::new(ExternalUrlItemKind::Episode)
                    .with_index_number(1)
                    .with_season_index_number(1)
            )
            .is_empty()
    );
}

#[test]
fn zap2it_official_matrix() {
    let provider = Zap2ItExternalUrlProvider;
    assert_only(
        &provider,
        &with_id(
            ExternalUrlItemKind::Series,
            MetadataProvider::Zap2It.as_str(),
            "EP012345678901",
        ),
        "http://tvlistings.zap2it.com/overview.html?programSeriesId=EP012345678901",
    );
    assert!(
        provider
            .get_external_urls(&ExternalUrlItem::new(ExternalUrlItemKind::Series))
            .is_empty()
    );
}

#[test]
fn external_ids_are_encoded_and_blank_values_are_ignored() {
    assert_only(
        &GoogleBooksExternalUrlProvider,
        &with_id(ExternalUrlItemKind::Book, "googlebooks", "a b&c#d"),
        "https://books.google.com/books?id=a%20b%26c%23d",
    );
    assert_only(
        &ComicVineExternalUrlProvider,
        &with_id(ExternalUrlItemKind::Book, "comicvine", "issue/4000 1?#"),
        "https://comicvine.gamespot.com/issue/4000%201%3F%23",
    );
    assert_only(
        &AudioDbAlbumExternalUrlProvider,
        &with_id(
            ExternalUrlItemKind::MusicAlbum,
            MetadataProvider::AudioDbAlbum.as_str(),
            "12/34",
        ),
        "https://www.theaudiodb.com/album/12%2F34",
    );
    assert!(
        Zap2ItExternalUrlProvider
            .get_external_urls(&with_id(
                ExternalUrlItemKind::Series,
                MetadataProvider::Zap2It.as_str(),
                "  ",
            ))
            .is_empty()
    );
}

#[test]
fn music_brainz_custom_server_and_tmdb_order_match_configuration() {
    let provider = MusicBrainzTrackExternalUrlProvider::new("https://mirror.example/");
    assert_only(
        &provider,
        &with_id(
            ExternalUrlItemKind::Audio,
            MetadataProvider::MusicBrainzTrack.as_str(),
            MBID,
        ),
        &format!("https://mirror.example/track/{MBID}"),
    );

    let provider = TmdbExternalUrlProvider;
    let original_air_date = ExternalUrlItem::new(ExternalUrlItemKind::Season)
        .with_index_number(1)
        .with_series_provider_id(MetadataProvider::Tmdb.as_str(), "1399")
        .with_series_display_order("OriginalAirDate");
    assert_eq!(provider.get_external_urls(&original_air_date).len(), 1);
    let absolute = original_air_date.with_series_display_order("Absolute");
    assert!(provider.get_external_urls(&absolute).is_empty());
}

#[test]
fn provider_names_match_jellyfin() {
    let providers: [(&dyn ExternalUrlProvider, &str); 8] = [
        (&AudioDbAlbumExternalUrlProvider, "TheAudioDb Album"),
        (&ComicVineExternalUrlProvider, "Comic Vine"),
        (&GoogleBooksExternalUrlProvider, "Google Books"),
        (&ImdbExternalUrlProvider, "IMDb"),
        (&IsbnExternalUrlProvider, "ISBN"),
        (
            &MusicBrainzAlbumExternalUrlProvider::default(),
            "MusicBrainz Album",
        ),
        (&TmdbExternalUrlProvider, "TMDB"),
        (&Zap2ItExternalUrlProvider, "Zap2It"),
    ];
    for (provider, expected) in providers {
        assert_eq!(provider.name(), expected);
    }
}
