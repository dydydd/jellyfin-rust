use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use jellyfin_model::{
    BufferRequestDto, GroupInfoDto, GroupQueueMode, GroupRepeatMode, GroupShuffleMode,
    GroupStateType, GroupUpdateDto, GroupUpdateType, PlayQueueUpdateDto, PlayQueueUpdateReason,
    ReadyRequestDto, SendCommandDto, SendCommandType, SyncPlayQueueItemDto,
};
use rand::seq::SliceRandom;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlaySession {
    pub session_id: String,
    pub user_id: Uuid,
    pub user_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncPlayGroupUpdate {
    pub session_ids: Vec<String>,
    pub payload: Value,
}

#[derive(Debug, Clone)]
struct SyncPlayParticipant {
    user_id: Uuid,
    user_name: String,
    ping: i64,
    is_buffering: bool,
    ignore_wait: bool,
}

#[derive(Debug, Clone)]
struct ManagedSyncPlayGroup {
    id: Uuid,
    name: String,
    state: GroupStateType,
    participants: HashMap<String, SyncPlayParticipant>,
    play_queue: PlayQueue,
    position_ticks: i64,
    resume_playing: bool,
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
    websocket_connections: HashMap<String, usize>,
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
        self.create_group_with_updates(session, group_name).await.0
    }

    pub async fn create_group_with_updates(
        &self,
        session: SyncPlaySession,
        group_name: String,
    ) -> (GroupInfoDto, Vec<SyncPlayGroupUpdate>) {
        let mut state = self.state.write().await;
        let mut updates = remove_session_with_updates(&mut state, &session).unwrap_or_default();

        let id = Uuid::new_v4();
        let participant = SyncPlayParticipant {
            user_id: session.user_id,
            user_name: session.user_name,
            ping: 500,
            is_buffering: false,
            ignore_wait: false,
        };
        let group = ManagedSyncPlayGroup {
            id,
            name: group_name,
            state: GroupStateType::Idle,
            participants: HashMap::from([(session.session_id.clone(), participant)]),
            play_queue: PlayQueue::new(),
            position_ticks: 0,
            resume_playing: false,
        };
        let info = group.info();
        state.groups.insert(id, group);
        state.session_groups.insert(session.session_id.clone(), id);
        updates.push(group_update(
            vec![session.session_id],
            id,
            info.clone(),
            GroupUpdateType::GroupJoined,
        ));
        (info, updates)
    }

    pub async fn join_group(&self, session: SyncPlaySession, group_id: Uuid) -> bool {
        self.join_group_with_updates(session, group_id)
            .await
            .is_some()
    }

    pub async fn join_group_with_updates(
        &self,
        session: SyncPlaySession,
        group_id: Uuid,
    ) -> Option<Vec<SyncPlayGroupUpdate>> {
        let mut state = self.state.write().await;
        if !state.groups.contains_key(&group_id) {
            return None;
        }
        if state.session_groups.get(&session.session_id) == Some(&group_id) {
            return Some(Vec::new());
        }

        let mut updates = Vec::new();
        if state
            .session_groups
            .get(&session.session_id)
            .is_some_and(|id| *id != group_id)
        {
            updates.extend(remove_session_with_updates(&mut state, &session).unwrap_or_default());
        }

        let mut participant = SyncPlayParticipant {
            user_id: session.user_id,
            user_name: session.user_name.clone(),
            ping: 500,
            is_buffering: false,
            ignore_wait: false,
        };
        let Some(group) = state.groups.get_mut(&group_id) else {
            return None;
        };
        participant.is_buffering = group.state == GroupStateType::Waiting;
        group
            .participants
            .insert(session.session_id.clone(), participant);
        let info = group.info();
        let mut other_sessions = group
            .participants
            .keys()
            .filter(|id| *id != &session.session_id)
            .cloned()
            .collect::<Vec<_>>();
        other_sessions.sort_unstable();
        state
            .session_groups
            .insert(session.session_id.clone(), group_id);
        updates.push(group_update(
            vec![session.session_id],
            group_id,
            info,
            GroupUpdateType::GroupJoined,
        ));
        if !other_sessions.is_empty() {
            updates.push(group_update(
                other_sessions,
                group_id,
                session.user_name,
                GroupUpdateType::UserJoined,
            ));
        }
        Some(updates)
    }

    pub async fn leave_group(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        remove_session(&mut state, session_id)
    }

    pub async fn leave_group_with_updates(
        &self,
        session: &SyncPlaySession,
    ) -> Option<Vec<SyncPlayGroupUpdate>> {
        let mut state = self.state.write().await;
        remove_session_with_updates(&mut state, session)
    }

    pub async fn websocket_connected(&self, session_id: &str) {
        let mut state = self.state.write().await;
        *state
            .websocket_connections
            .entry(session_id.to_owned())
            .or_default() += 1;
    }

    pub async fn websocket_disconnected(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        let Some(connections) = state.websocket_connections.get_mut(session_id) else {
            return false;
        };
        *connections -= 1;
        if *connections != 0 {
            return false;
        }
        state.websocket_connections.remove(session_id);
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
            repeat_mode: group.play_queue.repeat_mode(),
            shuffle_mode: group.play_queue.shuffle_mode(),
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

    pub async fn playback_command_for_session(
        &self,
        session_id: &str,
        command: SendCommandType,
    ) -> Option<(Vec<String>, SendCommandDto)> {
        let state = self.state.read().await;
        let group_id = *state.session_groups.get(session_id)?;
        let group = state.groups.get(&group_id)?;
        let emitted_at = Utc::now();
        let mut sessions = group.participants.keys().cloned().collect::<Vec<_>>();
        sessions.sort_unstable();
        Some((
            sessions,
            SendCommandDto {
                group_id,
                playlist_item_id: group.play_queue.playing_item_playlist_id(),
                when: emitted_at,
                position_ticks: Some(group.position_ticks),
                command,
                emitted_at,
            },
        ))
    }

    pub async fn queue_update_for_session(
        &self,
        session_id: &str,
        reason: PlayQueueUpdateReason,
    ) -> Option<(Vec<String>, GroupUpdateDto<PlayQueueUpdateDto>)> {
        let state = self.state.read().await;
        let group_id = *state.session_groups.get(session_id)?;
        let group = state.groups.get(&group_id)?;
        let mut sessions = group.participants.keys().cloned().collect::<Vec<_>>();
        sessions.sort_unstable();
        let playlist = group
            .play_queue
            .get_playlist()
            .iter()
            .map(|item| SyncPlayQueueItemDto {
                item_id: item.item_id,
                playlist_item_id: item.playlist_item_id,
            })
            .collect();
        Some((
            sessions,
            GroupUpdateDto {
                group_id,
                data: PlayQueueUpdateDto {
                    reason,
                    last_update: Utc::now(),
                    playlist,
                    playing_item_index: group.play_queue.playing_item_index(),
                    start_position_ticks: group.position_ticks,
                    is_playing: group.state == GroupStateType::Playing,
                    shuffle_mode: group.play_queue.shuffle_mode(),
                    repeat_mode: group.play_queue.repeat_mode(),
                },
                update_type: GroupUpdateType::PlayQueue,
            },
        ))
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
        group.play_queue.reset();
        group.play_queue.set_playlist(item_ids);
        group
            .play_queue
            .set_playing_item_by_index(playing_item_position);
        group.position_ticks = start_position_ticks;
        begin_wait(group, true);
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
        begin_wait(group, true);
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

    pub async fn unpause(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        match group.state {
            GroupStateType::Idle => begin_wait(group, true),
            GroupStateType::Waiting if !group.resume_playing => group.resume_playing = true,
            GroupStateType::Waiting | GroupStateType::Paused | GroupStateType::Playing => {
                group.state = GroupStateType::Playing;
                group.resume_playing = true;
                set_all_buffering(group, false);
            }
        }
        true
    }

    pub async fn pause(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        match group.state {
            GroupStateType::Playing => group.state = GroupStateType::Paused,
            GroupStateType::Waiting => group.resume_playing = false,
            GroupStateType::Idle | GroupStateType::Paused => {}
        }
        true
    }

    pub async fn stop(&self, session_id: &str) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        group.state = GroupStateType::Idle;
        group.position_ticks = 0;
        group.resume_playing = false;
        set_all_buffering(group, false);
        true
    }

    pub async fn seek(&self, session_id: &str, position_ticks: i64, runtime_ticks: i64) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        if group.state == GroupStateType::Idle {
            return false;
        }
        let resume_playing = match group.state {
            GroupStateType::Playing => true,
            GroupStateType::Paused => false,
            GroupStateType::Waiting => group.resume_playing,
            GroupStateType::Idle => return false,
        };
        group.position_ticks = position_ticks.clamp(0, runtime_ticks.max(0));
        begin_wait(group, resume_playing);
        true
    }

    pub async fn next_item(&self, session_id: &str, playlist_item_id: Uuid) -> bool {
        navigate_queue(self, session_id, playlist_item_id, true).await
    }

    pub async fn previous_item(&self, session_id: &str, playlist_item_id: Uuid) -> bool {
        navigate_queue(self, session_id, playlist_item_id, false).await
    }

    pub async fn set_repeat_mode(&self, session_id: &str, mode: GroupRepeatMode) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        group.play_queue.set_repeat_mode(mode);
        true
    }

    pub async fn set_shuffle_mode(&self, session_id: &str, mode: GroupShuffleMode) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        group.play_queue.set_shuffle_mode(mode);
        true
    }

    pub async fn buffering(
        &self,
        session_id: &str,
        request: BufferRequestDto,
        runtime_ticks: i64,
    ) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        let resume_playing = match group.state {
            GroupStateType::Playing => true,
            GroupStateType::Paused => false,
            GroupStateType::Waiting => group.resume_playing,
            GroupStateType::Idle => return true,
        };
        group.state = GroupStateType::Waiting;
        group.resume_playing = resume_playing;
        let current_item = group.play_queue.playing_item_playlist_id();
        let Some(participant) = group.participants.get_mut(session_id) else {
            return false;
        };
        participant.is_buffering = true;
        if request.playlist_item_id == current_item {
            group.position_ticks = request.position_ticks.clamp(0, runtime_ticks.max(0));
        }
        true
    }

    pub async fn ready(
        &self,
        session_id: &str,
        request: ReadyRequestDto,
        runtime_ticks: i64,
    ) -> bool {
        const MAX_PLAYBACK_OFFSET_TICKS: i64 = 5_000_000;

        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        let current_item = group.play_queue.playing_item_playlist_id();
        let request_ticks = request.position_ticks.clamp(0, runtime_ticks.max(0));
        let correct_position = request.is_playing
            || group.position_ticks.abs_diff(request_ticks) <= MAX_PLAYBACK_OFFSET_TICKS as u64;
        let correct_position = if group.resume_playing {
            correct_position
        } else {
            group.position_ticks.abs_diff(request_ticks) <= MAX_PLAYBACK_OFFSET_TICKS as u64
        };
        let Some(participant) = group.participants.get_mut(session_id) else {
            return false;
        };
        participant.is_buffering = request.playlist_item_id != current_item || !correct_position;
        finish_wait_if_ready(group);
        true
    }

    pub async fn set_ignore_wait(&self, session_id: &str, ignore_wait: bool) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        let Some(participant) = group.participants.get_mut(session_id) else {
            return false;
        };
        participant.ignore_wait = ignore_wait;
        finish_wait_if_ready(group);
        true
    }

    pub async fn update_ping(&self, session_id: &str, ping: i64) -> bool {
        let mut state = self.state.write().await;
        let Some(group) = group_for_session_mut(&mut state, session_id) else {
            return false;
        };
        let Some(participant) = group.participants.get_mut(session_id) else {
            return false;
        };
        participant.ping = ping;
        true
    }

    pub async fn participant_state_for_session(
        &self,
        session_id: &str,
    ) -> Option<SyncPlayParticipantState> {
        let state = self.state.read().await;
        let group_id = state.session_groups.get(session_id)?;
        let participant = state.groups.get(group_id)?.participants.get(session_id)?;
        Some(SyncPlayParticipantState {
            ping: participant.ping,
            is_buffering: participant.is_buffering,
            ignore_wait: participant.ignore_wait,
        })
    }
}

