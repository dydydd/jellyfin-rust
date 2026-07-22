use jellyfin_naming::{NamingOptions, VideoResolver};

#[test]
fn clean_date_time_official_matrix() {
    let options = NamingOptions::default();
    let cases = [
        (
            "The Wolf of Wall Street (2013).mkv",
            "The Wolf of Wall Street",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street 2 (2013).mkv",
            "The Wolf of Wall Street 2",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street - 2 (2013).mkv",
            "The Wolf of Wall Street - 2",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street 2001 (2013).mkv",
            "The Wolf of Wall Street 2001",
            Some(2013),
        ),
        ("300 (2006).mkv", "300", Some(2006)),
        ("d:/movies/300 (2006).mkv", "300", Some(2006)),
        ("300 2 (2006).mkv", "300 2", Some(2006)),
        ("300 - 2 (2006).mkv", "300 - 2", Some(2006)),
        ("300 2001 (2006).mkv", "300 2001", Some(2006)),
        (
            "curse.of.chucky.2013.stv.unrated.multi.1080p.bluray.x264-rough",
            "curse.of.chucky",
            Some(2013),
        ),
        (
            "curse.of.chucky.2013.stv.unrated.multi.2160p.bluray.x264-rough",
            "curse.of.chucky",
            Some(2013),
        ),
        (
            "/server/Movies/300 (2007)/300 (2006).bluray.disc",
            "300",
            Some(2006),
        ),
        ("Arrival.2016.2160p.Blu-Ray.HEVC.mkv", "Arrival", Some(2016)),
        (
            "The Wolf of Wall Street (2013)",
            "The Wolf of Wall Street",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street 2 (2013)",
            "The Wolf of Wall Street 2",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street - 2 (2013)",
            "The Wolf of Wall Street - 2",
            Some(2013),
        ),
        (
            "The Wolf of Wall Street 2001 (2013)",
            "The Wolf of Wall Street 2001",
            Some(2013),
        ),
        ("300 (2006)", "300", Some(2006)),
        ("d:/movies/300 (2006)", "300", Some(2006)),
        ("300 2 (2006)", "300 2", Some(2006)),
        ("300 - 2 (2006)", "300 - 2", Some(2006)),
        ("300 2001 (2006)", "300 2001", Some(2006)),
        ("/server/Movies/300 (2007)/300 (2006)", "300", Some(2006)),
        (
            "/server/Movies/300 (2007)/300 (2006).mkv",
            "300",
            Some(2006),
        ),
        ("American.Psycho.mkv", "American.Psycho.mkv", None),
        ("American Psycho.mkv", "American Psycho.mkv", None),
        ("[rec].mkv", "[rec].mkv", None),
        ("St. Vincent (2014)", "St. Vincent", Some(2014)),
        ("Super movie(2009).mp4", "Super movie", Some(2009)),
        ("Drug War 2013.mp4", "Drug War", Some(2013)),
        (
            "My Movie (1997) - GreatestReleaseGroup 2019.mp4",
            "My Movie",
            Some(1997),
        ),
        ("First Man 2018 1080p.mkv", "First Man", Some(2018)),
        ("First Man (2018) 1080p.mkv", "First Man", Some(2018)),
        (
            "Maximum Ride - 2016 - WEBDL-1080p - x264 AC3.mkv",
            "Maximum Ride",
            Some(2016),
        ),
        (
            "3.Days.to.Kill.2014.720p.BluRay.x264.YIFY.mkv",
            "3.Days.to.Kill",
            Some(2014),
        ),
        ("3 days to kill (2005).mkv", "3 days to kill", Some(2005)),
        (
            "Rain Man 1988 REMASTERED 1080p BluRay x264 AAC - Ozlem.mp4",
            "Rain Man",
            Some(1988),
        ),
        ("My Movie 2013.12.09", "My Movie 2013.12.09", None),
        ("My Movie 2013-12-09", "My Movie 2013-12-09", None),
        ("My Movie 20131209", "My Movie 20131209", None),
        (
            "My Movie 2013-12-09 2013",
            "My Movie 2013-12-09",
            Some(2013),
        ),
        ("", "", None),
    ];

    assert_eq!(cases.len(), 42);
    for (input, expected_name, expected_year) in cases {
        let input = file_name(input);
        let result = VideoResolver::clean_date_time(input, &options);
        assert!(
            result.name.eq_ignore_ascii_case(expected_name),
            "input={input:?}: {:?} != {expected_name:?}",
            result.name
        );
        assert_eq!(result.year, expected_year, "input={input:?}");
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}
