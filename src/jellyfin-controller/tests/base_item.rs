use std::collections::{HashMap, HashSet};

use chrono::{TimeZone, Utc};
use jellyfin_controller::library::{
    UserItemData, VersionGroup, VersionPlaybackUpdate, VideoItem, get_common_version_prefix,
    get_media_source_name, modify_sort_chunks,
};
use uuid::Uuid;

macro_rules! sort_chunk_test {
    ($name:ident, $input:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_eq!(modify_sort_chunks($input), $expected);
        }
    };
}

sort_chunk_test!(modify_sort_chunks_empty, "", "");
sort_chunk_test!(modify_sort_chunks_one, "1", "0000000001");
sort_chunk_test!(modify_sort_chunks_single_letter, "t", "t");
sort_chunk_test!(modify_sort_chunks_text, "test", "test");
sort_chunk_test!(
    modify_sort_chunks_trailing_number,
    "test1",
    "test0000000001"
);
sort_chunk_test!(
    modify_sort_chunks_multiple_numbers,
    "1test 2",
    "0000000001test 0000000002"
);

macro_rules! folder_name_test {
    ($name:ident, $primary:literal, $alternate:literal, $expected_primary:literal, $expected_alternate:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                get_media_source_name($primary, true, None),
                $expected_primary
            );
            assert_eq!(
                get_media_source_name($alternate, true, None),
                $expected_alternate
            );
        }
    };
}

folder_name_test!(
    media_source_name_ted_folder,
    "/Movies/Ted/Ted.mp4",
    "/Movies/Ted/Ted - Unrated Edition.mp4",
    "Ted",
    "Unrated Edition"
);
folder_name_test!(
    media_source_name_deadpool_folder,
    "/Movies/Deadpool 2 (2018)/Deadpool 2 (2018).mkv",
    "/Movies/Deadpool 2 (2018)/Deadpool 2 (2018) - Super Duper Cut.mkv",
    "Deadpool 2 (2018)",
    "Super Duper Cut"
);

fn assert_common_prefix_names(
    primary: &str,
    alternate: &str,
    expected_primary: &str,
    expected_alternate: &str,
) {
    let prefix = get_common_version_prefix(&[primary, alternate]);
    let primary_path = format!("/Shows/Demo/Season 01/{primary}.mkv");
    let alternate_path = format!("/Shows/Demo/Season 01/{alternate}.mkv");

    assert_eq!(
        get_media_source_name(&primary_path, false, Some(&prefix)),
        expected_primary
    );
    assert_eq!(
        get_media_source_name(&alternate_path, false, Some(&prefix)),
        expected_alternate
    );
}

macro_rules! common_prefix_test {
    ($name:ident, $primary:literal, $alternate:literal, $expected_primary:literal, $expected_alternate:literal) => {
        #[test]
        fn $name() {
            assert_common_prefix_names(
                $primary,
                $alternate,
                $expected_primary,
                $expected_alternate,
            );
        }
    };
}

common_prefix_test!(
    common_prefix_episode_suffixes,
    "Spider-Noir - S01E02 - Wo ist Flint - Greyscale",
    "Spider-Noir - S01E02 - Wo ist Flint - Colorized",
    "Greyscale",
    "Colorized"
);
common_prefix_test!(
    common_prefix_bare_primary,
    "Spider-Noir - S01E02 - Wo ist Flint",
    "Spider-Noir - S01E02 - Wo ist Flint - Greyscale",
    "Spider-Noir - S01E02 - Wo ist Flint",
    "Greyscale"
);
common_prefix_test!(
    common_prefix_retreats_from_shared_word,
    "Demo - S01E01 - Greyscale",
    "Demo - S01E01 - Greyish",
    "Greyscale",
    "Greyish"
);
common_prefix_test!(
    common_prefix_underscore_separator,
    "Movie (2020)_4K",
    "Movie (2020)_1080p",
    "4K",
    "1080p"
);
common_prefix_test!(
    common_prefix_dot_separator,
    "Movie (2020).UHD",
    "Movie (2020).1080p",
    "UHD",
    "1080p"
);
common_prefix_test!(
    common_prefix_resolution_suffixes,
    "Movie - 1080p",
    "Movie - 1080i",
    "1080p",
    "1080i"
);
common_prefix_test!(
    common_prefix_keeps_shared_resolution,
    "movie (2020) - 2160p Extended",
    "movie (2020) - 2160p Original",
    "2160p Extended",
    "2160p Original"
);
common_prefix_test!(
    common_prefix_keeps_opening_bracket,
    "Blade Runner (1982) [Final Cut] [1080p HEVC AAC]",
    "Blade Runner (1982) [EE by ADM] [480p HEVC AAC]",
    "[Final Cut] [1080p HEVC AAC]",
    "[EE by ADM] [480p HEVC AAC]"
);

