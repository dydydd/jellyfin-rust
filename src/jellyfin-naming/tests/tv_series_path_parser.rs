use jellyfin_naming::{NamingOptions, SeriesPathParser};

#[test]
fn official_series_path_matrix() {
    let options = NamingOptions::default();
    let cases = [
        ("The.Show.S01", "The.Show"),
        ("/The.Show.S01", "The.Show"),
        ("/some/place/The.Show.S01", "The.Show"),
        ("/something/The.Show.S01", "The.Show"),
        ("The Show Season 10", "The Show"),
        ("The Show S01E01", "The Show"),
        ("The Show S01E01 Episode", "The Show"),
        ("/something/The Show/Season 1", "The Show"),
        ("/something/The Show/S01", "The Show"),
    ];
    assert_eq!(cases.len(), 9);

    for (path, expected) in cases {
        let result = SeriesPathParser::parse(&options, path);
        assert!(result.success, "{path}");
        assert_eq!(result.series_name.as_deref(), Some(expected), "{path}");
    }
}
