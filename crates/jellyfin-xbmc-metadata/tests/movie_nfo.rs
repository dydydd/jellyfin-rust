use chrono::{NaiveDate, NaiveDateTime};
use jellyfin_model::MetadataProvider;
use jellyfin_xbmc_metadata::{
    ImageType, NfoParseError, PersonKind, Video3dFormat, parse_movie_nfo,
};

const JUSTICE_LEAGUE: &str = include_str!("fixtures/Justice League.nfo");
const LILO_AND_STITCH: &str = include_str!("fixtures/Lilo & Stitch.nfo");
const COMMUNITY_RATING: &str = include_str!("fixtures/CommunityRating.nfo");
const COMMUNITY_RATING_COMMA: &str = include_str!("fixtures/CommunityRating_Comma.nfo");
const COMMUNITY_RATING_OUT_OF_RANGE: &str = include_str!("fixtures/CommunityRating_OutOfRange.nfo");
const TMDB_URL: &str = include_str!("fixtures/Tmdb.nfo");
const IMDB_URL: &str = include_str!("fixtures/Imdb.nfo");
const RADARR_URLS: &str = include_str!("fixtures/Radarr.nfo");
const FANART: &str = include_str!("fixtures/Fanart.nfo");

#[test]
fn parses_official_movie_fixture_core_fields() {
    let movie = parse_movie_nfo(JUSTICE_LEAGUE).unwrap();
    assert_eq!(movie.name.as_deref(), Some("Justice League"));
    assert_eq!(movie.original_title.as_deref(), Some("Justice League"));
    assert_eq!(movie.tagline.as_deref(), Some("Justice for all."));
    assert_eq!(
        movie
            .provider_ids
            .get(MetadataProvider::Imdb.as_str())
            .map(String::as_str),
        Some("tt0974015")
    );
    assert_eq!(
        movie
            .provider_ids
            .get(MetadataProvider::Tmdb.as_str())
            .map(String::as_str),
        Some("141052")
    );
    assert_eq!(movie.genres, ["Action", "Adventure", "Fantasy", "Sci-Fi"]);
    assert_eq!(movie.studios, ["DC Comics"]);
    assert_eq!(movie.premiere_date, NaiveDate::from_ymd_opt(2017, 11, 15));
    assert_eq!(movie.end_date, NaiveDate::from_ymd_opt(2017, 11, 16));
    assert_eq!(
        movie.date_created,
        NaiveDateTime::parse_from_str("2019-08-06 09:01:18", "%Y-%m-%d %H:%M:%S").ok()
    );
    assert_eq!(movie.production_year, Some(2017));
    assert_eq!(movie.aspect_ratio.as_deref(), Some("1.777778"));
    assert_eq!(movie.width, Some(1920));
    assert_eq!(movie.height, Some(1080));
    assert_eq!(movie.runtime_ticks, Some(62_680_000_000));
    assert!(movie.has_subtitles);
    assert_eq!(movie.video_3d_format, Some(Video3dFormat::HalfSideBySide));
    assert_eq!(movie.critic_rating, Some(7.6));
    assert_eq!(movie.custom_rating.as_deref(), Some("8.7"));
    assert_eq!(movie.preferred_metadata_language.as_deref(), Some("en"));
    assert_eq!(movie.preferred_metadata_country_code.as_deref(), Some("us"));
    assert_eq!(
        movie.remote_trailers,
        ["https://www.youtube.com/watch?v=dQw4w9WgXcQ"]
    );
}

#[test]
fn parses_people_collection_and_user_fields() {
    let movie = parse_movie_nfo(JUSTICE_LEAGUE).unwrap();
    assert_eq!(movie.people.len(), 20);
    assert_eq!(
        movie
            .people
            .iter()
            .filter(|person| person.kind == PersonKind::Writer)
            .count(),
        3
    );
    assert_eq!(
        movie
            .people
            .iter()
            .filter(|person| person.kind == PersonKind::Director)
            .count(),
        1
    );
    assert_eq!(
        movie
            .people
            .iter()
            .filter(|person| person.kind == PersonKind::Actor)
            .count(),
        15
    );
    let aquaman = movie
        .people
        .iter()
        .find(|person| person.role == "Aquaman")
        .unwrap();
    assert_eq!(aquaman.name, "Jason Momoa");
    assert_eq!(aquaman.sort_order, Some(5));
    assert!(
        aquaman
            .image_url
            .as_deref()
            .unwrap()
            .contains("MV5BMTI5MTU5")
    );
    assert!(
        movie.people.iter().any(|person| {
            person.name == "Test Lyricist" && person.kind == PersonKind::Lyricist
        })
    );
    assert_eq!(
        movie.collection_name.as_deref(),
        Some("Justice League Collection")
    );
    assert_eq!(
        movie
            .provider_ids
            .get(MetadataProvider::TmdbCollection.as_str())
            .map(String::as_str),
        Some("702342")
    );
    assert_eq!(movie.user_data.play_count, Some(2));
    assert_eq!(movie.user_data.played, Some(true));
    assert_eq!(
        movie.user_data.last_played_date,
        NaiveDateTime::parse_from_str("2021-02-11 07:47:23", "%Y-%m-%d %H:%M:%S").ok()
    );
}