fn setup_version_group() -> (VersionGroup, Uuid, Uuid, Uuid) {
    let primary_id = Uuid::new_v4();
    let alternate1_id = Uuid::new_v4();
    let alternate2_id = Uuid::new_v4();

    let mut primary = VideoItem::new(primary_id, "/Movies/Movie/Movie.mkv");
    primary.width = Some(3_840);
    let mut alternate1 = VideoItem::new(alternate1_id, "/Movies/Movie/Movie - 1080p.mkv");
    alternate1.primary_version_id = Some(primary_id);
    alternate1.width = Some(1_920);
    let mut alternate2 = VideoItem::new(alternate2_id, "/Movies/Movie/Movie - 4K.mkv");
    alternate2.primary_version_id = Some(primary_id);
    alternate2.width = Some(1_920);

    let mut group = VersionGroup::new(primary);
    group.insert(alternate1).unwrap();
    group.insert(alternate2).unwrap();
    (group, primary_id, alternate1_id, alternate2_id)
}

#[test]
fn get_alternate_version_returns_matching_local_version() {
    let (group, primary, alternate1, alternate2) = setup_version_group();

    assert_eq!(
        group
            .alternate_version(primary, alternate1)
            .unwrap()
            .unwrap()
            .id,
        alternate1
    );
    assert_eq!(
        group
            .alternate_version(primary, alternate2)
            .unwrap()
            .unwrap()
            .id,
        alternate2
    );
    assert_eq!(
        group
            .alternate_version(primary, primary)
            .unwrap()
            .unwrap()
            .id,
        primary
    );
    assert!(
        group
            .alternate_version(primary, Uuid::new_v4())
            .unwrap()
            .is_none()
    );
}

#[test]
fn get_all_versions_from_any_version_returns_every_version_once() {
    let (group, primary, alternate1, alternate2) = setup_version_group();
    let expected = HashSet::from([primary, alternate1, alternate2]);

    for source in [primary, alternate1, alternate2] {
        let versions = group.all_versions(source).unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(
            versions.iter().map(|item| item.id).collect::<HashSet<_>>(),
            expected
        );
    }
}

#[test]
fn propagate_played_state_marks_alternates_and_resets_position_by_default() {
    let (group, primary, alternate1, alternate2) = setup_version_group();
    let updates = group
        .propagate_played_state(primary, true, true, HashMap::new())
        .unwrap();

    assert_eq!(updates.len(), 2);
    assert_eq!(
        updates
            .iter()
            .map(VersionPlaybackUpdate::item_id)
            .collect::<HashSet<_>>(),
        HashSet::from([alternate1, alternate2])
    );
    assert!(updates.iter().all(|update| matches!(
        update,
        VersionPlaybackUpdate::MarkPlayed {
            playback_position_ticks: Some(0),
            ..
        }
    )));
}

#[test]
fn propagate_played_state_without_reset_leaves_position_untouched() {
    let (group, primary, _, _) = setup_version_group();
    let updates = group
        .propagate_played_state(primary, true, false, HashMap::new())
        .unwrap();

    assert_eq!(updates.len(), 2);
    assert!(updates.iter().all(|update| matches!(
        update,
        VersionPlaybackUpdate::MarkPlayed {
            playback_position_ticks: None,
            ..
        }
    )));
}

#[test]
fn propagate_unwatched_clears_all_watched_state_on_versions() {
    let (group, primary, alternate1, alternate2) = setup_version_group();
    let mut first = UserItemData::new("alt1");
    first.played = true;
    first.play_count = 3;
    first.playback_position_ticks = 1_000;
    first.last_played_date = Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
    let mut second = UserItemData::new("alt2");
    second.played = true;
    second.play_count = 1;
    second.playback_position_ticks = 500;
    second.last_played_date = Some(Utc.with_ymd_and_hms(2021, 2, 2, 0, 0, 0).unwrap());
    let existing = HashMap::from([(alternate1, first), (alternate2, second)]);

    let updates = group
        .propagate_played_state(primary, false, true, existing)
        .unwrap();

    assert_eq!(updates.len(), 2);
    for update in updates {
        let VersionPlaybackUpdate::MarkUnplayed { user_data, .. } = update else {
            panic!("expected an unplayed update");
        };
        assert!(!user_data.played);
        assert_eq!(user_data.play_count, 0);
        assert_eq!(user_data.playback_position_ticks, 0);
        assert_eq!(user_data.last_played_date, None);
    }
}

#[test]
fn propagate_played_state_single_version_does_nothing() {
    let solo = VideoItem::new(Uuid::new_v4(), "/Movies/Solo/Solo.mkv");
    let solo_id = solo.id;
    let group = VersionGroup::new(solo);

    assert!(
        group
            .propagate_played_state(solo_id, true, true, HashMap::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn media_sources_default_to_queried_versions_own_source() {
    let (group, primary, alternate1, _) = setup_version_group();

    assert_eq!(
        group.media_sources(alternate1).unwrap()[0].id,
        alternate1.simple().to_string()
    );
    assert_eq!(
        group.media_sources(primary).unwrap()[0].id,
        primary.simple().to_string()
    );
}

#[test]
fn all_items_for_media_sources_from_any_version_has_no_duplicates() {
    let (group, primary, alternate1, alternate2) = setup_version_group();
    let expected = HashSet::from([primary, alternate1, alternate2]);

    for source in [primary, alternate1, alternate2] {
        let ids: Vec<_> = group
            .all_versions(source)
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>(), expected);
    }
}
