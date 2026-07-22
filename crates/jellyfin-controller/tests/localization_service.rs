use jellyfin_controller::LocalizationService;
use jellyfin_model::ParentalRatingScore;

fn score(value: i32, sub_score: Option<i32>) -> ParentalRatingScore {
    ParentalRatingScore {
        score: value,
        sub_score,
    }
}

#[test]
fn countries_match_the_official_resource() {
    let countries = LocalizationService.countries();
    assert_eq!(countries.len(), 140);

    let germany = countries
        .iter()
        .find(|country| country.name == "DE")
        .expect("Germany must be present");
    assert_eq!(germany.display_name, "Germany");
    assert_eq!(germany.three_letter_iso_region_name, "DEU");
    assert_eq!(germany.two_letter_iso_region_name, "DE");
}

#[test]
fn cultures_include_every_official_iso_row() {
    let service = LocalizationService;
    let cultures = service.cultures();
    assert_eq!(cultures.len(), 496);
    assert_eq!(service.distinct_sorted_cultures().len(), 494);

    let german = cultures
        .iter()
        .find(|culture| culture.two_letter_iso_language_name == "de")
        .expect("German must be present");
    assert_eq!(
        german.three_letter_iso_language_name.as_deref(),
        Some("deu")
    );
    assert_eq!(german.display_name, "German");
    assert_eq!(german.name, "German");
    assert!(
        german
            .three_letter_iso_language_names
            .contains(&"deu".into())
    );
    assert!(
        german
            .three_letter_iso_language_names
            .contains(&"ger".into())
    );
}

#[test]
fn translates_iso6392_bibliographic_codes_to_terminologic_codes() {
    let service = LocalizationService;
    assert_eq!(service.try_get_iso6392_t_from_b("ger"), Some("deu"));
    assert_eq!(service.try_get_iso6392_t_from_b("CHI"), Some("zho"));
    assert_eq!(service.try_get_iso6392_t_from_b("eng"), None);
}

#[test]
fn finds_languages_by_every_official_identifier() {
    let service = LocalizationService;
    for identifier in ["de", "deu", "ger", "german"] {
        let german = service
            .find_language_info(identifier)
            .unwrap_or_else(|| panic!("{identifier} must resolve"));
        assert_eq!(
            german.three_letter_iso_language_name.as_deref(),
            Some("deu")
        );
        assert_eq!(german.display_name, "German");
        assert_eq!(german.name, "German");
        assert!(
            german
                .three_letter_iso_language_names
                .contains(&"deu".into())
        );
        assert!(
            german
                .three_letter_iso_language_names
                .contains(&"ger".into())
        );
    }

    for (identifier, display_name) in [
        ("mul", "Multiple languages"),
        ("und", "Undetermined"),
        ("mis", "Uncoded languages"),
        ("zxx", "No linguistic content; Not applicable"),
    ] {
        let culture = service
            .find_language_info(identifier)
            .unwrap_or_else(|| panic!("{identifier} must resolve"));
        assert_eq!(culture.display_name, display_name);
        assert_eq!(
            culture.three_letter_iso_language_name.as_deref(),
            Some(identifier)
        );
    }
}

#[test]
fn language_display_names_are_truncated_at_the_first_delimiter() {
    let service = LocalizationService;
    for (language, expected) in [
        ("ell", "Greek"),
        ("nld", "Dutch"),
        ("ron", "Romanian"),
        ("eng", "English"),
        ("zh-CN", "Chinese (Simplified)"),
    ] {
        assert_eq!(
            service.language_display_name(Some(language)).as_deref(),
            Some(expected)
        );
    }

    assert_eq!(service.language_display_name(None), None);
    assert_eq!(service.language_display_name(Some("")), None);
    assert_eq!(service.language_display_name(Some("xyz")), None);
}

#[test]
fn parental_rating_lists_match_the_official_country_systems() {
    let service = LocalizationService;
    let us_ratings = service.parental_ratings("US");
    assert_eq!(us_ratings.len(), 56);
    assert_eq!(
        us_ratings
            .iter()
            .find(|rating| rating.name == "TV-MA")
            .and_then(|rating| rating.rating_score),
        Some(score(17, Some(1)))
    );

    let de_ratings = service.parental_ratings("DE");
    assert_eq!(de_ratings.len(), 24);
    assert_eq!(
        de_ratings
            .iter()
            .find(|rating| rating.name == "FSK-12")
            .and_then(|rating| rating.rating_score),
        Some(score(12, None))
    );
}

#[test]
fn resolves_valid_rating_strings() {
    let service = LocalizationService;
    for (rating, country, expected) in [
        ("CA-R", "CA", score(18, Some(1))),
        ("FSK-16", "DE", score(16, None)),
        ("FSK-18", "DE", score(18, None)),
        ("FSK-18", "US", score(18, None)),
        ("TV-MA", "US", score(17, Some(1))),
        ("XXX", "asdf", score(1000, None)),
        ("Germany: FSK-18", "DE", score(18, None)),
        ("Rated : R", "US", score(17, Some(0))),
        ("Rated: R", "US", score(17, Some(0))),
        ("Rated R", "US", score(17, Some(0))),
        (" PG-13 ", "US", score(13, Some(0))),
    ] {
        assert_eq!(
            service.rating_score(rating, country, None),
            Some(expected),
            "failed to resolve {rating} for {country}"
        );
    }
}