async fn navigate_queue(
    manager: &SyncPlayManager,
    session_id: &str,
    playlist_item_id: Uuid,
    forwards: bool,
) -> bool {
    let mut state = manager.state.write().await;
    let Some(group) = group_for_session_mut(&mut state, session_id) else {
        return false;
    };
    if group.play_queue.playing_item_playlist_id() != playlist_item_id {
        return false;
    }
    let changed = if forwards {
        group.play_queue.next_item()
    } else {
        group.play_queue.previous()
    };
    if changed {
        group.position_ticks = 0;
        begin_wait(group, true);
    }
    changed
}

fn begin_wait(group: &mut ManagedSyncPlayGroup, resume_playing: bool) {
    group.state = GroupStateType::Waiting;
    group.resume_playing = resume_playing;
    set_all_buffering(group, true);
}

fn set_all_buffering(group: &mut ManagedSyncPlayGroup, is_buffering: bool) {
    for participant in group.participants.values_mut() {
        participant.is_buffering = is_buffering;
    }
}

fn finish_wait_if_ready(group: &mut ManagedSyncPlayGroup) {
    if group.state == GroupStateType::Waiting
        && !group
            .participants
            .values()
            .any(|participant| participant.is_buffering && !participant.ignore_wait)
    {
        group.state = if group.resume_playing {
            GroupStateType::Playing
        } else {
            GroupStateType::Paused
        };
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
        finish_wait_if_ready(group);
        group.participants.is_empty()
    });
    if remove_group {
        state.groups.remove(&group_id);
    }
    true
}

