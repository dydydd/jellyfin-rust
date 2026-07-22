use jellyfin_model::{MimeTypeError, MimeTypes};

#[test]
fn get_mime_type_matches_official_matrix() {
    let cases = [
        (".cb7", "application/x-cb7"),
        (".cba", "application/x-cba"),
        (".cbr", "application/vnd.comicbook-rar"),
        (".cbt", "application/x-cbt"),
        (".cbz", "application/vnd.comicbook+zip"),
        (".dll", "application/octet-stream"),
        (".log", "text/plain"),
        (".srt", "application/x-subrip"),
        (".html", "text/html; charset=UTF-8"),
        (".htm", "text/html; charset=UTF-8"),
        (".7z", "application/x-7z-compressed"),
        (".azw", "application/vnd.amazon.ebook"),
        (".azw3", "application/vnd.amazon.ebook"),
        (".eot", "application/vnd.ms-fontobject"),
        (".epub", "application/epub+zip"),
        (".json", "application/json"),
        (".mobi", "application/x-mobipocket-ebook"),
        (".opf", "application/oebps-package+xml"),
        (".pdf", "application/pdf"),
        (".rar", "application/vnd.rar"),
        (".ttml", "application/ttml+xml"),
        (".wasm", "application/wasm"),
        (".xml", "application/xml"),
        (".zip", "application/zip"),
        (".bmp", "image/bmp"),
        (".gif", "image/gif"),
        (".ico", "image/vnd.microsoft.icon"),
        (".jpg", "image/jpeg"),
        (".jpeg", "image/jpeg"),
        (".png", "image/png"),
        (".svg", "image/svg+xml"),
        (".svgz", "image/svg+xml"),
        (".tbn", "image/jpeg"),
        (".tif", "image/tiff"),
        (".tiff", "image/tiff"),
        (".webp", "image/webp"),
        (".ttf", "font/ttf"),
        (".woff", "font/woff"),
        (".woff2", "font/woff2"),
        (".ass", "text/x-ssa"),
        (".ssa", "text/x-ssa"),
        (".css", "text/css"),
        (".csv", "text/csv"),
        (".edl", "text/plain"),
        (".txt", "text/plain"),
        (".vtt", "text/vtt"),
        (".3gp", "video/3gpp"),
        (".3g2", "video/3gpp2"),
        (".asf", "video/x-ms-asf"),
        (".avi", "video/x-msvideo"),
        (".flv", "video/x-flv"),
        (".mp4", "video/mp4"),
        (".m4v", "video/x-m4v"),
        (".mpegts", "video/mp2t"),
        (".mpg", "video/mpeg"),
        (".mkv", "video/x-matroska"),
        (".mov", "video/quicktime"),
        (".ogv", "video/ogg"),
        (".ts", "video/mp2t"),
        (".webm", "video/webm"),
        (".wmv", "video/x-ms-wmv"),
        (".aac", "audio/aac"),
        (".ac3", "audio/ac3"),
        (".ape", "audio/x-ape"),
        (".dsf", "audio/dsf"),
        (".dsp", "audio/dsp"),
        (".flac", "audio/flac"),
        (".m4a", "audio/mp4"),
        (".m4b", "audio/mp4"),
        (".mid", "audio/midi"),
        (".midi", "audio/midi"),
        (".mp3", "audio/mpeg"),
        (".oga", "audio/ogg"),
        (".ogg", "audio/ogg"),
        (".opus", "audio/ogg"),
        (".vorbis", "audio/vorbis"),
        (".wav", "audio/wav"),
        (".webma", "audio/webm"),
        (".wma", "audio/x-ms-wma"),
        (".wv", "audio/x-wavpack"),
        (".xsp", "audio/xsp"),
    ];

    assert_eq!(cases.len(), 81);
    let mut mismatches = Vec::new();
    for (input, expected) in cases {
        let actual = MimeTypes::get_mime_type_or(input, None).unwrap();
        if actual.as_deref() != Some(expected) {
            mismatches.push(format!("{input}: expected {expected}, got {actual:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn to_extension_matches_official_matrix() {
    let cases = [
        ("application/epub+zip", ".epub"),
        ("application/json", ".json"),
        ("application/oebps-package+xml", ".opf"),
        ("application/pdf", ".pdf"),
        ("application/ttml+xml", ".ttml"),
        ("application/vnd.amazon.ebook", ".azw"),
        ("application/vnd.comicbook-rar", ".cbr"),
        ("application/vnd.comicbook+zip", ".cbz"),
        ("application/vnd.ms-fontobject", ".eot"),
        ("application/vnd.rar", ".rar"),
        ("application/wasm", ".wasm"),
        ("application/x-7z-compressed", ".7z"),
        ("application/x-cb7", ".cb7"),
        ("application/x-cba", ".cba"),
        ("application/x-cbr", ".cbr"),
        ("application/x-cbt", ".cbt"),
        ("application/x-cbz", ".cbz"),
        ("application/x-javascript", ".js"),
        ("application/x-mobipocket-ebook", ".mobi"),
        ("application/x-mpegURL", ".m3u8"),
        ("application/x-subrip", ".srt"),
        ("application/xml", ".xml"),
        ("application/zip", ".zip"),
        ("audio/aac", ".aac"),
        ("audio/ac3", ".ac3"),
        ("audio/dsf", ".dsf"),
        ("audio/dsp", ".dsp"),
        ("audio/flac", ".flac"),
        ("audio/m4b", ".m4b"),
        ("audio/mp4", ".m4a"),
        ("audio/vorbis", ".vorbis"),
        ("audio/wav", ".wav"),
        ("audio/x-aac", ".aac"),
        ("audio/x-ape", ".ape"),
        ("audio/x-ms-wma", ".wma"),
        ("audio/x-wavpack", ".wv"),
        ("audio/xsp", ".xsp"),
        ("font/ttf", ".ttf"),
        ("font/woff", ".woff"),
        ("font/woff2", ".woff2"),
        ("image/bmp", ".bmp"),
        ("image/gif", ".gif"),
        ("image/jpeg", ".jpg"),
        ("image/png", ".png"),
        ("image/svg+xml", ".svg"),
        ("image/tiff", ".tiff"),
        ("image/vnd.microsoft.icon", ".ico"),
        ("image/webp", ".webp"),
        ("image/x-icon", ".ico"),
        ("image/x-png", ".png"),
        ("text/css", ".css"),
        ("text/csv", ".csv"),
        ("text/plain", ".txt"),
        ("text/rtf", ".rtf"),
        ("text/vtt", ".vtt"),
        ("text/x-ssa", ".ssa"),
        ("video/3gpp", ".3gp"),
        ("video/3gpp2", ".3g2"),
        ("video/mp2t", ".ts"),
        ("video/mp4", ".mp4"),
        ("video/ogg", ".ogv"),
        ("video/quicktime", ".mov"),
        ("video/vnd.mpeg.dash.mpd", ".mpd"),
        ("video/webm", ".webm"),
        ("video/x-flv", ".flv"),
        ("video/x-m4v", ".m4v"),
        ("video/x-matroska", ".mkv"),
        ("video/x-ms-asf", ".asf"),
        ("video/x-ms-wmv", ".wmv"),
        ("video/x-msvideo", ".avi"),
    ];

    assert_eq!(cases.len(), 70);
    let mut mismatches = Vec::new();
    for (input, expected) in cases {
        let actual = MimeTypes::to_extension(input).unwrap();
        if actual.as_deref() != Some(expected) {
            mismatches.push(format!("{input}: expected {expected}, got {actual:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn lookup_is_case_insensitive_and_honors_defaults() {
    assert_eq!(
        MimeTypes::get_mime_type_or("MOVIE.DIVX", None)
            .unwrap()
            .as_deref(),
        Some("video/DIVX")
    );
    assert_eq!(
        MimeTypes::get_mime_type_or("cover.TBN", None)
            .unwrap()
            .as_deref(),
        Some("image/jpeg")
    );
    assert_eq!(
        MimeTypes::get_mime_type_or("unknown.jellyfin", None).unwrap(),
        None
    );
    assert_eq!(
        MimeTypes::get_mime_type_or("unknown.jellyfin", Some("test/default")).unwrap(),
        Some("test/default".to_owned())
    );
    assert_eq!(
        MimeTypes::get_mime_type("unknown.jellyfin").unwrap(),
        "application/octet-stream"
    );
    assert_eq!(
        MimeTypes::get_mime_type("").unwrap_err(),
        MimeTypeError::EmptyValue
    );
}

#[test]
fn reverse_lookup_handles_case_parameters_unknowns_and_images() {
    assert_eq!(
        MimeTypes::to_extension("TEXT/HTML; charset=UTF-8")
            .unwrap()
            .as_deref(),
        Some(".htm")
    );
    assert_eq!(MimeTypes::to_extension("unknown/type").unwrap(), None);
    assert_eq!(
        MimeTypes::to_extension("").unwrap_err(),
        MimeTypeError::EmptyValue
    );
    assert!(MimeTypes::is_image("IMAGE/JPEG"));
    assert!(!MimeTypes::is_image("video/mp4"));
}
