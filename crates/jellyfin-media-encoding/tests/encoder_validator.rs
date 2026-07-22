use jellyfin_media_encoding::encoder::{
    FfmpegVersion, ffmpeg_version, is_supported_ffmpeg_version,
};

const OFFICIAL_OUTPUTS: &str = include_str!("fixtures/encoder/EncoderValidatorTestsData.cs");

fn official_output(name: &str) -> &str {
    let marker = format!("public const string {name} = @\"");
    let remainder = OFFICIAL_OUTPUTS
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing official output {name}"))
        .1;
    remainder
        .split_once("\";")
        .unwrap_or_else(|| panic!("unterminated official output {name}"))
        .0
}

#[test]
fn ffmpeg_version_matrix_matches_official_cases() {
    let cases = [
        ("FFmpegV701Output", Some(FfmpegVersion::with_patch(7, 0, 1))),
        ("FFmpegV611Output", Some(FfmpegVersion::with_patch(6, 1, 1))),
        ("FFmpegV60Output", Some(FfmpegVersion::new(6, 0))),
        ("FFmpegV512Output", Some(FfmpegVersion::with_patch(5, 1, 2))),
        ("FFmpegV44Output", Some(FfmpegVersion::new(4, 4))),
        ("FFmpegV432Output", Some(FfmpegVersion::with_patch(4, 3, 2))),
        ("FFmpegGitUnknownOutput2", Some(FfmpegVersion::new(4, 4))),
        (
            "FFmpegGitWithoutLibpostprocOutput",
            Some(FfmpegVersion::new(4, 4)),
        ),
        ("FFmpegGitUnknownOutput", None),
    ];
    for (name, expected) in cases {
        assert_eq!(ffmpeg_version(official_output(name)), expected, "{name}");
    }
}

#[test]
fn supported_version_matrix_matches_official_cases() {
    let cases = [
        ("FFmpegV701Output", true),
        ("FFmpegV611Output", true),
        ("FFmpegV60Output", true),
        ("FFmpegV512Output", true),
        ("FFmpegV44Output", true),
        ("FFmpegV432Output", false),
        ("FFmpegGitUnknownOutput2", true),
        ("FFmpegGitWithoutLibpostprocOutput", true),
        ("FFmpegGitUnknownOutput", false),
    ];
    for (name, expected) in cases {
        assert_eq!(
            is_supported_ffmpeg_version(official_output(name)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn malformed_release_and_libav_outputs_are_rejected() {
    assert_eq!(ffmpeg_version("ffmpeg version 7."), None);
    assert_eq!(ffmpeg_version("prefix ffmpeg version 7.0"), None);
    assert!(!is_supported_ffmpeg_version(
        "ffmpeg version 7.0\nCopyright Libav developers"
    ));
}

#[test]
fn git_fallback_requires_every_core_library() {
    let missing_library = official_output("FFmpegGitWithoutLibpostprocOutput")
        .lines()
        .filter(|line| !line.starts_with("libavfilter"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(ffmpeg_version(&missing_library), None);
}
