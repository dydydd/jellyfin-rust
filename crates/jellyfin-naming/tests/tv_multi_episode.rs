use jellyfin_naming::{EpisodePathParser, NamingOptions};

#[test]
fn official_multiple_episode_matrix() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    let cases = [
        ("Season 1/4x01 – 20 Hours in America (1).mkv", None),
        ("Season 1/01x02 blah.avi", None),
        ("Season 1/S01x02 blah.avi", None),
        ("Season 1/S01E02 blah.avi", None),
        ("Season 1/S01xE02 blah.avi", None),
        ("Season 1/seriesname 01x02 blah.avi", None),
        ("Season 1/seriesname S01x02 blah.avi", None),
        ("Season 1/seriesname S01E02 blah.avi", None),
        ("Season 1/seriesname S01xE02 blah.avi", None),
        ("Season 2/02x03 - 04 Ep Name.mp4", None),
        ("Season 2/My show name 02x03 - 04 Ep Name.mp4", None),
        (
            "Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 2/02x03 - 02x04 - 02x15 - Ep Name.mp4", Some(15)),
        ("Season 2/02x03-04-15 - Ep Name.mp4", Some(15)),
        ("Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", Some(15)),
        ("Season 02/02x03-E15 - Ep Name.mp4", Some(15)),
        ("Season 02/Elementary - 02x03-E15 - Ep Name.mp4", Some(15)),
        ("Season 02/02x03 - x04 - x15 - Ep Name.mp4", Some(15)),
        (
            "Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 02/02x03x04x15 - Ep Name.mp4", Some(15)),
        ("Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", Some(15)),
        (
            "Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4",
            Some(26),
        ),
        ("Season 1/S01E23-E24-E26 - The Woman.mp4", Some(26)),
        ("Season 2009/2009x02 blah.avi", None),
        ("Season 2009/S2009x02 blah.avi", None),
        ("Season 2009/S2009E02 blah.avi", None),
        ("Season 2009/S2009xE02 blah.avi", None),
        ("Season 2009/seriesname 2009x02 blah.avi", None),
        ("Season 2009/seriesname S2009x02 blah.avi", None),
        ("Season 2009/seriesname S2009E02 blah.avi", None),
        ("Season 2009/seriesname S2009xE02 blah.avi", None),
        (
            "Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4",
            Some(15),
        ),
        (
            "Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 2009/2009x03-04-15 - Ep Name.mp4", Some(15)),
        (
            "Season 2009/Elementary - 2009x03-04-15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 2009/2009x03-E15 - Ep Name.mp4", Some(15)),
        (
            "Season 2009/Elementary - 2009x03-E15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 2009/2009x03 - x04 - x15 - Ep Name.mp4", Some(15)),
        (
            "Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4",
            Some(15),
        ),
        ("Season 2009/2009x03x04x15 - Ep Name.mp4", Some(15)),
        (
            "Season 2009/Elementary - 2009x03x04x15 - Ep Name.mp4",
            Some(15),
        ),
        (
            "Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4",
            Some(26),
        ),
        ("Season 2009/S2009E23-E24-E26 - The Woman.mp4", Some(26)),
        ("Season 1/02 - blah.avi", None),
        ("Season 2/02 - blah 14 blah.avi", None),
        ("Season 1/02 - blah-02 a.avi", None),
        ("Season 2/02.avi", None),
        ("Season 1/02-03 - blah.avi", Some(3)),
        ("Season 2/02-04 - blah 14 blah.avi", Some(4)),
        ("Season 1/02-05 - blah-02 a.avi", Some(5)),
        ("Season 2/02-04.avi", Some(4)),
        (
            "Season 2 /[HorribleSubs] Hunter X Hunter - 136[720p].mkv",
            None,
        ),
        ("Season 1/series-s09e14-1080p.mkv", None),
        ("Season 1/series-s09e14-720p.mkv", None),
        ("Season 1/series-s09e14-720i.mkv", None),
        ("Season 1/MOONLIGHTING_s01e01-e04.mkv", Some(4)),
        ("Season 1/MOONLIGHTING_s01e01-e04", Some(4)),
    ];
    assert_eq!(cases.len(), 57);

    for (path, expected) in cases {
        assert_eq!(
            parser.parse(path, false).ending_episode_number,
            expected,
            "{path}"
        );
    }
}
