use jellyfin_naming::{
    ExtraResolver, ExtraRule, ExtraRuleType, ExtraType, MediaType, NamingOptions, VideoResolver,
};

fn assert_extra(path: &str, expected: Option<ExtraType>) {
    assert_eq!(
        ExtraResolver::resolve(path, &NamingOptions::default()).extra_type,
        expected,
        "unexpected extra type for {path}"
    );
}

fn assert_extra_with_root(path: &str, root: &str, expected: Option<ExtraType>) {
    assert_eq!(
        ExtraResolver::resolve_with_library_root(path, &NamingOptions::default(), Some(root))
            .extra_type,
        expected,
        "unexpected extra type for {path} with root {root}"
    );
}

#[test]
fn test_kodi_extras() {
    for path in [
        "trailer.mp4",
        "300-trailer.mp4",
        "300.trailer.mp4",
        "300_trailer.mp4",
        "300 - trailer.mp4",
    ] {
        assert_extra(path, Some(ExtraType::Trailer));
    }
    assert_extra("theme.mp3", Some(ExtraType::ThemeSong));
}

#[test]
fn test_expanded_extras() {
    let cases = [
        ("trailer.mp4", Some(ExtraType::Trailer)),
        ("trailer.mp3", None),
        ("300-trailer.mp4", Some(ExtraType::Trailer)),
        ("stuff trailerthings.mkv", None),
        ("theme.mp3", Some(ExtraType::ThemeSong)),
        ("theme.mkv", None),
        ("300-scene.mp4", Some(ExtraType::Scene)),
        ("300-scene2.mp4", Some(ExtraType::Scene)),
        ("300-clip.mp4", Some(ExtraType::Clip)),
        ("300-deleted.mp4", Some(ExtraType::DeletedScene)),
        ("300-deletedscene.mp4", Some(ExtraType::DeletedScene)),
        ("300-interview.mp4", Some(ExtraType::Interview)),
        ("300-behindthescenes.mp4", Some(ExtraType::BehindTheScenes)),
        ("300-featurette.mp4", Some(ExtraType::Featurette)),
        ("300-short.mp4", Some(ExtraType::Short)),
        ("300-extra.mp4", Some(ExtraType::Unknown)),
        ("300-other.mp4", Some(ExtraType::Unknown)),
    ];
    for (path, expected) in cases {
        assert_extra(path, expected);
    }
}

#[test]
fn test_directories_audio_extras() {
    let directory = "theme-music";
    for path in [
        format!("{directory}/300.mp3"),
        format!("300/{directory}/something.mp3"),
        format!("/data/something/Movies/300/{directory}/whoknows.mp3"),
    ] {
        assert_extra(&path, Some(ExtraType::ThemeSong));
    }
}

#[test]
fn test_directories_video_extras() {
    let cases = [
        (ExtraType::BehindTheScenes, "behind the scenes"),
        (ExtraType::DeletedScene, "deleted scenes"),
        (ExtraType::Interview, "interviews"),
        (ExtraType::Scene, "scenes"),
        (ExtraType::Sample, "samples"),
        (ExtraType::Short, "shorts"),
        (ExtraType::Trailer, "trailers"),
        (ExtraType::Featurette, "featurettes"),
        (ExtraType::Clip, "clips"),
        (ExtraType::ThemeVideo, "backdrops"),
        (ExtraType::Unknown, "extra"),
        (ExtraType::Unknown, "extras"),
        (ExtraType::Unknown, "other"),
    ];
    for (extra_type, directory) in cases {
        for path in [
            format!("{directory}/300.mp4"),
            format!("300/{directory}/something.mkv"),
            format!("/data/something/Movies/300/{directory}/whoknows.mp4"),
        ] {
            assert_extra(&path, Some(extra_type));
        }
    }
}

#[test]
fn test_non_extra_directories() {
    for directory in ["gibberish", "not a scene", "The Big Short"] {
        for path in [
            format!("{directory}/300.mp4"),
            format!("300/{directory}/something.mkv"),
            format!("/data/something/Movies/300/{directory}/whoknows.mp4"),
            format!("/data/something/Movies/{directory}/{directory}.mp4"),
        ] {
            assert_extra(&path, None);
        }
    }
}

#[test]
fn test_top_level_directories_with_audio_extra_names() {
    let directory = "theme-music";
    let root = format!("/data/something/{directory}");
    assert_extra_with_root(&format!("{root}/300.mp3"), &root, None);
    assert_extra_with_root(
        &format!("{root}/300/{directory}/something.mp3"),
        &root,
        Some(ExtraType::ThemeSong),
    );
}

#[test]
fn test_top_level_directories_with_video_extra_names() {
    let cases = [
        (ExtraType::Trailer, "trailers"),
        (ExtraType::ThemeVideo, "backdrops"),
        (ExtraType::BehindTheScenes, "behind the scenes"),
        (ExtraType::DeletedScene, "deleted scenes"),
        (ExtraType::Interview, "interviews"),
        (ExtraType::Scene, "scenes"),
        (ExtraType::Sample, "samples"),
        (ExtraType::Short, "shorts"),
        (ExtraType::Featurette, "featurettes"),
        (ExtraType::Unknown, "extras"),
        (ExtraType::Unknown, "extra"),
        (ExtraType::Unknown, "other"),
        (ExtraType::Clip, "clips"),
    ];
    for (extra_type, directory) in cases {
        let root = format!("/data/something/{directory}");
        assert_extra_with_root(&format!("{root}/300.mp4"), &root, None);
        assert_extra_with_root(
            &format!("{root}/300/{directory}/something.mkv"),
            &root,
            Some(extra_type),
        );
    }
}

#[test]
fn test_sample() {
    for path in [
        "sample.mp4",
        "300-sample.mp4",
        "300.sample.mp4",
        "300_sample.mp4",
        "300 - sample.mp4",
    ] {
        assert_extra(path, Some(ExtraType::Sample));
    }
}

#[test]
fn test_suffix_part_of_title() {
    assert_extra("I Live In A Trailer.mp4", None);
    assert_extra("The DNA Sample.mp4", None);
}

#[test]
fn test_extra_info_invalid_rule_type() {
    let rule = ExtraRule::new(
        ExtraType::Unknown,
        ExtraRuleType::Regex,
        r"([eE]x(tra)?\.\w+)",
        MediaType::Video,
    );
    let options = NamingOptions {
        video_extra_rules: vec![rule.clone()],
        ..NamingOptions::default()
    };
    let result = ExtraResolver::resolve("extra.mp4", &options);
    assert_eq!(result.extra_type, Some(ExtraType::Unknown));
    assert_eq!(result.rule, Some(rule));
}

#[test]
fn configured_rule_priority_uses_first_match() {
    let first = ExtraRule::new(
        ExtraType::Unknown,
        ExtraRuleType::Regex,
        r"trailer\.mp4$",
        MediaType::Video,
    );
    let second = ExtraRule::new(
        ExtraType::Trailer,
        ExtraRuleType::Filename,
        "trailer",
        MediaType::Video,
    );
    let options = NamingOptions {
        video_extra_rules: vec![first.clone(), second],
        ..NamingOptions::default()
    };
    let result = ExtraResolver::resolve("trailer.mp4", &options);
    assert_eq!(result.extra_type, Some(ExtraType::Unknown));
    assert_eq!(result.rule, Some(first.clone()));

    let video = VideoResolver::resolve_file(Some("trailer.mp4"), &options).expect("video");
    assert_eq!(video.extra_type, Some(ExtraType::Unknown));
    assert_eq!(video.extra_rule, Some(first));
}