#[test]
fn keeps_only_first_remote_image_of_each_type() {
    let movie = parse_movie_nfo(JUSTICE_LEAGUE).unwrap();
    assert_eq!(movie.remote_images.len(), 7);
    for image_type in [
        ImageType::Primary,
        ImageType::Logo,
        ImageType::Banner,
        ImageType::Thumb,
        ImageType::Art,
        ImageType::Disc,
        ImageType::Backdrop,
    ] {
        assert_eq!(
            movie
                .remote_images
                .iter()
                .filter(|image| image.image_type == image_type)
                .count(),
            1
        );
    }
    let backdrop = movie
        .remote_images
        .iter()
        .find(|image| image.image_type == ImageType::Backdrop)
        .unwrap();
    assert!(
        backdrop
            .url
            .ends_with("moviebackground/justice-league-5793f518c6d6e.jpg")
    );

    let fanart = parse_movie_nfo(FANART).unwrap();
    assert_eq!(
        fanart
            .remote_images
            .iter()
            .filter(|image| image.image_type == ImageType::Backdrop)
            .count(),
        1
    );
}

#[test]
fn provider_only_url_fixtures_are_supported() {
    for (input, provider, expected) in [
        (TMDB_URL, MetadataProvider::Tmdb, "30287"),
        (IMDB_URL, MetadataProvider::Imdb, "tt0944947"),
    ] {
        let movie = parse_movie_nfo(input).unwrap();
        assert_eq!(
            movie
                .provider_ids
                .get(provider.as_str())
                .map(String::as_str),
            Some(expected)
        );
    }

    let movie = parse_movie_nfo(RADARR_URLS).unwrap();
    assert_eq!(
        movie.provider_ids.get("Tmdb").map(String::as_str),
        Some("583689")
    );
    assert_eq!(
        movie.provider_ids.get("Imdb").map(String::as_str),
        Some("tt4154796")
    );
}

#[test]
fn escaped_xml_and_tmdb_collection_id_are_normalized() {
    let movie = parse_movie_nfo(LILO_AND_STITCH).unwrap();
    assert_eq!(movie.name.as_deref(), Some("Lilo & Stitch"));
    assert_eq!(movie.original_title.as_deref(), Some("Lilo & Stitch"));
    assert_eq!(
        movie.collection_name.as_deref(),
        Some("Lilo & Stitch Collection")
    );
    assert!(movie.overview.as_deref().unwrap().starts_with(">>"));
    assert!(movie.overview.as_deref().unwrap().ends_with("<<"));
    assert_eq!(
        movie.provider_ids.get("TmdbCollection").map(String::as_str),
        Some("97020")
    );
    assert!(!movie.provider_ids.contains_key("tmdbcol"));
}

#[test]
fn community_rating_matrix_matches_official_behavior() {
    assert_eq!(
        parse_movie_nfo(COMMUNITY_RATING).unwrap().community_rating,
        Some(7.5)
    );
    assert_eq!(
        parse_movie_nfo(COMMUNITY_RATING_COMMA)
            .unwrap()
            .community_rating,
        Some(7.5)
    );
    assert_eq!(
        parse_movie_nfo(COMMUNITY_RATING_OUT_OF_RANGE)
            .unwrap()
            .community_rating,
        None
    );
}

#[test]
fn malformed_xml_and_wrong_root_report_typed_errors() {
    assert!(matches!(
        parse_movie_nfo("<movie><title>broken</movie>"),
        Err(NfoParseError::Xml(_))
    ));
    assert!(matches!(
        parse_movie_nfo("<tvshow><title>Show</title></tvshow>"),
        Err(NfoParseError::UnexpectedRoot(root)) if root == "tvshow"
    ));
}