#[test]
fn parses_numeric_rating_scores() {
    let service = LocalizationService;
    for value in [0, 1, 6, 12, 42, 9999] {
        let rating = value.to_string();
        assert_eq!(
            service.rating_score(&rating, "nl", None),
            Some(score(value, None))
        );
    }
    assert_eq!(
        service.rating_score("18+", "nl", None),
        Some(score(18, None))
    );
}

#[test]
fn unrated_and_empty_split_values_do_not_resolve() {
    let service = LocalizationService;
    for rating in [
        "NR",
        "unrated",
        "Not Rated",
        "n/a",
        "-NO RATING SHOWN-",
        ":NO RATING SHOWN:",
    ] {
        assert_eq!(service.rating_score(rating, "US", None), None);
    }
}

#[test]
fn fallback_prioritizes_the_us_rating_system() {
    let service = LocalizationService;
    for (rating, country, expected) in [
        ("TV-MA", "DE", score(17, Some(1))),
        ("PG-13", "FR", score(13, Some(0))),
        ("R", "JP", score(17, Some(0))),
    ] {
        assert_eq!(service.rating_score(rating, country, None), Some(expected));
    }
}

#[test]
fn known_country_prefixes_do_not_fall_through_to_other_systems() {
    let service = LocalizationService;
    for rating in ["US:INVALID", "us:INVALID", "DE-INVALID", "ca:INVALID"] {
        assert_eq!(service.rating_score(rating, "US", None), None);
    }
}

#[test]
fn country_prefixes_select_their_own_rating_system() {
    let service = LocalizationService;
    for (rating, configured_country, expected) in [
        ("us:R", "DE", score(17, Some(0))),
        ("US:PG-13", "DE", score(13, Some(0))),
        ("ca:R", "US", score(18, Some(1))),
    ] {
        assert_eq!(
            service.rating_score(rating, configured_country, None),
            Some(expected)
        );
    }
}

#[test]
fn multiple_provider_ratings_use_the_first_resolvable_value() {
    let service = LocalizationService;
    assert_eq!(
        service.rating_score("INVALID / SE:15 / 18", "US", None),
        Some(score(15, None))
    );
    assert_eq!(
        service.rating_score("R", "DE", Some("CA")),
        Some(score(18, Some(1)))
    );
}

#[test]
fn localized_strings_match_embedded_official_resources_and_fallbacks() {
    let service = LocalizationService;
    for (key, expected) in [("Default", "Default"), ("HeaderLiveTV", "Live TV")] {
        assert_eq!(
            service.localized_string(key, Some("en-US"), "en-US"),
            expected
        );
    }

    let invalid = "SuperInvalidTranslationKeyThatWillNeverBeAdded";
    assert_eq!(
        service.localized_string(invalid, Some("en-US"), "en-US"),
        invalid
    );
    assert_eq!(
        service.localized_string("Artists", Some("de"), "en-US"),
        "Interpreten"
    );
    assert_eq!(
        service.localized_string("Artists", Some("zz"), "en-US"),
        "Artists"
    );
}

#[test]
fn bcp47_codes_map_to_case_sensitive_underscore_resources() {
    let service = LocalizationService;
    assert_ne!(
        service.localized_string("Default", Some("es-419"), "en-US"),
        "Default"
    );
    assert_eq!(
        service.localized_string("Books", Some("he-IL"), "en-US"),
        "ספרים"
    );
    assert_eq!(
        service.localized_string("Books", Some("HE-il"), "en-US"),
        "ספרים"
    );
}

#[test]
fn server_and_current_ui_cultures_are_resolved_independently() {
    let service = LocalizationService;
    assert_eq!(
        service.server_localized_string("Artists", "de"),
        "Interpreten"
    );
    assert_eq!(
        service.localized_string("Artists", Some("de"), "en-US"),
        "Interpreten"
    );
    assert_eq!(
        service.localized_string("Artists", None, "de"),
        "Interpreten"
    );
    assert_eq!(service.localized_string("Artists", Some(""), ""), "Artists");
}

#[test]
fn ui_options_and_supported_cultures_are_complete_and_stable() {
    let service = LocalizationService;
    let options = service.localization_options();
    assert_eq!(options.len(), 105);
    assert!(
        options
            .windows(2)
            .all(|pair| { pair[0].name.to_lowercase() <= pair[1].name.to_lowercase() })
    );
    assert_eq!(
        options
            .iter()
            .find(|option| option.value == "en-US")
            .map(|option| option.name.as_str()),
        Some("English")
    );
    assert!(
        options.iter().any(|option| {
            option.value == "es_419" && option.name == "español latinoamericano"
        })
    );
    for novelty in ["jbo", "pr"] {
        assert!(
            options
                .iter()
                .any(|option| option.value == novelty && option.name == novelty)
        );
    }

    let supported = service.supported_ui_cultures();
    for culture in ["de", "en-US", "fr", "es-419"] {
        assert!(
            supported
                .iter()
                .any(|value| value.eq_ignore_ascii_case(culture)),
            "missing supported UI culture {culture}"
        );
    }
    assert!(!supported.iter().any(|culture| culture == "es_419"));
    assert!(!supported.iter().any(|culture| culture == "jbo"));
    assert!(!supported.iter().any(|culture| culture == "pr"));
}
