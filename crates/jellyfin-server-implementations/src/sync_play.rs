use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use jellyfin_model::{GroupInfoDto, GroupQueueMode, GroupStateType};
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
    play_queue: PlayQueue,
    position_ticks: i64,
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
            play_queue: PlayQueue::new(),
            position_ticks: 0,
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

    pub async fn queue_state_for_session(&self, session_id: &str) -> Option<SyncPlayQueueState> {
        let state = self.state.read().await;
        let group_id = state.session_groups.get(session_id)?;
        let group = state.groups.get(group_id)?;
        Some(SyncPlayQueueState {
            items: group.play_queue.get_playlist().to_vec(),
            playing_item_index: group.play_queue.playing_item_index(),
            position_ticks: group.position_ticks,
        })
    }

    pub async fn queue_item_ids_for_group(&self, group_id: Uuid) -> Option<Vec<Uuid>> {
        self.state.read().await.groups.get(&group_id).map(|group| {
            group
                .play_queue
                .get_playlist()
                .iter()
                .map(|item| item.item_id)
                .collect()
        })
    }

    pub async fn participant_user_ids_for_session(&self, session_id: &str) -> Option<Vec<Uuid>> {
        let state = self.state.read().await;
        let group_id = state.session_groups.get(session_id)?;
        let group = state.groups.get(group_id)?;
        let mut user_ids = group
            .participants
            .values()
            .map(|participant| participant.user_id)
            .collect::<Vec<_>>();
        user_ids.sort_unstable();
        user_ids.dedup();
        Some(user_ids)
    }

    pub async fn set_new_queue(
        &self,
        session_id: &str,
        item_ids: &[Uuid],
        playing_item_position: i32,
        start_position_ticks: i64,
    ) -> bool {
        if item_ids.is_empty()
            || playing_item_position < 0
            || usize::try_from(playing_item_position).map_or(true, |index| index >= item_ids.len())
        {
            return false;
        }
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        group.play_queue.set_playlist(item_ids);
        group
            .play_queue
            .set_playing_item_by_index(playing_item_position);
        group.position_ticks = start_position_ticks;
        group.state = GroupStateType::Waiting;
        true
    }

    pub async fn set_playlist_item(&self, session_id: &str, playlist_item_id: Uuid) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        if !group
            .play_queue
            .set_playing_item_by_playlist_id(playlist_item_id)
        {
            return false;
        }
        group.position_ticks = 0;
        group.state = GroupStateType::Waiting;
        true
    }

    pub async fn remove_from_playlist(
        &self,
        session_id: &str,
        playlist_item_ids: &[Uuid],
        clear_playlist: bool,
        clear_playing_item: bool,
    ) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        let playing_item_removed = if clear_playlist {
            group.play_queue.clear_playlist(clear_playing_item);
            clear_playing_item
        } else {
            group.play_queue.remove_from_playlist(playlist_item_ids)
        };
        if playing_item_removed {
            group.position_ticks = 0;
            if !group.play_queue.is_item_playing() {
                group.state = GroupStateType::Idle;
            }
        }
        true
    }

    pub async fn move_playlist_item(
        &self,
        session_id: &str,
        playlist_item_id: Uuid,
        new_index: i32,
    ) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        group
            .play_queue
            .move_playlist_item(playlist_item_id, new_index)
    }

    pub async fn queue_items(
        &self,
        session_id: &str,
        item_ids: &[Uuid],
        mode: GroupQueueMode,
    ) -> bool {
        if item_ids.is_empty() {
            return false;
        }
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        match mode {
            GroupQueueMode::Queue => group.play_queue.queue(item_ids),
            GroupQueueMode::QueueNext => group.play_queue.queue_next(item_ids),
        }
        true
    }
}

fn group_for_session_mut<'a>(
    state: &'a mut SyncPlayManagerState,
    session_id: &str,
) -> Option<&'a mut ManagedSyncPlayGroup> {
    let group_id = *state.session_groups.get(session_id)?;
    state.groups.get_mut(&group_id)
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayQueue {
    playlist: Vec<PlayQueueItem>,
    playing_item_index: i32,
}

