use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime, Weekday};
use jellyfin_xbmc_metadata::{
    ImageType, MovieNfoLocation, MovieVideoType, NfoDocumentKind, NfoFetchError, NfoMetadata,
    PersonKind, SeriesStatus, fetch_nfo_file, movie_nfo_save_paths,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn parse_fixture(name: &str, kind: NfoDocumentKind) -> NfoMetadata {
    let mut metadata = NfoMetadata::default();
    fetch_nfo_file(kind, Some(&mut metadata), fixture(name)).expect("fixture should parse");
    metadata
}

#[test]
fn movie_mixed_folder_success() {
    let paths = movie_nfo_save_paths(&MovieNfoLocation {
        path: "/media/movies/Avengers Endgame.mp4".into(),
        is_in_mixed_folder: true,
        video_type: MovieVideoType::File,
    });
    assert_eq!(paths, [PathBuf::from("/media/movies/Avengers Endgame.nfo")]);
}

#[test]
fn movie_separate_folder_success() {
    let paths = movie_nfo_save_paths(&MovieNfoLocation {
        path: "/media/movies/Avengers Endgame/Avengers Endgame.mp4".into(),
        is_in_mixed_folder: false,
        video_type: MovieVideoType::File,
    });
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&PathBuf::from(
        "/media/movies/Avengers Endgame/Avengers Endgame.nfo"
    )));
    assert!(paths.contains(&PathBuf::from("/media/movies/Avengers Endgame/movie.nfo")));
}

#[test]
fn movie_separate_folder_preserves_windows_separators() {
    let paths = movie_nfo_save_paths(&MovieNfoLocation {
        path: r"C:\media\movies\Avengers Endgame\Avengers Endgame.mp4".into(),
        is_in_mixed_folder: false,
        video_type: MovieVideoType::File,
    });
    assert_eq!(
        paths,
        [
            PathBuf::from(r"C:\media\movies\Avengers Endgame\Avengers Endgame.nfo"),
            PathBuf::from(r"C:\media\movies\Avengers Endgame\movie.nfo"),
        ]
    );
}

#[test]
fn movie_dvd_success() {
    let paths = movie_nfo_save_paths(&MovieNfoLocation {
        path: "/media/movies/Avengers Endgame".into(),
        is_in_mixed_folder: false,
        video_type: MovieVideoType::Dvd,
    });
    assert_eq!(paths.len(), 3);
    assert!(paths.contains(&PathBuf::from(
        "/media/movies/Avengers Endgame/Avengers Endgame.nfo"
    )));
    assert!(paths.contains(&PathBuf::from(
        "/media/movies/Avengers Endgame/VIDEO_TS/VIDEO_TS.nfo"
    )));
    assert!(paths.contains(&PathBuf::from("/media/movies/Avengers Endgame/movie.nfo")));
}

#[test]
fn episode_fetch_valid_success() {
    let item = parse_fixture("The Bone Orchard.nfo", NfoDocumentKind::Episode);
    assert_eq!(item.name.as_deref(), Some("The Bone Orchard"));
    assert_eq!(item.series_name.as_deref(), Some("American Gods"));
    assert_eq!(item.index_number, Some(1));
    assert_eq!(item.index_number_end, Some(1));
    assert_eq!(item.parent_index_number, Some(1));
    assert_eq!(
        item.overview.as_deref(),
        Some(
            "When Shadow Moon is released from prison early after the death of his wife, he meets Mr. Wednesday and is recruited as his bodyguard. Shadow discovers that this may be more than he bargained for."
        )
    );
    assert_eq!(item.runtime_ticks, 0);
    assert_eq!(item.official_rating.as_deref(), Some("16"));
    assert_eq!(item.genres, ["Drama", "Mystery", "Sci-Fi & Fantasy"]);
    assert_eq!(item.premiere_date, NaiveDate::from_ymd_opt(2017, 4, 30));
    assert_eq!(item.production_year, Some(2017));
    assert_eq!(item.studios, ["Starz"]);
    assert_eq!(item.airs_after_season_number, Some(2));
    assert_eq!(item.airs_before_season_number, Some(3));
    assert_eq!(item.airs_before_episode_number, Some(1));
    assert_eq!(
        item.provider_ids.get("Imdb").map(String::as_str),
        Some("tt5017734")
    );
    assert_eq!(
        item.provider_ids.get("Tmdb").map(String::as_str),
        Some("1276153")
    );

    let writers: Vec<_> = item
        .people
        .iter()
        .filter(|person| person.kind == PersonKind::Writer)
        .collect();
    assert_eq!(writers.len(), 2);
    assert!(writers.iter().any(|person| person.name == "Bryan Fuller"));
    assert!(writers.iter().any(|person| person.name == "Michael Green"));
    let directors: Vec<_> = item
        .people
        .iter()
        .filter(|person| person.kind == PersonKind::Director)
        .collect();
    assert_eq!(directors.len(), 1);
    assert_eq!(directors[0].name, "David Slade");
    let actors: Vec<_> = item
        .people
        .iter()
        .filter(|person| person.kind == PersonKind::Actor)
        .collect();
    assert_eq!(actors.len(), 11);
    let shadow = actors
        .iter()
        .find(|person| person.role == "Shadow Moon")
        .expect("Shadow Moon should exist");
    assert_eq!(shadow.name, "Ricky Whittle");
    assert_eq!(shadow.sort_order, Some(0));
    assert_eq!(
        shadow.image_url.as_deref(),
        Some("http://image.tmdb.org/t/p/original/cjeDbVfBp6Qvb3C74Dfy7BKDTQN.jpg")
    );
    assert_eq!(
        item.date_created,
        NaiveDateTime::parse_from_str("2017-10-07 14:25:47", "%Y-%m-%d %H:%M:%S").ok()
    );
}

