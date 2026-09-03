use jellyfin_naming::{EpisodeResolver, NamingOptions};

#[test]
fn official_simple_episode_matrix() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    let cases = [
        (
            "/server/anything_s01e02.mp4",
            "anything",
            Some(1),
            Some(2),
            None,
        ),
        (
            "/server/anything_s1e2.mp4",
            "anything",
            Some(1),
            Some(2),
            None,
        ),
        (
            "/server/anything_s01.e02.mp4",
            "anything",
            Some(1),
            Some(2),
            None,
        ),
        (
            "/server/anything_102.mp4",
            "anything",
            Some(1),
            Some(2),
            None,
        ),
        (
            "/server/anything_1x02.mp4",
            "anything",
            Some(1),
            Some(2),
            None,
        ),
        (
            "/server/The Walking Dead 4x01.mp4",
            "The Walking Dead",
            Some(4),
            Some(1),
            None,
        ),
        (
            "/server/the_simpsons-s02e01_18536.mp4",
            "the_simpsons",
            Some(2),
            Some(1),
            None,
        ),
        ("/server/Temp/S01E02 foo.mp4", "", Some(1), Some(2), None),
        ("Series/4x12 - The Woman.mp4", "", Some(4), Some(12), None),
        (
            "Series/LA X, Pt. 1_s06e32.mp4",
            "LA X, Pt. 1",
            Some(6),
            Some(32),
            None,
        ),
        (
            "[Baz-Bar]Foo - [1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv",
            "Foo",
            None,
            Some(5),
            None,
        ),
        (
            "/Foo/The.Series.Name.S01E04.WEBRip.x264-Baz[Bar]/the.series.name.s01e04.webrip.x264-Baz[Bar].mkv",
            "The.Series.Name",
            Some(1),
            Some(4),
            None,
        ),
        (
            "Love.Death.and.Robots.S01.1080p.NF.WEB-DL.DDP5.1.x264-NTG/Love.Death.and.Robots.S01E01.Sonnies.Edge.1080p.NF.WEB-DL.DDP5.1.x264-NTG.mkv",
            "Love.Death.and.Robots",
            Some(1),
            Some(1),
            None,
        ),
        (
            "[YuiSubs] Tensura Nikki - Tensei Shitara Slime Datta Ken/[YuiSubs] Tensura Nikki - Tensei Shitara Slime Datta Ken - 12 (NVENC H.265 1080p).mkv",
            "Tensura Nikki - Tensei Shitara Slime Datta Ken",
            None,
            Some(12),
            None,
        ),
        (
            "[Baz-Bar]Foo - 01 - 12[1080p][Multiple Subtitle]/[Baz-Bar] Foo - 05 [1080p][Multiple Subtitle].mkv",
            "Foo",
            None,
            Some(5),
            None,
        ),
        (
            "Series/4-12 - The Woman.mp4",
            "",
            Some(4),
            Some(12),
            Some(12),
        ),
        (
            "/Library/Series/The Grand Tour (2016)/Season 1/S01E01 The Holy Trinity.mkv",
            "The Grand Tour",
            Some(1),
            Some(1),
            None,
        ),
    ];
    assert_eq!(cases.len(), 17);

    for (path, series, season, episode, ending) in cases {
        let result = resolver
            .resolve(path, false)
            .unwrap_or_else(|| panic!("{path}"));
        assert_eq!(result.season_number, season, "{path}");
        assert_eq!(result.episode_number, episode, "{path}");
        assert!(
            result
                .series_name
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(series)),
            "{path}: {:?}",
            result.series_name
        );
        assert_eq!(result.path, path, "{path}");
        assert_eq!(result.ending_episode_number, ending, "{path}");
        assert_eq!(result.format_3d, None, "{path}");
        assert!(!result.is_3d, "{path}");
        assert!(!result.is_stub, "{path}");
        assert_eq!(result.stub_type, None, "{path}");
        assert!(!result.is_by_date, "{path}");
    }
}
