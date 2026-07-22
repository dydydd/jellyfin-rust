use jellyfin_controller::library::{InternalItemsQuery, InternalItemsQueryError, ItemFilter};

macro_rules! conflicting_filter_test {
    ($name:ident, $first:ident, $second:ident) => {
        #[test]
        fn $name() {
            let mut query = InternalItemsQuery::default();

            assert!(matches!(
                query.apply_filters(&[ItemFilter::$first, ItemFilter::$second]),
                Err(InternalItemsQueryError::ConflictingFilters { .. })
            ));
        }
    };
}

conflicting_filter_test!(
    apply_filters_rejects_folder_and_not_folder,
    IsFolder,
    IsNotFolder
);
conflicting_filter_test!(
    apply_filters_rejects_played_and_unplayed,
    IsPlayed,
    IsUnplayed
);
conflicting_filter_test!(apply_filters_rejects_likes_and_dislikes, Likes, Dislikes);

#[test]
fn apply_filters_maps_every_supported_filter_to_query_criteria() {
    let cases = [
        (ItemFilter::IsFolder, Some(true), None, None),
        (ItemFilter::IsNotFolder, Some(false), None, None),
        (ItemFilter::IsPlayed, None, Some(true), None),
        (ItemFilter::IsUnplayed, None, Some(false), None),
        (ItemFilter::Likes, None, None, Some(true)),
        (ItemFilter::Dislikes, None, None, Some(false)),
    ];

    for (filter, expected_folder, expected_played, expected_liked) in cases {
        let mut query = InternalItemsQuery::default();
        query.apply_filters(&[filter]).unwrap();

        assert_eq!(query.is_folder, expected_folder);
        assert_eq!(query.is_played, expected_played);
        assert_eq!(query.is_liked, expected_liked);
        assert!(query.has_filters());
    }

    let mut query = InternalItemsQuery::default();
    query
        .apply_filters(&[
            ItemFilter::IsFavorite,
            ItemFilter::IsResumable,
            ItemFilter::IsFavoriteOrLikes,
        ])
        .unwrap();

    assert_eq!(query.is_favorite, Some(true));
    assert_eq!(query.is_resumable, Some(true));
    assert_eq!(query.is_favorite_or_liked, Some(true));
    assert!(query.has_filters());
}
