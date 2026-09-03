use jellyfin_media_encoding::probing::{
    estimated_audio_bitrate, frame_rate, is_near_square_pixel_sar,
};

#[test]
fn frame_rate_matrix_matches_official_cases() {
    let cases = [
        ("2997/125", Some(23.976_f32)),
        ("1/50", Some(0.02)),
        ("25/1", Some(25.0)),
        ("120/1", Some(120.0)),
        ("1704753000/71073479", Some(23.985_782)),
        ("0/0", None),
        ("1/1000", Some(0.001)),
        ("1/90000", Some(1.111_111_1e-5)),
        ("1/48000", Some(2.083_333_3e-5)),
    ];
    for (value, expected) in cases {
        assert_eq!(frame_rate(value), expected, "frame rate {value}");
    }
}

#[test]
fn near_square_pixel_sar_matrix_matches_official_cases() {
    let cases = [
        (Some("1:1"), true),
        (Some("3201:3200"), true),
        (Some("1215:1216"), true),
        (Some("1001:1000"), true),
        (Some("16:15"), false),
        (Some("8:9"), false),
        (Some("32:27"), false),
        (Some("10:11"), false),
        (Some("64:45"), false),
        (Some("4:3"), false),
        (Some("0:1"), false),
        (Some(""), false),
        (None, false),
    ];
    for (value, expected) in cases {
        assert_eq!(
            is_near_square_pixel_sar(value),
            expected,
            "sample aspect ratio {value:?}"
        );
    }
}

#[test]
fn estimated_audio_bitrate_matrix_matches_official_cases() {
    let cases = [
        ("aac", None, Some(2), Some(192_000)),
        ("mp3", None, Some(2), Some(192_000)),
        ("mp2", None, Some(2), Some(192_000)),
        ("aac", None, Some(6), Some(320_000)),
        ("ac3", None, Some(2), Some(192_000)),
        ("eac3", None, Some(6), Some(640_000)),
        ("opus", None, Some(2), Some(128_000)),
        ("vorbis", None, Some(6), Some(320_000)),
        ("wmav2", None, Some(2), Some(192_000)),
        ("dts", None, Some(2), Some(768_000)),
        ("dts", Some("DTS"), Some(6), Some(1_509_000)),
        ("dts", Some("DTS-HD HRA"), Some(8), Some(1_509_000)),
        ("dts", Some("DTS-HD MA"), Some(6), Some(4_200_000)),
        ("dts", Some("DTS-HD MA + DTS:X"), Some(8), Some(5_600_000)),
        ("flac", None, Some(2), Some(960_000)),
        ("flac", None, Some(6), Some(2_880_000)),
        ("flac", None, Some(8), Some(3_840_000)),
        ("alac", None, Some(6), Some(2_880_000)),
        ("truehd", None, Some(2), Some(1_400_000)),
        ("truehd", None, Some(6), Some(4_200_000)),
        (
            "truehd",
            Some("Dolby TrueHD + Dolby Atmos"),
            Some(8),
            Some(5_600_000),
        ),
        ("aac", None, Some(3), Some(320_000)),
        ("ac3", None, Some(4), Some(640_000)),
        ("AAC", None, Some(2), Some(192_000)),
        ("pcm_s16le", None, Some(2), None),
        ("aac", None, None, None),
    ];
    for (codec, profile, channels, expected) in cases {
        assert_eq!(
            estimated_audio_bitrate(codec, profile, channels),
            expected,
            "audio codec={codec}, profile={profile:?}, channels={channels:?}"
        );
    }
}

#[test]
fn malformed_rationals_and_overflowing_channel_estimates_are_rejected() {
    assert_eq!(frame_rate("25"), None);
    assert_eq!(frame_rate("25/1/2"), None);
    assert_eq!(frame_rate("abc/1"), None);
    assert!(!is_near_square_pixel_sar(Some("1:0")));
    assert!(!is_near_square_pixel_sar(Some("square")));
    assert_eq!(estimated_audio_bitrate("flac", None, Some(u32::MAX)), None);
}
