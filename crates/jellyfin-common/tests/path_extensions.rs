use std::path::MAIN_SEPARATOR_STR;

use jellyfin_common::{
    normalize_path, normalize_path_default, normalize_path_with_detected_separator,
    try_replace_sub_path,
};

#[test]
fn try_replace_sub_path_valid_args_official_matrix() {
    let cases = [
        (
            "C:/Users/jeff/myfile.mkv",
            "C:/Users/jeff",
            "/home/jeff",
            "/home/jeff/myfile.mkv",
        ),
        (
            "C:/Users/jeff/myfile.mkv",
            "C:/Users/jeff/",
            "/home/jeff",
            "/home/jeff/myfile.mkv",
        ),
        (
            "/home/jeff/music/jeff's band/consistently inconsistent.mp3",
            "/home/jeff/music/jeff's band",
            "/home/not jeff",
            "/home/not jeff/consistently inconsistent.mp3",
        ),
        (
            r"C:\Users\jeff\myfile.mkv",
            r"C:\Users/jeff",
            "/home/jeff",
            "/home/jeff/myfile.mkv",
        ),
        (
            r"C:\Users\jeff\myfile.mkv",
            r"C:\Users/jeff",
            "/home/jeff/",
            "/home/jeff/myfile.mkv",
        ),
        (
            r"C:\Users\jeff\myfile.mkv",
            r"C:\Users/jeff/",
            "/home/jeff/",
            "/home/jeff/myfile.mkv",
        ),
        (
            r"C:\Users\jeff\myfile.mkv",
            r"C:\Users/jeff/",
            "/",
            "/myfile.mkv",
        ),
        ("/o", "/o", "/s", "/s"),
    ];

    for (path, sub_path, new_sub_path, expected) in cases {
        assert_eq!(
            try_replace_sub_path(Some(path), Some(sub_path), Some(new_sub_path)).as_deref(),
            Some(expected),
            "path={path:?}, sub_path={sub_path:?}, new_sub_path={new_sub_path:?}"
        );
    }
}

#[test]
fn try_replace_sub_path_invalid_input_official_matrix() {
    let cases = [
        (None, None, None),
        (None, Some("/my/path"), Some("/another/path")),
        (Some("/my/path"), None, Some("/another/path")),
        (Some("/my/path"), Some("/another/path"), None),
        (Some(""), Some(""), Some("")),
        (Some("/my/path"), Some(""), Some("")),
        (Some(""), Some("/another/path"), Some("")),
        (Some(""), Some(""), Some("/new/subpath")),
        (
            Some("/home/jeff/music/jeff's band/consistently inconsistent.mp3"),
            Some("/home/jeff/music/not jeff's band"),
            Some("/home/not jeff"),
        ),
    ];

    for (path, sub_path, new_sub_path) in cases {
        assert_eq!(try_replace_sub_path(path, sub_path, new_sub_path), None);
    }
}

#[test]
fn try_replace_sub_path_respects_directory_boundaries_and_unicode() {
    assert_eq!(
        try_replace_sub_path(
            Some("/media/tv-old/show"),
            Some("/media/tv"),
            Some("/srv/tv")
        ),
        None
    );
    assert_eq!(
        try_replace_sub_path(
            Some("/媒体/ÉMISSIONS/show.mkv"),
            Some("/媒体/émissions"),
            Some("/library/tv"),
        )
        .as_deref(),
        Some("/library/tv/show.mkv")
    );
}

#[test]
fn normalize_path_specifying_separator_official_matrix() {
    let cases = [
        (None, '/', None),
        (None, '\\', None),
        (
            Some("/home/jeff/myfile.mkv"),
            '\\',
            Some(r"\home\jeff\myfile.mkv"),
        ),
        (
            Some(r"C:\Users\Jeff\myfile.mkv"),
            '/',
            Some("C:/Users/Jeff/myfile.mkv"),
        ),
        (
            Some(r"\home/jeff\myfile.mkv"),
            '\\',
            Some(r"\home\jeff\myfile.mkv"),
        ),
        (
            Some(r"\home/jeff\myfile.mkv"),
            '/',
            Some("/home/jeff/myfile.mkv"),
        ),
        (Some(""), '/', Some("")),
    ];

    for (path, separator, expected) in cases {
        assert_eq!(
            normalize_path(path, separator).unwrap().as_deref(),
            expected
        );
    }
}

#[test]
fn normalize_path_default_uses_directory_separator_char() {
    let cases = [
        "/home/jeff/myfile.mkv",
        r"C:\Users\Jeff\myfile.mkv",
        r"\home/jeff\myfile.mkv",
    ];

    for path in cases {
        let expected = path.replace(['\\', '/'], MAIN_SEPARATOR_STR);
        assert_eq!(
            normalize_path_default(Some(path)).as_deref(),
            Some(expected.as_str())
        );
    }
    assert_eq!(normalize_path_default(None), None);
}

#[test]
fn normalize_path_with_detected_separator_official_matrix() {
    let cases = [
        ("/home/jeff/myfile.mkv", '/', "/home/jeff/myfile.mkv"),
        (
            r"C:\Users\Jeff\myfile.mkv",
            '\\',
            r"C:\Users\Jeff\myfile.mkv",
        ),
        (r"\home/jeff\myfile.mkv", '/', "/home/jeff/myfile.mkv"),
    ];

    for (path, expected_separator, expected_path) in cases {
        let (normalized, separator) = normalize_path_with_detected_separator(Some(path)).unwrap();
        assert_eq!(separator, Some(expected_separator));
        assert_eq!(normalized, expected_path);
    }

    assert_eq!(normalize_path_with_detected_separator(None), None);
    assert_eq!(
        normalize_path_with_detected_separator(Some("")),
        Some((String::new(), None))
    );
}

#[test]
fn normalize_path_rejects_invalid_separator_with_typed_error() {
    let error = normalize_path(Some(""), 'a').unwrap_err();
    assert_eq!(error.separator(), 'a');
    assert_eq!(
        error.to_string(),
        "'a' is not a directory separator; expected '/' or '\\\\'"
    );
}
