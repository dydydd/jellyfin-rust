use std::{cell::RefCell, collections::HashMap};

use jellyfin_model::{GroupRepeatMode, GroupShuffleMode};
use jellyfin_server_implementations::{GroupLibrary, SyncPlayGroup};
use uuid::Uuid;

#[test]
fn visible_official_queue_is_accessible_and_structured() {
    let item_id = Uuid::new_v4();
    let library = RecordingLibrary::with_items([(item_id, true)]);
    let mut group = SyncPlayGroup::new(library);
    group.play_queue_mut().reset();
    group.play_queue_mut().set_playlist(&[item_id]);

    assert_eq!(group.play_queue().get_playlist().len(), 1);
    assert_eq!(group.play_queue().get_playlist()[0].item_id, item_id);
    assert!(group.has_access_to_play_queue(&TestUser));
}

#[test]
fn missing_official_queue_item_denies_access() {
    let item_id = Uuid::new_v4();
    let mut group = SyncPlayGroup::new(RecordingLibrary::default());
    group.play_queue_mut().reset();
    group.play_queue_mut().set_playlist(&[item_id]);

    assert_eq!(group.play_queue().get_playlist().len(), 1);
    assert_eq!(group.play_queue().get_playlist()[0].item_id, item_id);
    assert!(!group.has_access_to_play_queue(&TestUser));
}

#[test]
fn empty_queue_is_accessible_without_library_calls() {
    let group = SyncPlayGroup::new(RecordingLibrary::default());

    assert!(group.has_access_to_play_queue(&TestUser));
    assert!(group.library().loaded.borrow().is_empty());
    assert!(group.library().visibility_checked.borrow().is_empty());
}

#[test]
fn invisible_item_denies_access_and_stops_at_its_queue_position() {
    let visible = Uuid::new_v4();
    let invisible = Uuid::new_v4();
    let never_loaded = Uuid::new_v4();
    let library =
        RecordingLibrary::with_items([(visible, true), (invisible, false), (never_loaded, true)]);
    let mut group = SyncPlayGroup::new(library);
    group
        .play_queue_mut()
        .set_playlist(&[visible, invisible, never_loaded]);

    assert!(!group.has_access_to_play_queue(&TestUser));
    assert_eq!(&*group.library().loaded.borrow(), &[visible, invisible]);
    assert_eq!(
        &*group.library().visibility_checked.borrow(),
        &[visible, invisible]
    );
}

#[test]
fn mixed_queue_stops_when_an_item_is_missing() {
    let visible = Uuid::new_v4();
    let missing = Uuid::new_v4();
    let never_loaded = Uuid::new_v4();
    let library = RecordingLibrary::with_items([(visible, true), (never_loaded, true)]);
    let mut group = SyncPlayGroup::new(library);
    group
        .play_queue_mut()
        .set_playlist(&[visible, missing, never_loaded]);

    assert!(!group.has_access_to_play_queue(&TestUser));
    assert_eq!(&*group.library().loaded.borrow(), &[visible, missing]);
    assert_eq!(&*group.library().visibility_checked.borrow(), &[visible]);
}

#[test]
fn duplicate_items_keep_order_and_receive_distinct_playlist_ids() {
    let repeated = Uuid::new_v4();
    let other = Uuid::new_v4();
    let library = RecordingLibrary::with_items([(repeated, true), (other, true)]);
    let mut group = SyncPlayGroup::new(library);
    group
        .play_queue_mut()
        .set_playlist(&[repeated, other, repeated]);

    let playlist = group.play_queue().get_playlist();
    assert_eq!(
        playlist.iter().map(|item| item.item_id).collect::<Vec<_>>(),
        [repeated, other, repeated]
    );
    assert_ne!(playlist[0].playlist_item_id, playlist[2].playlist_item_id);
    assert!(group.has_access_to_play_queue(&TestUser));
    assert_eq!(
        &*group.library().loaded.borrow(),
        &[repeated, other, repeated]
    );
    assert_eq!(
        &*group.library().visibility_checked.borrow(),
        &[repeated, other, repeated]
    );
}

#[test]
fn queue_mutations_match_official_current_item_contract() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let third = Uuid::new_v4();
    let next = Uuid::new_v4();
    let mut group = SyncPlayGroup::new(RecordingLibrary::default());
    group.play_queue_mut().set_playlist(&[first, second, third]);
    group.play_queue_mut().set_playing_item_by_index(1);
    let second_playlist_id = group.play_queue().get_playlist()[1].playlist_item_id;

    group.play_queue_mut().queue_next(&[next]);
    assert_eq!(item_ids(group.play_queue()), [first, second, next, third]);
    assert_eq!(group.play_queue().playing_item_index(), 1);

    let third_playlist_id = group.play_queue().get_playlist()[3].playlist_item_id;
    assert!(
        group
            .play_queue_mut()
            .move_playlist_item(third_playlist_id, -50)
    );
    assert_eq!(item_ids(group.play_queue()), [third, first, second, next]);
    assert_eq!(group.play_queue().playing_item_index(), 2);

    assert!(
        group
            .play_queue_mut()
            .remove_from_playlist(&[second_playlist_id])
    );
    assert_eq!(item_ids(group.play_queue()), [third, first, next]);
    assert_eq!(group.play_queue().playing_item_index(), 1);
}