#[test]
fn episode_fetch_valid_multi_episode_success() {
    let item = parse_fixture("Rising.nfo", NfoDocumentKind::Episode);
    assert_eq!(item.name.as_deref(), Some("Rising (1) / Rising (2)"));
    assert_eq!(item.index_number, Some(1));
    assert_eq!(item.index_number_end, Some(2));
    assert_eq!(item.parent_index_number, Some(1));
    assert_eq!(
        item.overview.as_deref(),
        Some(
            "A new Stargate team embarks on a dangerous mission to a distant galaxy, where they discover a mythical lost city -- and a deadly new enemy. / Sheppard tries to convince Weir to mount a rescue mission to free Colonel Sumner, Teyla, and the others captured by the Wraith."
        )
    );
    assert_eq!(item.premiere_date, NaiveDate::from_ymd_opt(2004, 7, 16));
    assert_eq!(item.production_year, Some(2004));
}

#[test]
fn episode_fetch_valid_multi_episode_with_missing_tags_success() {
    let item = parse_fixture("Stargate Atlantis S01E01-E04.nfo", NfoDocumentKind::Episode);
    assert_eq!(
        item.name.as_deref(),
        Some("Rising / Hide and Seek / Thirty-Eight Minutes")
    );
    assert_eq!(
        item.original_title.as_deref(),
        Some("Rising (1) / Rising (2) / Hide and Seek / Thirty-Eight Minutes")
    );
    assert_eq!(item.index_number, Some(1));
    assert_eq!(item.index_number_end, Some(4));
    assert_eq!(item.parent_index_number, Some(1));
    assert_eq!(
        item.overview.as_deref(),
        Some(
            "A new Stargate team embarks on a dangerous mission to a distant galaxy, where they discover a mythical lost city -- and a deadly new enemy."
        )
    );
    assert_eq!(item.premiere_date, NaiveDate::from_ymd_opt(2004, 7, 16));
    assert_eq!(item.production_year, Some(2004));
}

#[test]
fn single_episode_without_end_tag_keeps_index_number_end_unset() {
    let episode = jellyfin_xbmc_metadata::parse_nfo(
        "<episodedetails><title>Only</title><episode>3</episode></episodedetails>",
        NfoDocumentKind::Episode,
    )
    .unwrap();
    assert_eq!(episode.index_number, Some(3));
    assert_eq!(episode.index_number_end, None);
}

#[test]
fn multi_episode_uses_maximum_episode_number_when_last_block_is_missing_one() {
    let episode = jellyfin_xbmc_metadata::parse_nfo(
        "<episodedetails><title>One</title><episode>1</episode></episodedetails>\
         <episodedetails><title>Two</title><episode>2</episode></episodedetails>\
         <episodedetails><title>Three</title></episodedetails>",
        NfoDocumentKind::Episode,
    )
    .unwrap();
    assert_eq!(episode.index_number_end, Some(2));
}

#[test]
fn episode_thumb_without_aspect_is_primary() {
    let item = parse_fixture("Sonarr-Thumb.nfo", NfoDocumentKind::Episode);
    let primary: Vec<_> = item
        .remote_images
        .iter()
        .filter(|image| image.image_type == ImageType::Primary)
        .collect();
    assert_eq!(primary.len(), 1);
    assert_eq!(
        primary[0].url,
        "https://artworks.thetvdb.com/banners/episodes/359095/7081317.jpg"
    );
}