impl Default for PlayQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayQueue {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            playlist: Vec::new(),
            playing_item_index: -1,
        }
    }

    /// Clears every queued item.
    pub fn reset(&mut self) {
        self.playlist.clear();
        self.playing_item_index = -1;
    }

    /// Replaces the queue while preserving input order and duplicate items.
    pub fn set_playlist(&mut self, item_ids: &[Uuid]) {
        self.playlist = item_ids.iter().copied().map(PlayQueueItem::new).collect();
        self.playing_item_index = -1;
    }

    /// Returns the structured queue in playback order.
    #[must_use]
    pub fn get_playlist(&self) -> &[PlayQueueItem] {
        &self.playlist
    }

    #[must_use]
    pub const fn playing_item_index(&self) -> i32 {
        self.playing_item_index
    }

    #[must_use]
    pub const fn is_item_playing(&self) -> bool {
        self.playing_item_index != -1
    }

    pub fn set_playing_item_by_index(&mut self, index: i32) {
        self.playing_item_index = usize::try_from(index)
            .ok()
            .filter(|index| *index < self.playlist.len())
            .map_or(-1, |index| i32::try_from(index).unwrap_or(i32::MAX));
    }

    pub fn set_playing_item_by_playlist_id(&mut self, playlist_item_id: Uuid) -> bool {
        self.playing_item_index = self
            .playlist
            .iter()
            .position(|item| item.playlist_item_id == playlist_item_id)
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        self.is_item_playing()
    }

    pub fn queue(&mut self, item_ids: &[Uuid]) {
        self.playlist
            .extend(item_ids.iter().copied().map(PlayQueueItem::new));
    }

    pub fn queue_next(&mut self, item_ids: &[Uuid]) {
        let insertion_index = usize::try_from(self.playing_item_index + 1)
            .unwrap_or(0)
            .min(self.playlist.len());
        self.playlist.splice(
            insertion_index..insertion_index,
            item_ids.iter().copied().map(PlayQueueItem::new),
        );
    }

    pub fn clear_playlist(&mut self, clear_playing_item: bool) {
        let playing_item = self.playing_item();
        self.playlist.clear();
        if !clear_playing_item && let Some(playing_item) = playing_item {
            self.playlist.push(playing_item);
            self.playing_item_index = 0;
        } else {
            self.playing_item_index = -1;
        }
    }

    pub fn remove_from_playlist(&mut self, playlist_item_ids: &[Uuid]) -> bool {
        let playing_item = self.playing_item();
        self.playlist
            .retain(|item| !playlist_item_ids.contains(&item.playlist_item_id));
        let Some(playing_item) = playing_item else {
            return false;
        };
        if playlist_item_ids.contains(&playing_item.playlist_item_id) {
            self.playing_item_index -= 1;
            if self.playing_item_index < 0 {
                self.playing_item_index = if self.playlist.is_empty() { -1 } else { 0 };
            }
            return true;
        }
        self.set_playing_item_by_playlist_id(playing_item.playlist_item_id);
        false
    }

    pub fn move_playlist_item(&mut self, playlist_item_id: Uuid, new_index: i32) -> bool {
        let Some(old_index) = self
            .playlist
            .iter()
            .position(|item| item.playlist_item_id == playlist_item_id)
        else {
            return false;
        };
        let playing_item = self.playing_item();
        let queue_item = self.playlist.remove(old_index);
        let new_index = usize::try_from(new_index)
            .unwrap_or(0)
            .min(self.playlist.len());
        self.playlist.insert(new_index, queue_item);
        self.playing_item_index = playing_item
            .and_then(|playing_item| self.playlist.iter().position(|item| *item == playing_item))
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        true
    }

    fn playing_item(&self) -> Option<PlayQueueItem> {
        usize::try_from(self.playing_item_index)
            .ok()
            .and_then(|index| self.playlist.get(index))
            .copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlayQueueState {
    pub items: Vec<PlayQueueItem>,
    pub playing_item_index: i32,
    pub position_ticks: i64,
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