#[test]
fn clearing_queue_can_preserve_or_remove_the_playing_occurrence() {
    let first = Uuid::new_v4();
    let repeated = Uuid::new_v4();
    let mut group = SyncPlayGroup::new(RecordingLibrary::default());
    group
        .play_queue_mut()
        .set_playlist(&[first, repeated, repeated]);
    group.play_queue_mut().set_playing_item_by_index(2);
    let playing_playlist_id = group.play_queue().get_playlist()[2].playlist_item_id;

    group.play_queue_mut().clear_playlist(false);
    assert_eq!(item_ids(group.play_queue()), [repeated]);
    assert_eq!(group.play_queue().playing_item_index(), 0);
    assert_eq!(
        group.play_queue().get_playlist()[0].playlist_item_id,
        playing_playlist_id
    );

    group.play_queue_mut().clear_playlist(true);
    assert!(group.play_queue().get_playlist().is_empty());
    assert!(!group.play_queue().is_item_playing());
    assert_eq!(group.play_queue().playing_item_index(), -1);
}

#[test]
fn navigation_respects_repeat_modes_and_current_boundaries() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut group = SyncPlayGroup::new(RecordingLibrary::default());
    group.play_queue_mut().set_playlist(&[first, second]);
    group.play_queue_mut().set_playing_item_by_index(0);

    assert!(!group.play_queue_mut().previous());
    assert_eq!(group.play_queue().playing_item_id(), first);
    assert!(group.play_queue_mut().next_item());
    assert_eq!(group.play_queue().playing_item_id(), second);
    assert!(!group.play_queue_mut().next_item());
    assert_eq!(group.play_queue().playing_item_id(), second);

    group
        .play_queue_mut()
        .set_repeat_mode(GroupRepeatMode::RepeatAll);
    assert!(group.play_queue_mut().next_item());
    assert_eq!(group.play_queue().playing_item_id(), first);
    assert!(group.play_queue_mut().previous());
    assert_eq!(group.play_queue().playing_item_id(), second);

    group
        .play_queue_mut()
        .set_repeat_mode(GroupRepeatMode::RepeatOne);
    assert!(group.play_queue_mut().next_item());
    assert_eq!(group.play_queue().playing_item_id(), second);
}

#[test]
fn shuffle_preserves_current_occurrence_and_restores_sorted_order() {
    let expected_ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let mut group = SyncPlayGroup::new(RecordingLibrary::default());
    group.play_queue_mut().set_playlist(&expected_ids);
    group.play_queue_mut().set_playing_item_by_index(1);
    let playing_playlist_id = group.play_queue().playing_item_playlist_id();

    group
        .play_queue_mut()
        .set_shuffle_mode(GroupShuffleMode::Shuffle);
    assert_eq!(group.play_queue().shuffle_mode(), GroupShuffleMode::Shuffle);
    assert_eq!(group.play_queue().playing_item_index(), 0);
    assert_eq!(
        group.play_queue().playing_item_playlist_id(),
        playing_playlist_id
    );

    group
        .play_queue_mut()
        .set_shuffle_mode(GroupShuffleMode::Sorted);
    assert_eq!(item_ids(group.play_queue()), expected_ids);
    assert_eq!(group.play_queue().playing_item_index(), 1);
    assert_eq!(group.play_queue().shuffle_mode(), GroupShuffleMode::Sorted);
}

fn item_ids(queue: &jellyfin_server_implementations::PlayQueue) -> Vec<Uuid> {
    queue
        .get_playlist()
        .iter()
        .map(|item| item.item_id)
        .collect()
}

#[derive(Debug)]
struct TestUser;

#[derive(Debug, Clone, Copy)]
struct TestItem {
    id: Uuid,
    visible: bool,
}

#[derive(Debug, Default)]
struct RecordingLibrary {
    items: HashMap<Uuid, TestItem>,
    loaded: RefCell<Vec<Uuid>>,
    visibility_checked: RefCell<Vec<Uuid>>,
}

impl RecordingLibrary {
    fn with_items(items: impl IntoIterator<Item = (Uuid, bool)>) -> Self {
        Self {
            items: items
                .into_iter()
                .map(|(id, visible)| (id, TestItem { id, visible }))
                .collect(),
            ..Default::default()
        }
    }
}

impl GroupLibrary for RecordingLibrary {
    type User = TestUser;
    type Item = TestItem;

    fn get_item_by_id(&self, item_id: Uuid) -> Option<Self::Item> {
        self.loaded.borrow_mut().push(item_id);
        self.items.get(&item_id).copied()
    }

    fn is_visible_standalone(&self, item: &Self::Item, _user: &Self::User) -> bool {
        self.visibility_checked.borrow_mut().push(item.id);
        item.visible
    }
}
