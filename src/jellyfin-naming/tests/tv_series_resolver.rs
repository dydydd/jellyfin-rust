use jellyfin_naming::{NamingOptions, SeriesResolver};

#[test]
fn official_series_resolver_matrix() {
    let options = NamingOptions::default();
    let cases = [
        ("The.Show.S01", "The Show"),
        ("The.Show.S01.COMPLETE", "The Show"),
        ("S.H.O.W.S01", "S.H.O.W"),
        ("The.Show.P.I.S01", "The Show P.I"),
        ("The_Show_Season_1", "The Show"),
        ("/something/The_Show/Season 10", "The Show"),
        ("The Show", "The Show"),
        ("/some/path/The Show", "The Show"),
        ("/some/path/The Show s02e10 720p hdtv", "The Show"),
        (
            "/some/path/The Show s02e10 the episode 720p hdtv",
            "The Show",
        ),
        ("/some/path/1923 (2022)", "1923"),
    ];
    assert_eq!(cases.len(), 11);

    for (path, expected) in cases {
        let result = SeriesResolver::resolve(&options, path);
        assert_eq!(result.name.as_deref(), Some(expected), "{path}");
    }
}

#[test]
fn resolver_preserves_the_year_as_structured_data() {
    let result = SeriesResolver::resolve(&NamingOptions::default(), "/some/path/1923 (2022)");
    assert_eq!(result.year, Some(2022));
}
