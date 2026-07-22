use jellyfin_naming::{EpisodePathParser, NamingOptions};

#[test]
fn official_episode_number_matrix() {
    let parser = EpisodePathParser::new(NamingOptions::default());
    let cases = [
        ("Season 21/One Piece 1001", 1001),
        (
            "Watchmen (2019)/Watchmen 1x03 [WEBDL-720p][EAC3 5.1][h264][-TBS] - She Was Killed by Space Junk.mkv",
            3,
        ),
        (
            "The Daily Show/The Daily Show 25x22 - [WEBDL-720p][AAC 2.0][x264] Noah Baumbach-TBS.mkv",
            22,
        ),
        (
            "Castle Rock 2x01 Que el rio siga su curso [WEB-DL HULU 1080p h264 Dual DD5.1 Subs].mkv",
            1,
        ),
        (
            "After Life 1x06 Episodio 6 [WEB-DL NF 1080p h264 Dual DD 5.1 Sub].mkv",
            6,
        ),
        ("Season 02/S02E03 blah.avi", 3),
        ("Season 2/02x03 - 02x04 - 02x15 - Ep Name.mp4", 3),
        ("Season 02/02x03 - x04 - x15 - Ep Name.mp4", 3),
        ("Season 1/01x02 blah.avi", 2),
        ("Season 1/S01x02 blah.avi", 2),
        ("Season 1/S01E02 blah.avi", 2),
        ("Season 2/Elementary - 02x03-04-15 - Ep Name.mp4", 3),
        ("Season 1/S01xE02 blah.avi", 2),
        ("Season 1/seriesname S01E02 blah.avi", 2),
        ("Season 2/Episode - 16.avi", 16),
        ("Season 2/Episode 16.avi", 16),
        ("Season 2/Episode 16 - Some Title.avi", 16),
        ("Season 2/16 Some Title.avi", 16),
        ("Season 2/16 - 12 Some Title.avi", 16),
        ("Season 2/7 - 12 Angry Men.avi", 7),
        ("Season 1/seriesname 01x02 blah.avi", 2),
        ("Season 25/The Simpsons.S25E09.Steal this episode.mp4", 9),
        ("Season 1/seriesname S01x02 blah.avi", 2),
        (
            "Season 2/Elementary - 02x03 - 02x04 - 02x15 - Ep Name.mp4",
            3,
        ),
        ("Season 1/seriesname S01xE02 blah.avi", 2),
        ("Season 02/Elementary - 02x03 - x04 - x15 - Ep Name.mp4", 3),
        ("Season 02/Elementary - 02x03x04x15 - Ep Name.mp4", 3),
        ("Season 2/02x03-04-15 - Ep Name.mp4", 3),
        ("Season 02/02x03-E15 - Ep Name.mp4", 3),
        ("Season 02/Elementary - 02x03-E15 - Ep Name.mp4", 3),
        ("Season 1/Elementary - S01E23-E24-E26 - The Woman.mp4", 23),
        ("Season 2009/S2009E23-E24-E26 - The Woman.mp4", 23),
        ("Season 2009/2009x02 blah.avi", 2),
        ("Season 2009/S2009x02 blah.avi", 2),
        ("Season 2009/S2009E02 blah.avi", 2),
        ("Season 2009/seriesname 2009x02 blah.avi", 2),
        ("Season 2009/Elementary - 2009x03x04x15 - Ep Name.mp4", 3),
        ("Season 2009/2009x03x04x15 - Ep Name.mp4", 3),
        ("Season 2009/Elementary - 2009x03-E15 - Ep Name.mp4", 3),
        ("Season 2009/S2009xE02 blah.avi", 2),
        (
            "Season 2009/Elementary - S2009E23-E24-E26 - The Woman.mp4",
            23,
        ),
        ("Season 2009/seriesname S2009xE02 blah.avi", 2),
        ("Season 2009/2009x03-E15 - Ep Name.mp4", 3),
        ("Season 2009/seriesname S2009E02 blah.avi", 2),
        ("Season 2009/2009x03 - 2009x04 - 2009x15 - Ep Name.mp4", 3),
        ("Season 2009/2009x03 - x04 - x15 - Ep Name.mp4", 3),
        ("Season 2009/seriesname S2009x02 blah.avi", 2),
        (
            "Season 2009/Elementary - 2009x03 - 2009x04 - 2009x15 - Ep Name.mp4",
            3,
        ),
        ("Season 2009/Elementary - 2009x03-04-15 - Ep Name.mp4", 3),
        ("Season 2009/2009x03-04-15 - Ep Name.mp4", 3),
        (
            "Season 2009/Elementary - 2009x03 - x04 - x15 - Ep Name.mp4",
            3,
        ),
        ("Season 1/02 - blah-02 a.avi", 2),
        ("Season 1/02 - blah.avi", 2),
        ("Season 2/02 - blah 14 blah.avi", 2),
        ("Season 2/02.avi", 2),
        ("Season 2/2. Infestation.avi", 2),
        (
            "The Wonder Years/The.Wonder.Years.S04.PDTV.x264-JCH/The Wonder Years s04e07 Christmas Party NTSC PDTV.avi",
            7,
        ),
        ("Running Man/Running Man S2017E368.mkv", 368),
        (
            "Season 2/[HorribleSubs] Hunter X Hunter - 136 [720p].mkv",
            136,
        ),
        (
            "Log Horizon 2/[HorribleSubs] Log Horizon 2 - 03 [720p].mkv",
            3,
        ),
        ("Season 1/seriesname 05.mkv", 5),
        ("[BBT-RMX] Ranma ½ - 154 [50AC421A].mkv", 154),
        ("Season 2/Episode 21 - 94 Meetings.mp4", 21),
        (
            "/The.Legend.of.Condor.Heroes.2017.V2.web-dl.1080p.h264.aac-hdctv/The.Legend.of.Condor.Heroes.2017.E07.V2.web-dl.1080p.h264.aac-hdctv.mkv",
            7,
        ),
        ("Season 3/The Series Season 3 Episode 9 - The title.avi", 9),
        ("Season 3/The Series S3 E9 - The title.avi", 9),
        ("Season 3/S003 E009.avi", 9),
        ("Season 3/Season 3 Episode 9.avi", 9),
        (
            "[VCB-Studio] Re Zero kara Hajimeru Isekai Seikatsu [21][Ma10p_1080p][x265_flac].mkv",
            21,
        ),
        (
            "[CASO&Sumisora][Oda_Nobuna_no_Yabou][04][BDRIP][1920x1080][x264_AAC][7620E503].mp4",
            4,
        ),
        ("Case Closed (1996-2007)/Case Closed - 317.mkv", 317),
        ("Season 2/Hunter X Hunter - 101.mkv", 101),
        ("Season 1/Show Name - 1234 [720p].mkv", 1234),
        ("Season 2/16 12 Some Title.avi", 16),
        ("Season 4/Uchuu.Senkan.Yamato.2199.E03.avi", 3),
        ("Season 2/7 12 Angry Men.avi", 7),
        ("Season 02/02x03x04x15 - Ep Name.mp4", 2),
    ];

    assert_eq!(cases.len(), 77);
    let mismatches = cases
        .into_iter()
        .filter_map(|(path, expected)| {
            let actual = parser.parse(path, false).episode_number;
            (actual != Some(expected))
                .then(|| format!("{path}: expected {expected:?}, got {actual:?}"))
        })
        .collect::<Vec<_>>();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