#[test]
fn episode_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(
        NfoDocumentKind::Episode,
        None,
        fixture("The Bone Orchard.nfo"),
    )
    .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn episode_fetch_with_empty_path_returns_typed_error() {
    let mut target = NfoMetadata::default();
    let error = fetch_nfo_file(NfoDocumentKind::Episode, Some(&mut target), "")
        .expect_err("empty path should fail");
    assert!(matches!(error, NfoFetchError::EmptyPath));
}

#[test]
fn music_album_fetch_valid_success() {
    let item = parse_fixture("The Best of 1980-1990.nfo", NfoDocumentKind::MusicAlbum);
    assert_eq!(item.name.as_deref(), Some("The Best of 1980-1990"));
    assert_eq!(item.production_year, Some(1989));
    assert_eq!(item.genres, ["Pop"]);
    assert!(item.tags.iter().any(|tag| tag == "Rock/Pop"));
    assert_eq!(
        item.overview.as_deref(),
        Some(concat!(
            "The Best of 1980-1990 is the first greatest hits compilation by Irish rock band U2, released in November 1998. It mostly contains the group's hit singles from the eighties but also mixes in some live staples as well as one new recording, Sweetest Thing. In April 1999, a companion video (featuring music videos and live footage) was released. The album was followed by another compilation, The Best of 1990-2000, in 2002.\n",
            "A limited edition version containing a special B-sides disc was released on the same date as the single-disc version. At the time of release, the official word was that the 2-disc album would be available the first week the album went on sale, then pulled from the stores. While this threat never materialized, it did result in the 2-disc version being in very high demand. Both versions charted in the Billboard 200.\n",
            "The boy on the cover is Peter Rowan, brother of Bono's friend Guggi (real name Derek Rowan) of the Virgin Prunes. He also appears on the covers of the early EP Three, two of the band's first three albums (Boy and War), and Early Demos."
        ))
    );
}

#[test]
fn music_album_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(
        NfoDocumentKind::MusicAlbum,
        None,
        fixture("The Best of 1980-1990.nfo"),
    )
    .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn music_album_fetch_with_empty_path_returns_typed_error() {
    assert_empty_path(NfoDocumentKind::MusicAlbum);
}

#[test]
fn music_artist_fetch_valid_success() {
    let item = parse_fixture("U2.nfo", NfoDocumentKind::MusicArtist);
    assert_eq!(item.name.as_deref(), Some("U2"));
    assert_eq!(item.sort_name.as_deref(), Some("U2"));
    assert_eq!(
        item.provider_ids
            .get("MusicBrainzArtist")
            .map(String::as_str),
        Some("a3cb23fc-acd3-4ce0-8f36-1e5aa6a18432")
    );
    assert_eq!(item.genres, ["Rock"]);
}

#[test]
fn music_artist_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(NfoDocumentKind::MusicArtist, None, fixture("U2.nfo"))
        .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn music_artist_fetch_with_empty_path_returns_typed_error() {
    assert_empty_path(NfoDocumentKind::MusicArtist);
}

#[test]
fn music_video_fetch_valid_success() {
    let item = parse_fixture("Dancing Queen.nfo", NfoDocumentKind::MusicVideo);
    assert_eq!(item.name.as_deref(), Some("Dancing Queen"));
    assert_eq!(item.artists, ["ABBA"]);
    assert_eq!(item.album.as_deref(), Some("Arrival"));
}

#[test]
fn music_video_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(
        NfoDocumentKind::MusicVideo,
        None,
        fixture("Dancing Queen.nfo"),
    )
    .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn music_video_fetch_with_empty_path_returns_typed_error() {
    assert_empty_path(NfoDocumentKind::MusicVideo);
}

#[test]
fn season_fetch_valid_success() {
    let item = parse_fixture("Season 01.nfo", NfoDocumentKind::Season);
    assert_eq!(item.name.as_deref(), Some("Season 1"));
    assert_eq!(item.index_number, Some(1));
    assert!(!item.is_locked);
    assert_eq!(item.production_year, Some(2019));
    assert_eq!(item.premiere_date, NaiveDate::from_ymd_opt(2019, 11, 8));
    assert_eq!(
        item.date_created,
        NaiveDateTime::parse_from_str("2020-06-14 17:26:51", "%Y-%m-%d %H:%M:%S").ok()
    );
    assert_eq!(item.people.len(), 10);
    assert!(
        item.people
            .iter()
            .all(|person| person.kind == PersonKind::Actor)
    );
    let nini = item
        .people
        .iter()
        .find(|person| person.role == "Nini")
        .expect("Nini should exist");
    assert_eq!(nini.name, "Olivia Rodrigo");
    assert_eq!(nini.sort_order, Some(0));
    assert_eq!(
        nini.image_url.as_deref(),
        Some("/config/metadata/People/O/Olivia Rodrigo/poster.jpg")
    );
}

