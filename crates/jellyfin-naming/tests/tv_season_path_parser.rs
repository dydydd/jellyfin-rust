use jellyfin_naming::SeasonPathParser;

#[test]
fn official_season_path_matrix() {
    let cases = [
        ("/Drive/Season 1", "/Drive", Some(1), true),
        ("/Drive/SEASON 1", "/Drive", Some(1), true),
        ("/Drive/Staffel 1", "/Drive", Some(1), true),
        ("/Drive/STAFFEL 1", "/Drive", Some(1), true),
        ("/Drive/Stagione 1", "/Drive", Some(1), true),
        ("/Drive/STAGIONE 1", "/Drive", Some(1), true),
        ("/Drive/sæson 1", "/Drive", Some(1), true),
        ("/Drive/SÆSON 1", "/Drive", Some(1), true),
        ("/Drive/Temporada 1", "/Drive", Some(1), true),
        ("/Drive/TEMPORADA 1", "/Drive", Some(1), true),
        ("/Drive/series 1", "/Drive", Some(1), true),
        ("/Drive/SERIES 1", "/Drive", Some(1), true),
        ("/Drive/Kausi 1", "/Drive", Some(1), true),
        ("/Drive/KAUSI 1", "/Drive", Some(1), true),
        ("/Drive/Säsong 1", "/Drive", Some(1), true),
        ("/Drive/SÄSONG 1", "/Drive", Some(1), true),
        ("/Drive/Seizoen 1", "/Drive", Some(1), true),
        ("/Drive/SEIZOEN 1", "/Drive", Some(1), true),
        ("/Drive/Seasong 1", "/Drive", Some(1), true),
        ("/Drive/SEASONG 1", "/Drive", Some(1), true),
        ("/Drive/Sezon 1", "/Drive", Some(1), true),
        ("/Drive/SEZON 1", "/Drive", Some(1), true),
        ("/Drive/sezona 1", "/Drive", Some(1), true),
        ("/Drive/SEZONA 1", "/Drive", Some(1), true),
        ("/Drive/sezóna 1", "/Drive", Some(1), true),
        ("/Drive/SEZÓNA 1", "/Drive", Some(1), true),
        ("/Drive/Sezonul 1", "/Drive", Some(1), true),
        ("/Drive/SEZONUL 1", "/Drive", Some(1), true),
        ("/Drive/시즌 1", "/Drive", Some(1), true),
        ("/Drive/シーズン 1", "/Drive", Some(1), true),
        ("/Drive/сезон 1", "/Drive", Some(1), true),
        ("/Drive/Сезон 1", "/Drive", Some(1), true),
        ("/Drive/СЕЗОН 1", "/Drive", Some(1), true),
        ("/Drive/Season 10", "/Drive", Some(10), true),
        ("/Drive/Season 100", "/Drive", Some(100), true),
        ("/Drive/s1", "/Drive", Some(1), true),
        ("/Drive/S1", "/Drive", Some(1), true),
        ("/Drive/Season 2", "/Drive", Some(2), true),
        ("/Drive/Season 02", "/Drive", Some(2), true),
        ("/Drive/Seinfeld/S02", "/Seinfeld", Some(2), true),
        ("/Drive/Seinfeld/2", "/Seinfeld", Some(2), true),
        ("/Drive/Seinfeld Season 2", "/Drive", None, false),
        ("/Drive/Season 2009", "/Drive", Some(2009), true),
        ("/Drive/Season1", "/Drive", Some(1), true),
        (
            "The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH",
            "/The Wonder Years",
            Some(4),
            true,
        ),
        ("/Drive/Season 7 (2016)", "/Drive", Some(7), true),
        ("/Drive/Staffel 7 (2016)", "/Drive", Some(7), true),
        ("/Drive/Stagione 7 (2016)", "/Drive", Some(7), true),
        (
            "/Drive/Stargate SG-1/Season 1",
            "/Drive/Stargate SG-1",
            Some(1),
            true,
        ),
        (
            "/Drive/Stargate SG-1/Stargate SG-1 Season 1",
            "/Drive/Stargate SG-1",
            Some(1),
            true,
        ),
        ("/Drive/Season (8)", "/Drive", None, false),
        ("/Drive/3.Staffel", "/Drive", Some(3), true),
        ("/Drive/1. season", "/Drive", Some(1), true),
        ("/Drive/s06e05", "/Drive", None, false),
        (
            "/Drive/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv",
            "/Drive",
            None,
            false,
        ),
        ("/Drive/extras", "/Drive", Some(0), true),
        ("/Drive/EXTRAS", "/Drive", Some(0), true),
        ("/Drive/specials", "/Drive", Some(0), true),
        ("/Drive/SPECIALS", "/Drive", Some(0), true),
        ("/Drive/Episode 1 Season 2", "/Drive", None, false),
        ("/Drive/Episode 1 SEASON 2", "/Drive", None, false),
        (
            "/media/YouTube/Devyn Johnston/2024-01-24 4070 Ti SUPER in under 7 minutes",
            "/media/YouTube/Devyn Johnston",
            None,
            false,
        ),
        (
            "/media/YouTube/Devyn Johnston/2025-01-28 5090 vs 2 SFF Cases",
            "/media/YouTube/Devyn Johnston",
            None,
            false,
        ),
        ("/Drive/202401244070", "/Drive", None, false),
        (
            "/Drive/Drive.S01.2160p.WEB-DL.DDP5.1.H.265-XXXX",
            "/Drive",
            Some(1),
            true,
        ),
        (
            "The Wonder Years/The.Wonder.Years.S04.1080p.PDTV.x264-JCH",
            "/The Wonder Years",
            Some(4),
            true,
        ),
        (
            "The Wonder Years/[The.Wonder.Years.S04.1080p.PDTV.x264-JCH]",
            "/The Wonder Years",
            Some(4),
            true,
        ),
        (
            "The Wonder Years/The.Wonder.Years [S04][1080p.PDTV.x264-JCH]",
            "/The Wonder Years",
            Some(4),
            true,
        ),
        (
            "The Wonder Years/The Wonder Years Season 01 1080p",
            "/The Wonder Years",
            Some(1),
            true,
        ),
    ];
    assert_eq!(cases.len(), 69);

    let mismatches = cases
        .into_iter()
        .filter_map(|(path, parent, season, is_season_folder)| {
            let result = SeasonPathParser::parse(path, Some(parent), true, true);
            assert_eq!(result.success, result.season_number.is_some(), "{path}");
            ((result.season_number, result.is_season_folder) != (season, is_season_folder)).then(
                || {
                    format!(
                        "{path}: expected ({season:?}, {is_season_folder}), got ({:?}, {})",
                        result.season_number, result.is_season_folder
                    )
                },
            )
        })
        .collect::<Vec<_>>();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn official_mixed_library_season_path_matrix() {
    let cases = [
        (
            "/Drive/300 Collection/300 (2006)",
            "/Drive/300 Collection",
            None,
            false,
        ),
        (
            "/Drive/300 Collection/300 Rise of an Empire",
            "/Drive/300 Collection",
            None,
            false,
        ),
        (
            "/Drive/300 Collection/1",
            "/Drive/300 Collection",
            None,
            false,
        ),
        (
            "/Drive/300 Collection/300 Disc 1",
            "/Drive/300 Collection",
            None,
            false,
        ),
        (
            "/Drive/28 Years Later Collection/28 Days Later",
            "/Drive/28 Years Later Collection",
            None,
            false,
        ),
        (
            "/Drive/28 Years Later Collection/28 Weeks Later (2007)",
            "/Drive/28 Years Later Collection",
            None,
            false,
        ),
        (
            "/Drive/28 Years Later Collection/28 Years Later 2025",
            "/Drive/28 Years Later Collection",
            None,
            false,
        ),
        (
            "/Drive/300 Collection/Season 1",
            "/Drive/300 Collection",
            Some(1),
            true,
        ),
        (
            "/Drive/28 Years Later Collection/Season 01",
            "/Drive/28 Years Later Collection",
            Some(1),
            true,
        ),
        (
            "/Drive/300 Collection/S01",
            "/Drive/300 Collection",
            Some(1),
            true,
        ),
        (
            "/Drive/300 Collection/S1",
            "/Drive/300 Collection",
            Some(1),
            true,
        ),
    ];
    assert_eq!(cases.len(), 11);

    for (path, parent, season, is_season_folder) in cases {
        let result = SeasonPathParser::parse(path, Some(parent), false, false);
        assert_eq!(result.success, result.season_number.is_some(), "{path}");
        assert_eq!(result.season_number, season, "{path}");
        assert_eq!(result.is_season_folder, is_season_folder, "{path}");
    }
}
