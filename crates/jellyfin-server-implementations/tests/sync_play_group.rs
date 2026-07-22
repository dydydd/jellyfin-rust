use std::{cell::RefCell, collections::HashMap};

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
