use jellyfin_naming::{Format3dParser, NamingOptions, VideoResolver};

#[test]
fn kodi_format_3d_official_matrix() {
    let options = NamingOptions::default();
    for (input, is_3d, format) in [
        ("Super movie.3d.mp4", false, None),
        ("Super movie.3d.hsbs.mp4", true, Some("hsbs")),
        ("Super movie.3d.sbs.mp4", true, Some("sbs")),
        ("Super movie.3d.htab.mp4", true, Some("htab")),
        ("Super movie.3d.tab.mp4", true, Some("tab")),
        ("Super movie 3d hsbs.mp4", true, Some("hsbs")),
    ] {
        assert_format(input, is_3d, format, &options);
    }
}

#[test]
fn resolved_video_has_3d_format_and_clean_name() {
    let result = VideoResolver::resolve_file(
        Some("C:/Users/media/Desktop/Video Test/Movies/Oblivion/Oblivion.3d.hsbs.mkv"),
        &NamingOptions::default(),
    )
    .unwrap();
    assert_eq!(result.format_3d.as_deref(), Some("hsbs"));
    assert_eq!(result.name, "Oblivion");
}

#[test]
fn expanded_format_3d_official_matrix() {
    let options = NamingOptions::default();
    for (input, is_3d, format) in [
        ("Super movie.3d.mp4", false, None),
        ("Super movie.3d.hsbs.mp4", true, Some("hsbs")),
        ("Super movie.3d.sbs.mp4", true, Some("sbs")),
        ("Super movie.3d.htab.mp4", true, Some("htab")),
        ("Super movie.3d.tab.mp4", true, Some("tab")),
        ("Super movie.hsbs.mp4", true, Some("hsbs")),
        ("Super movie.sbs.mp4", true, Some("sbs")),
        ("Super movie.htab.mp4", true, Some("htab")),
        ("Super movie.tab.mp4", true, Some("tab")),
        ("Super movie.sbs3d.mp4", true, Some("sbs3d")),
        ("Super movie.3d.mvc.mp4", true, Some("mvc")),
        ("Super movie [3d].mp4", false, None),
        ("Super movie [hsbs].mp4", true, Some("hsbs")),
        ("Super movie [fsbs].mp4", true, Some("fsbs")),
        ("Super movie [ftab].mp4", true, Some("ftab")),
        ("Super movie [htab].mp4", true, Some("htab")),
        ("Super movie [sbs3d].mp4", true, Some("sbs3d")),
    ] {
        assert_format(input, is_3d, format, &options);
    }
}

#[test]
fn preceding_token_rule_latches_after_non_adjacent_tokens() {
    let options = NamingOptions::default();
    for (input, format) in [
        ("Super movie 3d 1080p hsbs.mp4", Some("hsbs")),
        ("Super movie 3d whatever sbs.mp4", Some("sbs")),
        ("Super movie 3d htab.mp4", Some("htab")),
        ("Super movie 3d tab.mp4", Some("tab")),
    ] {
        let result = Format3dParser::parse(input, &options);
        assert!(result.is_3d, "input={input:?}");
        assert_eq!(result.format_3d.as_deref(), format, "input={input:?}");
    }
}

fn assert_format(input: &str, is_3d: bool, format: Option<&str>, options: &NamingOptions) {
    let result = Format3dParser::parse(input, options);
    assert_eq!(result.is_3d, is_3d, "input={input:?}");
    assert_eq!(result.format_3d.as_deref(), format, "input={input:?}");
}
