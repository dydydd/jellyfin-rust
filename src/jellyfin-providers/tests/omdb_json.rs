use jellyfin_providers::omdb::{JsonOmdbConverter, OmdbDate, OmdbJsonError};

const OFFICIAL_RESPONSE: &str = include_str!("data/omdb_chapter_1.json");

#[test]
fn deserializes_official_not_available_response() {
    let item = JsonOmdbConverter::deserialize_item(OFFICIAL_RESPONSE).unwrap();
    assert_eq!(item.title.as_deref(), Some("Chapter 1"));
    assert_eq!(item.awards, None);
    assert_eq!(item.season, None);
    assert_eq!(item.episode, None);
    assert_eq!(item.metascore, None);
    assert_eq!(item.series_id, None);
    assert_eq!(item.imdb_id.as_deref(), Some("tt2161930"));
    assert_eq!(item.ratings.as_ref().unwrap().len(), 1);
}

#[test]
fn roundtrips_official_response() {
    let first = JsonOmdbConverter::deserialize_item(OFFICIAL_RESPONSE).unwrap();
    let serialized = JsonOmdbConverter::serialize_item(&first).unwrap();
    let second = JsonOmdbConverter::deserialize_item(&serialized).unwrap();
    assert_eq!(first, second);
    assert_eq!(second.awards, None);
    assert_eq!(second.episode, None);
    assert_eq!(second.metascore, None);
}

#[test]
fn nullable_string_matrix_matches_official_converter() {
    for input in ["\"N/A\"", "\"n/a\"", "null"] {
        assert_eq!(
            JsonOmdbConverter::deserialize_nullable_string(input).unwrap(),
            None
        );
    }
    for (input, expected) in [
        ("\"Jellyfin\"", "Jellyfin"),
        ("\"\"", ""),
        ("\" N/A \"", " N/A "),
    ] {
        assert_eq!(
            JsonOmdbConverter::deserialize_nullable_string(input)
                .unwrap()
                .as_deref(),
            Some(expected)
        );
    }
    for input in ["8", "true", "[]", "{}"] {
        assert!(JsonOmdbConverter::deserialize_nullable_string(input).is_err());
    }
}

#[test]
fn nullable_integer_matrix_accepts_numbers_and_numeric_strings() {
    for input in ["\"N/A\"", "\"n/a\"", "null"] {
        assert_eq!(
            JsonOmdbConverter::deserialize_nullable_i32(input).unwrap(),
            None
        );
    }
    for (input, expected) in [
        ("\"8\"", 8),
        ("8", 8),
        ("\" 8 \"", 8),
        ("\"-2\"", -2),
        ("0", 0),
    ] {
        assert_eq!(
            JsonOmdbConverter::deserialize_nullable_i32(input).unwrap(),
            Some(expected)
        );
    }
    for input in ["\"eight\"", "8.5", "2147483648", "true", "[]", "{}"] {
        assert!(JsonOmdbConverter::deserialize_nullable_i32(input).is_err());
    }
}

#[test]
fn list_like_text_fields_accept_strings_and_arrays() {
    let item = JsonOmdbConverter::deserialize_item(
        r#"{
            "Genre":["Drama","Thriller"],
            "Director":"David Fincher",
            "Writer":["Writer One","N/A",null,"Writer Two"],
            "Actors":["Kevin Spacey","Robin Wright"],
            "Language":["English","French"],
            "Country":"USA"
        }"#,
    )
    .unwrap();
    assert_eq!(item.genre.as_deref(), Some("Drama, Thriller"));
    assert_eq!(item.director.as_deref(), Some("David Fincher"));
    assert_eq!(item.writer.as_deref(), Some("Writer One, Writer Two"));
    assert_eq!(item.actors.as_deref(), Some("Kevin Spacey, Robin Wright"));
    assert_eq!(item.language.as_deref(), Some("English, French"));
    assert_eq!(item.country.as_deref(), Some("USA"));

    let empty = JsonOmdbConverter::deserialize_item(r#"{"Genre":[]}"#).unwrap();
    assert_eq!(empty.genre, None);
}

