use jellyfin_naming::{EpisodeExpression, EpisodePathParser, EpisodeResolver, NamingOptions};

#[test]
fn official_episode_path_matrix() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    let cases = [
        ("/media/Foo/Foo-S01E01", true, "Foo", 1, 1),
        ("/media/Foo - S04E011", true, "Foo", 4, 11),
        ("/media/Foo/Foo s01x01", true, "Foo", 1, 1),
        (
            "/media/Foo (2019)/Season 4/Foo (2019).S04E03",
            true,
            "Foo (2019)",
            4,
            3,
        ),
        (r"D:\media\Foo\Foo-S01E01", true, "Foo", 1, 1),
        (r"D:\media\Foo - S04E011", true, "Foo", 4, 11),
        (r"D:\media\Foo\Foo s01x01", true, "Foo", 1, 1),
        (
            r"D:\media\Foo (2019)\Season 4\Foo (2019).S04E03",
            true,
            "Foo (2019)",
            4,
            3,
        ),
        (
            "/Season 2/Elementary - 02x03-04-15 - Ep Name.mp4",
            false,
            "Elementary",
            2,
            3,
        ),
        (
            "/Season 1/seriesname S01E02 blah.avi",
            false,
            "seriesname",
            1,
            2,
        ),
        (
            "/Running Man/Running Man S2017E368.mkv",
            false,
            "Running Man",
            2017,
            368,
        ),
        (
            "/Season 1/seriesname 01x02 blah.avi",
            false,
            "seriesname",
            1,
            2,
        ),
        (
            "/Season 25/The Simpsons.S25E09.Steal this episode.mp4",
            false,
            "The Simpsons",
            25,
            9,
        ),
        (
            "/Season 1/seriesname S01x02 blah.avi",
            false,
            "seriesname",
            1,
            2,
        ),
        (
            "/Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4",
            false,
            "Elementary",
            2,
            3,
        ),
        (
            "/Season 1/seriesname S01xE02 blah.avi",
            false,
            "seriesname",
            1,
            2,
        ),
        (
            "/Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4",
            false,
            "Elementary",
            2,
            3,
        ),
        (
            "/Season 02/Elementary - 02x03x04x15 - Ep Name.mp4",
            false,
            "Elementary",
            2,
            3,
        ),
        (
            "/Season 02/Elementary - 02x03-E15 - Ep Name.mp4",
            false,
            "Elementary",
            2,
            3,
        ),
        (
            "/Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4",
            false,
            "Elementary",
            1,
            23,
        ),
        (
            "/The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi",
            false,
            "The Wonder Years",
            4,
            7,
        ),
        (
            "/The.Sopranos/Season 3/The Sopranos Season 3 Episode 09 - The Telltale Moozadell.avi",
            false,
            "The Sopranos",
            3,
            9,
        ),
    ];
    assert_eq!(cases.len(), 22);

    for (path, is_directory, series, season, episode) in cases {
        let result = parser.parse(path, is_directory);
        assert!(result.success, "{path}");
        assert_eq!(result.series_name.as_deref(), Some(series), "{path}");
        assert_eq!(result.season_number, Some(season), "{path}");
        assert_eq!(result.episode_number, Some(episode), "{path}");
    }
}

#[test]
fn expression_filters_can_select_named_optimistic_rules() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    let result =
        parser.parse_with_options("/test/01-03.avi", false, Some(true), Some(true), None, true);
    assert!(result.success);
}

#[test]
fn pixel_dimensions_are_not_episode_numbers() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    assert!(
        !parser
            .parse("Series Special (1920x1080).mkv", false)
            .success
    );
}

#[test]
fn resolver_rejects_an_unsupported_extension() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    assert_eq!(resolver.resolve("test.mp3", false), None);
}

#[test]
fn resolver_accepts_a_stub_extension() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    let result = resolver.resolve("dvd.disc", false).expect("stub episode");
    assert!(result.is_stub);
}

#[test]
fn positional_date_expression_without_formats_remains_supported() {
    let options = NamingOptions {
        episode_expressions: vec![
            EpisodeExpression::try_new(
                r"(([0-9]{4})-([0-9]{2})-([0-9]{2}) [0-9]{2}:[0-9]{2}:[0-9]{2})",
                true,
            )
            .expect("valid custom expression"),
        ],
        ..NamingOptions::default()
    };
    let result = EpisodePathParser::new(options).parse("ABC_2019_10_21 11:00:00", false);
    assert!(result.success);
    assert_eq!(
        (result.year, result.month, result.day),
        (Some(2019), Some(10), Some(21))
    );
}
