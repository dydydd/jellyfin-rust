use uuid::Uuid;

/// A media item occurrence in a `SyncPlay` queue.
///
/// Repeated media items share an `item_id` but receive distinct
/// `playlist_item_id` values so each queue position remains addressable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayQueueItem {
    pub item_id: Uuid,
    pub playlist_item_id: Uuid,
}

impl PlayQueueItem {
    #[must_use]
    pub fn new(item_id: Uuid) -> Self {
        Self {
            item_id,
            playlist_item_id: Uuid::new_v4(),
        }
    }
}

/// Ordered `SyncPlay` media queue without playback state-machine behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayQueue {
    playlist: Vec<PlayQueueItem>,
}

impl PlayQueue {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            playlist: Vec::new(),
        }
    }

    /// Clears every queued item.
    pub fn reset(&mut self) {
        self.playlist.clear();
    }

    /// Replaces the queue while preserving input order and duplicate items.
    pub fn set_playlist(&mut self, item_ids: &[Uuid]) {
        self.playlist = item_ids.iter().copied().map(PlayQueueItem::new).collect();
    }

    /// Returns the structured queue in playback order.
    #[must_use]
    pub fn get_playlist(&self) -> &[PlayQueueItem] {
        &self.playlist
    }
}

/// Library lookup and standalone visibility boundary used by `SyncPlay` groups.
pub trait GroupLibrary {
    type User;
    type Item;

    fn get_item_by_id(&self, item_id: Uuid) -> Option<Self::Item>;

    fn is_visible_standalone(&self, item: &Self::Item, user: &Self::User) -> bool;
}

/// Minimal `SyncPlay` group orchestration for play-queue access checks.
///
/// This type intentionally excludes group membership, playback coordination,
/// broadcast, shuffle, repeat, and other `SyncPlay` state-machine behavior.
#[derive(Debug, Clone)]
pub struct SyncPlayGroup<L> {
    library: L,
    play_queue: PlayQueue,
}

impl<L> SyncPlayGroup<L> {
    #[must_use]
    pub const fn new(library: L) -> Self {
        Self {
            library,
            play_queue: PlayQueue::new(),
        }
    }

    #[must_use]
    pub const fn library(&self) -> &L {
        &self.library
    }

    #[must_use]
    pub const fn play_queue(&self) -> &PlayQueue {
        &self.play_queue
    }

    pub const fn play_queue_mut(&mut self) -> &mut PlayQueue {
        &mut self.play_queue
    }
}

impl<L: GroupLibrary> SyncPlayGroup<L> {
    /// Returns whether `user` can access every current queue occurrence.
    ///
    /// An empty queue is accessible. Items are loaded in queue order; a
    /// missing or invisible item immediately denies access.
    #[must_use]
    pub fn has_access_to_play_queue(&self, user: &L::User) -> bool {
        for queue_item in self.play_queue.get_playlist() {
            let Some(item) = self.library.get_item_by_id(queue_item.item_id) else {
                return false;
            };
            if !self.library.is_visible_standalone(&item, user) {
                return false;
            }
        }

        true
    }
}