#[test]
fn parses_year_dates_and_runtime() {
    let item = JsonOmdbConverter::deserialize_item(
        r#"{
            "Year":"2013-2018",
            "Released":"01 Feb 2013",
            "DVD":"29 Feb 2020",
            "Runtime":"55 min"
        }"#,
    )
    .unwrap();
    assert_eq!(item.production_year(), Some(2013));
    assert_eq!(
        item.release_date(),
        Some(OmdbDate {
            year: 2013,
            month: 2,
            day: 1,
        })
    );
    assert_eq!(
        item.dvd_release_date(),
        Some(OmdbDate {
            year: 2020,
            month: 2,
            day: 29,
        })
    );
    assert_eq!(item.runtime_minutes(), Some(55));

    for input in [
        r#"{"Year":"12","Released":"N/A","Runtime":"N/A"}"#,
        r#"{"Year":"abcd","Released":"29 Feb 2019","Runtime":"1 hour"}"#,
        r#"{"Released":"32 Jan 2020","Runtime":"-1 min"}"#,
    ] {
        let item = JsonOmdbConverter::deserialize_item(input).unwrap();
        assert_eq!(item.production_year(), None);
        assert_eq!(item.release_date(), None);
        assert_eq!(item.runtime_minutes(), None);
    }
}

#[test]
fn parses_rating_and_vote_semantics() {
    let item = JsonOmdbConverter::deserialize_item(
        r#"{
            "Ratings":[
                {"Source":"Internet Movie Database","Value":"8.7/10"},
                {"Source":"rotten tomatoes","Value":"94%"},
                {"Source":"Metacritic","Value":"76/100"}
            ],
            "Metascore":"76",
            "imdbRating":"8.7",
            "imdbVotes":"6,736"
        }"#,
    )
    .unwrap();
    assert!((item.rotten_tomatoes_score().unwrap() - 94.0).abs() < f32::EPSILON);
    assert!((item.imdb_score().unwrap() - 8.7).abs() < f32::EPSILON);
    assert!((item.metascore().unwrap() - 76.0).abs() < f32::EPSILON);
    assert_eq!(item.vote_count(), Some(6_736));

    let unavailable = JsonOmdbConverter::deserialize_item(
        r#"{"Ratings":"N/A","Metascore":"N/A","imdbRating":"-1","imdbVotes":"N/A"}"#,
    )
    .unwrap();
    assert_eq!(unavailable.ratings, None);
    assert_eq!(unavailable.metascore(), None);
    assert_eq!(unavailable.imdb_score(), None);
    assert_eq!(unavailable.vote_count(), None);
}

#[test]
fn parses_season_arrays_and_not_available_values() {
    let season = JsonOmdbConverter::deserialize_season(
        r#"{
            "Title":"House of Cards",
            "seriesID":"tt1856010",
            "Season":"1",
            "totalSeasons":6,
            "Episodes":[
                {"Title":"Chapter 1","Episode":"1","imdbID":"tt2161930"},
                {"Title":"Chapter 2","Episode":2,"imdbID":"tt2161931"}
            ],
            "Response":"True"
        }"#,
    )
    .unwrap();
    assert_eq!(season.season, Some(1));
    assert_eq!(season.total_seasons, Some(6));
    assert_eq!(season.episodes.as_ref().unwrap().len(), 2);
    assert_eq!(season.episodes.as_ref().unwrap()[1].episode, Some(2));

    let serialized = JsonOmdbConverter::serialize_season(&season).unwrap();
    assert_eq!(
        JsonOmdbConverter::deserialize_season(&serialized).unwrap(),
        season
    );

    for episodes in ["null", "\"N/A\""] {
        let input = format!(r#"{{"Season":"N/A","Episodes":{episodes}}}"#);
        let unavailable = JsonOmdbConverter::deserialize_season(&input).unwrap();
        assert_eq!(unavailable.season, None);
        assert_eq!(unavailable.episodes, None);
    }
}

#[test]
fn preserves_error_responses_and_ignores_unknown_fields() {
    let item = JsonOmdbConverter::deserialize_item(
        r#"{"Response":"False","Error":"Movie not found!","Unknown":42}"#,
    )
    .unwrap();
    assert_eq!(item.response.as_deref(), Some("False"));
    assert_eq!(item.error.as_deref(), Some("Movie not found!"));
}

#[test]
fn rejects_malformed_json_and_incompatible_field_types() {
    for input in ["", "{", "[]", "null", "true", "42"] {
        assert!(JsonOmdbConverter::deserialize_item(input).is_err());
    }
    for input in [
        r#"{"Title":42}"#,
        r#"{"Episode":"one"}"#,
        r#"{"Episode":1.5}"#,
        r#"{"Ratings":{}}"#,
        r#"{"Ratings":["94%"]}"#,
        r#"{"Actors":["A",42]}"#,
    ] {
        assert!(JsonOmdbConverter::deserialize_item(input).is_err());
    }

    let error = JsonOmdbConverter::deserialize_item(r#"{"Episode":"one"}"#).unwrap_err();
    assert!(matches!(error, OmdbJsonError::InvalidInteger { .. }));
}
