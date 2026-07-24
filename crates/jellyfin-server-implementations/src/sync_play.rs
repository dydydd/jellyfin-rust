use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use jellyfin_model::{GroupInfoDto, GroupStateType};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlaySession {
    pub session_id: String,
    pub user_id: Uuid,
    pub user_name: String,
}

#[derive(Debug, Clone)]
struct SyncPlayParticipant {
    user_id: Uuid,
    user_name: String,
}

#[derive(Debug, Clone)]
struct ManagedSyncPlayGroup {
    id: Uuid,
    name: String,
    state: GroupStateType,
    participants: HashMap<String, SyncPlayParticipant>,
}

impl ManagedSyncPlayGroup {
    fn info(&self) -> GroupInfoDto {
        let mut participants = self
            .participants
            .values()
            .map(|participant| participant.user_name.clone())
            .collect::<Vec<_>>();
        participants.sort_unstable();
        participants.dedup();
        GroupInfoDto::new(
            self.id,
            self.name.clone(),
            self.state,
            participants,
            Utc::now(),
        )
    }
}

#[derive(Debug, Default)]
struct SyncPlayManagerState {
    groups: HashMap<Uuid, ManagedSyncPlayGroup>,
    session_groups: HashMap<String, Uuid>,
}

/// Coordinates the process-local lifecycle of `SyncPlay` groups.
#[derive(Debug, Clone, Default)]
pub struct SyncPlayManager {
    state: Arc<RwLock<SyncPlayManagerState>>,
}

impl SyncPlayManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_group(&self, session: SyncPlaySession, group_name: String) -> GroupInfoDto {
        let mut state = self.state.write().await;
        remove_session(&mut state, &session.session_id);

        let id = Uuid::new_v4();
        let participant = SyncPlayParticipant {
            user_id: session.user_id,
            user_name: session.user_name,
        };
        let group = ManagedSyncPlayGroup {
            id,
            name: group_name,
            state: GroupStateType::Idle,
            participants: HashMap::from([(session.session_id.clone(), participant)]),
        };
        let info = group.info();
        state.groups.insert(id, group);
        state.session_groups.insert(session.session_id, id);
        info
    }

    pub async fn join_group(&self, session: SyncPlaySession, group_id: Uuid) -> bool {
        let mut state = self.state.write().await;
        if !state.groups.contains_key(&group_id) {
            return false;
        }
        if state.session_groups.get(&session.session_id) == Some(&group_id) {
            return true;
        }

        remove_session(&mut state, &session.session_id);
        let participant = SyncPlayParticipant {
            user_id: session.user_id,
            user_name: session.user_name,
        };
        let Some(group) = state.groups.get_mut(&group_id) else {
            return false;
        };
        group
            .participants
            .insert(session.session_id.clone(), participant);
        state.session_groups.insert(session.session_id, group_id);
        true
    }

    pub async fn leave_group(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        remove_session(&mut state, session_id)
    }

    pub async fn list_groups(&self) -> Vec<GroupInfoDto> {
        let state = self.state.read().await;
        let mut groups = state
            .groups
            .values()
            .map(ManagedSyncPlayGroup::info)
            .collect::<Vec<_>>();
        groups.sort_unstable_by_key(|group| group.group_id);
        groups
    }

    pub async fn get_group(&self, group_id: Uuid) -> Option<GroupInfoDto> {
        self.state
            .read()
            .await
            .groups
            .get(&group_id)
            .map(ManagedSyncPlayGroup::info)
    }

    pub async fn is_user_active(&self, user_id: Uuid) -> bool {
        self.state.read().await.groups.values().any(|group| {
            group
                .participants
                .values()
                .any(|participant| participant.user_id == user_id)
        })
    }

    pub async fn is_session_active(&self, session_id: &str) -> bool {
        self.state
            .read()
            .await
            .session_groups
            .contains_key(session_id)
    }
}

fn remove_session(state: &mut SyncPlayManagerState, session_id: &str) -> bool {
    let Some(group_id) = state.session_groups.remove(session_id) else {
        return false;
    };
    let remove_group = state.groups.get_mut(&group_id).is_some_and(|group| {
        group.participants.remove(session_id);
        group.participants.is_empty()
    });
    if remove_group {
        state.groups.remove(&group_id);
    }
    true
}

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
