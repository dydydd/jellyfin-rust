use std::collections::HashMap;

use jellyfin_naming::{
    EpisodeResolver, NamingOptions, SeasonPathParser, SeriesResolver, VideoResolver,
};

#[test]
fn video_resolver_preserves_name_and_extracts_provider_ids() {
    let result = VideoResolver::resolve_file(
        Some("/media/Movie (2020) [tmdb=618355][imdbid=tt10985510].mkv"),
        &NamingOptions::default(),
    )
    .unwrap();

    assert_eq!(result.name, "Movie");
    assert_eq!(result.year, Some(2020));
    assert_eq!(
        result.provider_ids,
        HashMap::from([
            ("Imdb".to_owned(), "tt10985510".to_owned()),
            ("Tmdb".to_owned(), "618355".to_owned()),
        ])
    );
}

#[test]
fn series_resolver_extracts_all_official_provider_types() {
    let result = SeriesResolver::resolve(
        &NamingOptions::default(),
        "/shows/Series [imdb=tt10985510][tvdbid=6][tvmazeid=7][tmdb=8][anidbid=9][anilistid=10][anisearchid=11]",
    );

    assert_eq!(
        result.path,
        "/shows/Series [imdb=tt10985510][tvdbid=6][tvmazeid=7][tmdb=8][anidbid=9][anilistid=10][anisearchid=11]"
    );
    assert_eq!(result.year, None);
    assert_eq!(result.provider_ids.len(), 7);
    assert_eq!(
        result.provider_ids.get("Imdb").map(String::as_str),
        Some("tt10985510")
    );
    assert_eq!(
        result.provider_ids.get("Tvdb").map(String::as_str),
        Some("6")
    );
    assert_eq!(
        result.provider_ids.get("TvMaze").map(String::as_str),
        Some("7")
    );
    assert_eq!(
        result.provider_ids.get("Tmdb").map(String::as_str),
        Some("8")
    );
    assert_eq!(
        result.provider_ids.get("AniDB").map(String::as_str),
        Some("9")
    );
    assert_eq!(
        result.provider_ids.get("AniList").map(String::as_str),
        Some("10")
    );
    assert_eq!(
        result.provider_ids.get("AniSearch").map(String::as_str),
        Some("11")
    );
}

#[test]
fn episode_resolver_preserves_numbers_and_extracts_provider_ids() {
    let result = EpisodeResolver::new(NamingOptions::default())
        .resolve(
            "/shows/Series/Season 01/Series S01E02 [imdbid=tt10985510][tvdb=6][tvmazeid=7][tmdbid=8].mkv",
            false,
        )
        .unwrap();

    assert_eq!(result.season_number, Some(1));
    assert_eq!(result.episode_number, Some(2));
    assert_eq!(result.provider_ids.len(), 4);
    assert_eq!(
        result.provider_ids.get("Imdb").map(String::as_str),
        Some("tt10985510")
    );
    assert_eq!(
        result.provider_ids.get("Tvdb").map(String::as_str),
        Some("6")
    );
    assert_eq!(
        result.provider_ids.get("TvMaze").map(String::as_str),
        Some("7")
    );
    assert_eq!(
        result.provider_ids.get("Tmdb").map(String::as_str),
        Some("8")
    );
}

#[test]
fn season_parser_preserves_number_and_extracts_provider_ids() {
    let result = SeasonPathParser::parse(
        "/shows/Series/Season 2 [tvdb=6][tvmazeid=7][tmdbid=8]",
        Some("/shows/Series"),
        true,
        true,
    );

    assert!(result.success);
    assert!(result.is_season_folder);
    assert_eq!(result.season_number, Some(2));
    assert_eq!(result.provider_ids.len(), 3);
    assert_eq!(
        result.provider_ids.get("Tvdb").map(String::as_str),
        Some("6")
    );
    assert_eq!(
        result.provider_ids.get("TvMaze").map(String::as_str),
        Some("7")
    );
    assert_eq!(
        result.provider_ids.get("Tmdb").map(String::as_str),
        Some("8")
    );
}
