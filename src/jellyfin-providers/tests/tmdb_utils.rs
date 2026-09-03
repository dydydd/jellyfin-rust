use jellyfin_providers::tmdb::{TmdbPersonKind, TmdbUtils};

#[test]
fn normalize_language_official_matrix() {
    for (input, expected) in [
        ("de", "de"),
        ("En", "En"),
        ("de-de", "de-DE"),
        ("en-US", "en-US"),
        ("de-CH", "de"),
    ] {
        assert_eq!(
            TmdbUtils::normalize_language(Some(input), None).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(TmdbUtils::normalize_language(None, None), None);
    assert_eq!(
        TmdbUtils::normalize_language(Some(""), None).as_deref(),
        Some("")
    );
}

#[test]
fn normalize_language_region_matrix() {
    for (language, country, expected) in [
        ("es-419", Some("AR"), "es-AR"),
        ("ES-419", Some("ar"), "es-AR"),
        ("es-419", Some("MX"), "es-MX"),
        ("es-419", Some("US"), "es-MX"),
        ("es-419", Some(""), "es-419"),
        ("es-419", None, "es-419"),
        ("fr-ch", None, "fr"),
        ("it-CH", None, "it"),
        ("zh-hans-CN", None, "zh-hans-CN"),
    ] {
        assert_eq!(
            TmdbUtils::normalize_language(Some(language), country).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn adjust_image_language_official_matrix() {
    for (image, request, expected) in [
        (Some("en"), "en-US", "en-US"),
        (Some("fr-CA"), "fr-BE", "fr-CA"),
        (Some("fr-CA"), "fr", "fr-CA"),
        (Some("de"), "en-US", "de"),
        (Some(""), "en-US", ""),
    ] {
        assert_eq!(TmdbUtils::adjust_image_language(image, request), expected);
    }
}

#[test]
fn adjust_image_language_handles_missing_and_language_neutral_images() {
    for (image, request, expected) in [
        (None, "en-US", ""),
        (Some("xx"), "en-US", ""),
        (Some("XX"), "", ""),
        (Some("EN"), "en-US", "en-US"),
        (Some("en"), "", "en"),
    ] {
        assert_eq!(TmdbUtils::adjust_image_language(image, request), expected);
    }
}

#[test]
fn builds_image_language_fallback_parameter() {
    for (language, country, expected) in [
        ("", None, "null,en"),
        ("en", None, "en,null"),
        ("EN", None, "EN,null"),
        ("en-US", None, "en-US,null,en"),
        ("de-de", None, "de-DE,null,en"),
        ("de-CH", None, "de,null,en"),
        ("es-419", Some("AR"), "es-AR,null,en"),
    ] {
        assert_eq!(
            TmdbUtils::image_languages_param(language, country),
            expected
        );
    }
}

#[test]
fn cleans_names_for_tmdb_searches() {
    for (name, expected) in [
        ("Spider-Man: Homecoming", "Spider Man Homecoming"),
        ("Mr._Robot", "Mr Robot"),
        ("Wall·E", "Wall·E"),
        ("Amélie / 天使", "Amélie 天使"),
        (" leading...and trailing!", " leading and trailing "),
    ] {
        assert_eq!(TmdbUtils::clean_name(name), expected);
    }
}

#[test]
fn maps_wanted_crew_roles() {
    for (department, job, expected) in [
        (
            Some("Directing"),
            Some("Director"),
            TmdbPersonKind::Director,
        ),
        (
            Some("production"),
            Some("PRODUCER"),
            TmdbPersonKind::Producer,
        ),
        (Some("Writing"), Some("Writer"), TmdbPersonKind::Writer),
        (Some("writing"), Some("screenplay"), TmdbPersonKind::Writer),
        (Some("WRITING"), Some("Novel"), TmdbPersonKind::Writer),
        (Some("Writing"), Some("Story"), TmdbPersonKind::Unknown),
        (
            Some("Directing"),
            Some("Assistant Director"),
            TmdbPersonKind::Unknown,
        ),
        (None, None, TmdbPersonKind::Unknown),
    ] {
        assert_eq!(
            TmdbUtils::map_crew_to_person_kind(department, job),
            expected
        );
    }
}

#[test]
fn identifies_youtube_trailers_and_teasers() {
    for (site, video_type, expected) in [
        (Some("YouTube"), Some("Trailer"), true),
        (Some("youtube"), Some("TEASER"), true),
        (Some("Vimeo"), Some("Trailer"), false),
        (Some("YouTube"), Some("Clip"), false),
        (None, Some("Trailer"), false),
        (Some("YouTube"), None, false),
    ] {
        assert_eq!(TmdbUtils::is_trailer_type(site, video_type), expected);
    }
}

#[test]
fn builds_parental_ratings() {
    for (country, rating, expected) in [
        ("US", "TV-14", "TV-14"),
        ("us", "R", "R"),
        ("DE", "16", "FSK-16"),
        ("de", "12", "FSK-12"),
        ("GB", "15", "GB-15"),
        ("US", "DE-16", "FSK-16"),
        ("", "PG", "-PG"),
    ] {
        assert_eq!(TmdbUtils::build_parental_rating(country, rating), expected);
    }
}

#[test]
fn builds_offline_image_urls() {
    for (size, path, expected) in [
        (
            Some("w500"),
            Some("/abc123.jpg"),
            Some("https://image.tmdb.org/t/p/w500/abc123.jpg"),
        ),
        (
            Some("original"),
            Some("profile.png"),
            Some("https://image.tmdb.org/t/p/original/profile.png"),
        ),
        (
            None,
            Some("/poster.jpg"),
            Some("https://image.tmdb.org/t/p/original/poster.jpg"),
        ),
        (
            Some(""),
            Some("/poster.jpg"),
            Some("https://image.tmdb.org/t/p/original/poster.jpg"),
        ),
        (Some("w500"), None, None),
        (Some("w500"), Some(""), None),
    ] {
        assert_eq!(TmdbUtils::image_url(size, path).as_deref(), expected);
    }
}

#[test]
fn exposes_official_provider_constants() {
    assert_eq!(TmdbUtils::BASE_TMDB_URL, "https://www.themoviedb.org/");
    assert_eq!(TmdbUtils::PROVIDER_NAME, "TheMovieDb");
}
