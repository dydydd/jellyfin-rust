use jellyfin_naming::{NamingOptions, VideoResolver};

#[test]
fn needs_cleaning_official_matrix() {
    let options = NamingOptions::default();
    let cases = [
        ("Super movie 480p.mp4", "Super movie"),
        ("Super movie Multi.mp4", "Super movie"),
        ("Super movie 480p 2001.mp4", "Super movie"),
        ("Super movie [480p].mp4", "Super movie"),
        ("480 Super movie [tmdbid=12345].mp4", "480 Super movie"),
        (
            "Crouching.Tiger.Hidden.Dragon.4k.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.UltraHD.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.UHD.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.HDR.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.HDC.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon-HDC.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.BDrip.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.BDrip-HDC.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Crouching.Tiger.Hidden.Dragon.4K.UltraHD.HDR.BDrip-HDC.mkv",
            "Crouching.Tiger.Hidden.Dragon",
        ),
        (
            "Last.Call.for.Nowhere.WEB-DL.1080p",
            "Last.Call.for.Nowhere",
        ),
        (
            "[HorribleSubs] Made in Abyss - 13 [720p].mkv",
            "Made in Abyss",
        ),
        (
            "[Tsundere] Kore wa Zombie Desu ka of the Dead [BDRip h264 1920x1080 FLAC]",
            "Kore wa Zombie Desu ka of the Dead",
        ),
        (
            "[Erai-raws] Jujutsu Kaisen - 03 [720p][Multiple Subtitle].mkv",
            "Jujutsu Kaisen",
        ),
        ("[OCN] 애타는 로맨스 720p-NEXT", "애타는 로맨스"),
        ("[tvN] 혼술남녀.E01-E16.720p-NEXT", "혼술남녀"),
        (
            "[tvN] 연애말고 결혼 E01~E16 END HDTV.H264.720p-WITH",
            "연애말고 결혼",
        ),
        (
            "2026年01月10日23時00分00秒-[新]TRIGUN　STARGAZE[字].mp4",
            "2026年01月10日23時00分00秒-[新]TRIGUN　STARGAZE",
        ),
    ];

    assert_eq!(cases.len(), 22);
    for (input, expected) in cases {
        assert_eq!(
            VideoResolver::try_clean_string(Some(input), &options).as_deref(),
            Some(expected),
            "input={input:?}"
        );
    }
}

#[test]
fn does_not_need_cleaning_official_matrix() {
    let options = NamingOptions::default();
    for input in [
        None,
        Some(""),
        Some("Super movie(2009).mp4"),
        Some("[rec].mkv"),
        Some("American.Psycho.mkv"),
        Some("American Psycho.mkv"),
        Some("Run lola run (lola rennt) (2009).mp4"),
        Some("2026年01月05日00時55分00秒-[新]違国日記【ＡＮｉＭｉＤＮｉＧＨＴ！！！】＃１.mp4"),
    ] {
        assert_eq!(
            VideoResolver::try_clean_string(input, &options),
            None,
            "input={input:?}"
        );
    }
}
