use jellyfin_naming::{SeriesStatus, TvParserHelpers};

#[test]
fn official_valid_series_status_matrix() {
    let cases = [
        ("Ended", SeriesStatus::Ended),
        ("Cancelled", SeriesStatus::Ended),
        ("Continuing", SeriesStatus::Continuing),
        ("Returning", SeriesStatus::Continuing),
        ("Returning Series", SeriesStatus::Continuing),
        ("Unreleased", SeriesStatus::Unreleased),
    ];
    assert_eq!(cases.len(), 6);
    for (input, expected) in cases {
        assert_eq!(
            TvParserHelpers::try_parse_series_status(Some(input)),
            Some(expected)
        );
    }
}

#[test]
fn official_invalid_series_status_matrix() {
    assert_eq!(TvParserHelpers::try_parse_series_status(Some("XXX")), None);
}