fn remove_session_with_updates(
    state: &mut SyncPlayManagerState,
    session: &SyncPlaySession,
) -> Option<Vec<SyncPlayGroupUpdate>> {
    let group_id = state.session_groups.remove(&session.session_id)?;
    let group = state.groups.get_mut(&group_id)?;
    group.participants.remove(&session.session_id);
    finish_wait_if_ready(group);

    let mut remaining_sessions = group.participants.keys().cloned().collect::<Vec<_>>();
    remaining_sessions.sort_unstable();
    let remove_group = remaining_sessions.is_empty();
    if remove_group {
        state.groups.remove(&group_id);
    }

    let mut updates = vec![group_update(
        vec![session.session_id.clone()],
        group_id,
        group_id.to_string(),
        GroupUpdateType::GroupLeft,
    )];
    if !remaining_sessions.is_empty() {
        updates.push(group_update(
            remaining_sessions,
            group_id,
            session.user_name.clone(),
            GroupUpdateType::UserLeft,
        ));
    }
    Some(updates)
}

fn group_update<T: Serialize>(
    session_ids: Vec<String>,
    group_id: Uuid,
    data: T,
    update_type: GroupUpdateType,
) -> SyncPlayGroupUpdate {
    let payload = serde_json::to_value(GroupUpdateDto {
        group_id,
        data,
        update_type,
    })
    .expect("SyncPlay group updates contain only serializable wire DTOs");
    SyncPlayGroupUpdate {
        session_ids,
        payload,
    }
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
    sorted_playlist: Option<Vec<PlayQueueItem>>,
    playing_item_index: i32,
    repeat_mode: GroupRepeatMode,
    shuffle_mode: GroupShuffleMode,
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
            sorted_playlist: None,
            playing_item_index: -1,
            repeat_mode: GroupRepeatMode::RepeatNone,
            shuffle_mode: GroupShuffleMode::Sorted,
        }
    }

    /// Clears every queued item.
    pub fn reset(&mut self) {
        self.playlist.clear();
        self.sorted_playlist = None;
        self.playing_item_index = -1;
        self.repeat_mode = GroupRepeatMode::RepeatNone;
        self.shuffle_mode = GroupShuffleMode::Sorted;
    }

    /// Replaces the queue while preserving input order and duplicate items.
    pub fn set_playlist(&mut self, item_ids: &[Uuid]) {
        self.playlist = item_ids.iter().copied().map(PlayQueueItem::new).collect();
        self.sorted_playlist = None;
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

    #[must_use]
    pub const fn repeat_mode(&self) -> GroupRepeatMode {
        self.repeat_mode
    }

    #[must_use]
    pub const fn shuffle_mode(&self) -> GroupShuffleMode {
        self.shuffle_mode
    }

    #[must_use]
    pub fn playing_item_playlist_id(&self) -> Uuid {
        self.playing_item()
            .map_or_else(Uuid::nil, |item| item.playlist_item_id)
    }

    #[must_use]
    pub fn playing_item_id(&self) -> Uuid {
        self.playing_item()
            .map_or_else(Uuid::nil, |item| item.item_id)
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
        let items = item_ids
            .iter()
            .copied()
            .map(PlayQueueItem::new)
            .collect::<Vec<_>>();
        self.playlist.extend(items.iter().copied());
        if let Some(sorted) = self.sorted_playlist.as_mut() {
            sorted.extend(items);
        }
    }

    pub fn queue_next(&mut self, item_ids: &[Uuid]) {
        let playing_item = self.playing_item();
        let insertion_index = usize::try_from(self.playing_item_index + 1)
            .unwrap_or(0)
            .min(self.playlist.len());
        let items = item_ids
            .iter()
            .copied()
            .map(PlayQueueItem::new)
            .collect::<Vec<_>>();
        self.playlist
            .splice(insertion_index..insertion_index, items.iter().copied());
        if let Some(sorted) = self.sorted_playlist.as_mut() {
            let sorted_index = playing_item
                .and_then(|playing| sorted.iter().position(|item| *item == playing))
                .map_or(0, |index| index + 1);
            sorted.splice(sorted_index..sorted_index, items);
        }
    }

    pub fn clear_playlist(&mut self, clear_playing_item: bool) {
        let playing_item = self.playing_item();
        self.playlist.clear();
        if let Some(sorted) = self.sorted_playlist.as_mut() {
            sorted.clear();
        }
        if !clear_playing_item && let Some(playing_item) = playing_item {
            self.playlist.push(playing_item);
            if let Some(sorted) = self.sorted_playlist.as_mut() {
                sorted.push(playing_item);
            }
            self.playing_item_index = 0;
        } else {
            self.playing_item_index = -1;
        }
    }

    pub fn remove_from_playlist(&mut self, playlist_item_ids: &[Uuid]) -> bool {
        let playing_item = self.playing_item();
        self.playlist
            .retain(|item| !playlist_item_ids.contains(&item.playlist_item_id));
        if let Some(sorted) = self.sorted_playlist.as_mut() {
            sorted.retain(|item| !playlist_item_ids.contains(&item.playlist_item_id));
        }
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

    pub const fn set_repeat_mode(&mut self, mode: GroupRepeatMode) {
        self.repeat_mode = mode;
    }

    pub fn set_shuffle_mode(&mut self, mode: GroupShuffleMode) {
        match mode {
            GroupShuffleMode::Shuffle => self.shuffle_playlist(),
            GroupShuffleMode::Sorted => self.restore_sorted_playlist(),
        }
    }

    pub fn next_item(&mut self) -> bool {
        if self.repeat_mode == GroupRepeatMode::RepeatOne {
            return self.is_item_playing();
        }
        let candidate = self.playing_item_index + 1;
        if usize::try_from(candidate).is_ok_and(|index| index < self.playlist.len()) {
            self.playing_item_index = candidate;
            return true;
        }
        if self.repeat_mode == GroupRepeatMode::RepeatAll && !self.playlist.is_empty() {
            self.playing_item_index = 0;
            return true;
        }
        if !self.playlist.is_empty() {
            self.playing_item_index = i32::try_from(self.playlist.len() - 1).unwrap_or(i32::MAX);
        }
        false
    }

    pub fn previous(&mut self) -> bool {
        if self.repeat_mode == GroupRepeatMode::RepeatOne {
            return self.is_item_playing();
        }
        let candidate = self.playing_item_index - 1;
        if candidate >= 0 {
            self.playing_item_index = candidate;
            return true;
        }
        if self.repeat_mode == GroupRepeatMode::RepeatAll && !self.playlist.is_empty() {
            self.playing_item_index = i32::try_from(self.playlist.len() - 1).unwrap_or(i32::MAX);
            return true;
        }
        if !self.playlist.is_empty() {
            self.playing_item_index = 0;
        }
        false
    }

    fn shuffle_playlist(&mut self) {
        let playing_item = self.playing_item();
        if self.sorted_playlist.is_none() {
            self.sorted_playlist = Some(self.playlist.clone());
        }
        if let Some(playing_item) = playing_item {
            self.playlist
                .retain(|item| item.playlist_item_id != playing_item.playlist_item_id);
            self.playlist.shuffle(&mut rand::rng());
            self.playlist.insert(0, playing_item);
            self.playing_item_index = 0;
        } else {
            self.playlist.shuffle(&mut rand::rng());
        }
        self.shuffle_mode = GroupShuffleMode::Shuffle;
    }

    fn restore_sorted_playlist(&mut self) {
        let playing_item = self.playing_item();
        if let Some(sorted) = self.sorted_playlist.take() {
            self.playlist = sorted;
        }
        self.playing_item_index = playing_item
            .and_then(|playing| self.playlist.iter().position(|item| *item == playing))
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        self.shuffle_mode = GroupShuffleMode::Sorted;
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
    pub repeat_mode: GroupRepeatMode,
    pub shuffle_mode: GroupShuffleMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncPlayParticipantState {
    pub ping: i64,
    pub is_buffering: bool,
    pub ignore_wait: bool,
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
