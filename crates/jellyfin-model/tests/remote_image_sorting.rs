use jellyfin_model::{RemoteImageInfo, order_by_language_descending};

#[test]
fn preferred_language_is_first() {
    let result = order_by_language_descending(
        [
            image(Some("en"), 5.0, 100),
            image(Some("de"), 9.0, 200),
            image(None, 7.0, 50),
            image(Some("fr"), 8.0, 150),
        ],
        Some("de"),
    );

    assert_eq!(
        languages(&result),
        [Some("de"), Some("en"), None, Some("fr")]
    );
}

#[test]
fn english_is_before_no_language_even_with_lower_rating() {
    let result = order_by_language_descending(
        [image(None, 9.0, 500), image(Some("en"), 3.0, 10)],
        Some("de"),
    );

    assert_eq!(languages(&result), [Some("en"), None]);
}

#[test]
fn same_language_is_sorted_by_rating_then_vote_count() {
    let result = order_by_language_descending(
        [
            image(Some("de"), 5.0, 100),
            image(Some("de"), 9.0, 50),
            image(Some("de"), 9.0, 200),
        ],
        Some("de"),
    );

    assert_eq!(result[0].vote_count, Some(200));
    assert_eq!(result[1].vote_count, Some(50));
    assert_eq!(result[2].vote_count, Some(100));
}

#[test]
fn missing_requested_language_defaults_to_english() {
    let result = order_by_language_descending(
        [image(Some("fr"), 9.0, 500), image(Some("en"), 5.0, 10)],
        None,
    );

    assert_eq!(languages(&result), [Some("en"), Some("fr")]);
}

#[test]
fn requesting_english_does_not_double_boost_it() {
    let result = order_by_language_descending(
        [
            image(None, 9.0, 500),
            image(Some("en"), 3.0, 10),
            image(Some("fr"), 8.0, 300),
        ],
        Some("en"),
    );

    assert_eq!(languages(&result), [Some("en"), None, Some("fr")]);
}

#[test]
fn full_language_priority_order_matches_official_behavior() {
    let result = order_by_language_descending(
        [
            image(Some("fr"), 9.0, 500),
            image(None, 8.0, 400),
            image(Some("en"), 7.0, 300),
            image(Some("de"), 6.0, 200),
        ],
        Some("de"),
    );

    assert_eq!(
        languages(&result),
        [Some("de"), Some("en"), None, Some("fr")]
    );
}

fn image(language: Option<&str>, community_rating: f64, vote_count: i32) -> RemoteImageInfo {
    RemoteImageInfo {
        language: language.map(str::to_owned),
        community_rating: Some(community_rating),
        vote_count: Some(vote_count),
        ..RemoteImageInfo::default()
    }
}

fn languages(images: &[RemoteImageInfo]) -> Vec<Option<&str>> {
    images
        .iter()
        .map(|image| image.language.as_deref())
        .collect()
}