#[test]
fn season_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(NfoDocumentKind::Season, None, fixture("Season 01.nfo"))
        .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn season_fetch_with_empty_path_returns_typed_error() {
    assert_empty_path(NfoDocumentKind::Season);
}

#[test]
fn series_fetch_valid_success() {
    let item = parse_fixture("American Gods.nfo", NfoDocumentKind::Series);
    assert_eq!(item.original_title.as_deref(), Some("American Gods"));
    assert!(item.tagline.is_empty());
    assert_eq!(item.runtime_ticks, 0);
    assert_eq!(
        item.provider_ids.get("Tmdb").map(String::as_str),
        Some("46639")
    );
    assert_eq!(
        item.provider_ids.get("Tvdb").map(String::as_str),
        Some("253573")
    );
    assert_eq!(
        item.provider_ids.get("Imdb").map(String::as_str),
        Some("tt11111")
    );
    assert_eq!(item.genres, ["Drama", "Mystery", "Sci-Fi & Fantasy"]);
    assert_eq!(item.premiere_date, NaiveDate::from_ymd_opt(2017, 4, 30));
    assert_eq!(item.studios, ["Starz"]);
    assert_eq!(item.air_time.as_deref(), Some("9 PM"));
    assert_eq!(item.air_days, [Weekday::Fri]);
    assert_eq!(item.status, Some(SeriesStatus::Ended));
    assert_eq!(item.people.len(), 6);
    assert!(
        item.people
            .iter()
            .all(|person| person.kind == PersonKind::Actor)
    );
    let sweeney = item
        .people
        .iter()
        .find(|person| person.role == "Mad Sweeney")
        .expect("Mad Sweeney should exist");
    assert_eq!(sweeney.name, "Pablo Schreiber");
    assert_eq!(sweeney.sort_order, Some(3));
    assert_eq!(
        sweeney.image_url.as_deref(),
        Some("http://image.tmdb.org/t/p/original/uo8YljeePz3pbj7gvWXdB4gOOW4.jpg")
    );
    assert_eq!(
        item.date_created,
        NaiveDateTime::parse_from_str("2017-10-07 14:25:47", "%Y-%m-%d %H:%M:%S").ok()
    );
}

#[test]
fn series_parse_url_file_success() {
    let item = parse_fixture("Tvdb.nfo", NfoDocumentKind::Series);
    assert_eq!(
        item.provider_ids.get("Tvdb").map(String::as_str),
        Some("121361")
    );
}

#[test]
fn id_content_is_only_used_for_imdb_shaped_values() {
    let series = jellyfin_xbmc_metadata::parse_nfo(
        r#"<tvshow><id TMDB="123" TVDB="456">789</id></tvshow>"#,
        NfoDocumentKind::Series,
    )
    .unwrap();
    assert_eq!(
        series.provider_ids.get("Tmdb").map(String::as_str),
        Some("123")
    );
    assert_eq!(
        series.provider_ids.get("Tvdb").map(String::as_str),
        Some("456")
    );
    assert!(!series.provider_ids.contains_key("Imdb"));

    let episode = jellyfin_xbmc_metadata::parse_nfo(
        "<episodedetails><id>789</id></episodedetails>",
        NfoDocumentKind::Episode,
    )
    .unwrap();
    assert!(episode.provider_ids.is_empty());

    let imdb = jellyfin_xbmc_metadata::parse_nfo(
        "<episodedetails><id>tt1234567</id></episodedetails>",
        NfoDocumentKind::Episode,
    )
    .unwrap();
    assert_eq!(
        imdb.provider_ids.get("Imdb").map(String::as_str),
        Some("tt1234567")
    );

    let collection = jellyfin_xbmc_metadata::parse_nfo(
        r#"<tvshow><uniqueid type="tmdbcol">97020</uniqueid></tvshow>"#,
        NfoDocumentKind::Series,
    )
    .unwrap();
    assert_eq!(
        collection
            .provider_ids
            .get("TmdbCollection")
            .map(String::as_str),
        Some("97020")
    );
}

#[test]
fn series_fetch_with_missing_target_returns_typed_error() {
    let error = fetch_nfo_file(NfoDocumentKind::Series, None, fixture("American Gods.nfo"))
        .expect_err("missing target should fail");
    assert!(matches!(error, NfoFetchError::MissingTarget));
}

#[test]
fn series_fetch_with_empty_path_returns_typed_error() {
    assert_empty_path(NfoDocumentKind::Series);
}

fn assert_empty_path(kind: NfoDocumentKind) {
    let mut target = NfoMetadata::default();
    let error = fetch_nfo_file(kind, Some(&mut target), "").expect_err("empty path should fail");
    assert!(matches!(error, NfoFetchError::EmptyPath));
}
