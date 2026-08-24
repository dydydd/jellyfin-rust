use jellyfin_naming::{EpisodePathParser, EpisodeResolver, NamingOptions};

#[test]
fn official_daily_episode_matrix() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    let cases = [
        ("/server/anything_1996.11.14.mp4", "anything", 1996, 11, 14),
        ("/server/anything_1996-11-14.mp4", "anything", 1996, 11, 14),
        (
            "/server/james.corden.2017.04.20.anne.hathaway.720p.hdtv.x264-crooks.mkv",
            "james.corden",
            2017,
            4,
            20,
        ),
        (
            "/server/ABC News 2018_03_24_19_00_00.mkv",
            "ABC News",
            2018,
            3,
            24,
        ),
        (
            "/server/Jeopardy 2023 07 14 HDTV x264 AC3.mkv",
            "Jeopardy",
            2023,
            7,
            14,
        ),
        ("/server/anything_14.11.1996.mp4", "anything", 1996, 11, 14),
        (
            "/server/A Daily Show - (2015-01-15) - Episode Name - [720p].mkv",
            "A Daily Show",
            2015,
            1,
            15,
        ),
        (
            "/server/Last Man Standing_KTLADT_2018_05_25_01_28_00.wtv",
            "Last Man Standing",
            2018,
            5,
            25,
        ),
    ];
    assert_eq!(cases.len(), 8);

    for (path, series, year, month, day) in cases {
        let result = resolver
            .resolve(path, false)
            .unwrap_or_else(|| panic!("{path}"));
        assert_eq!(result.season_number, None, "{path}");
        assert_eq!(result.episode_number, None, "{path}");
        assert_eq!(
            (result.year, result.month, result.day),
            (Some(year), Some(month), Some(day)),
            "{path}"
        );
        assert!(
            result
                .series_name
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(series)),
            "{path}: {:?}",
            result.series_name
        );
    }
}

#[test]
fn invalid_daily_date_keeps_by_date_success_without_parts() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    let result = parser.parse("/server/anything_2013-99-99.mp4", false);
    assert!(result.success);
    assert!(result.is_by_date);
    assert_eq!(result.year, None);
    assert_eq!(result.month, None);
    assert_eq!(result.day, None);
    assert_eq!(result.episode_number, None);
}
