use chrono::Utc;
use jellyfin_model::{
    BufferRequestDto, GroupQueueMode, GroupRepeatMode, GroupShuffleMode, GroupStateType,
    PlayQueueUpdateReason, PlaybackRequestType, ReadyRequestDto, SendCommandType,
};
use jellyfin_server_implementations::{SyncPlayGroupUpdate, SyncPlayManager, SyncPlaySession};
use uuid::Uuid;

fn session(session_id: i64, user_id: Uuid, user_name: &str) -> SyncPlaySession {
    SyncPlaySession {
        session_id: session_id.to_string(),
        user_id,
        user_name: user_name.to_owned(),
    }
}

fn update_type(update: &SyncPlayGroupUpdate) -> &str {
    update.payload["Type"].as_str().unwrap()
}

#[tokio::test]
async fn sync_play_manager_moves_sessions_and_removes_empty_groups() {
    let manager = SyncPlayManager::new();
    let alice_id = Uuid::new_v4();
    let bob_id = Uuid::new_v4();
    let first = manager
        .create_group(session(1, alice_id, "alice"), "First".to_owned())
        .await;
    assert!(
        manager
            .join_group(session(2, bob_id, "bob"), first.group_id)
            .await
    );
    assert!(
        manager
            .join_group(session(3, alice_id, "alice"), first.group_id)
            .await
    );

    let joined = manager.get_group(first.group_id).await.unwrap();
    assert_eq!(joined.participants, ["alice", "bob"]);
    assert!(manager.is_user_active(alice_id).await);
    assert!(manager.is_session_active("1").await);

    let second = manager
        .create_group(session(1, alice_id, "alice"), "Second".to_owned())
        .await;
    assert_ne!(first.group_id, second.group_id);
    assert_eq!(
        manager
            .get_group(first.group_id)
            .await
            .unwrap()
            .participants,
        ["alice", "bob"]
    );
    assert_eq!(manager.list_groups().await.len(), 2);

    assert!(manager.leave_group("2").await);
    assert!(manager.leave_group("3").await);
    assert!(manager.get_group(first.group_id).await.is_none());
    assert!(!manager.leave_group("999").await);
    assert!(manager.leave_group("1").await);
    assert!(manager.list_groups().await.is_empty());
}

#[tokio::test]
async fn sync_play_manager_rejects_unknown_groups_without_moving_session() {
    let manager = SyncPlayManager::new();
    let user_id = Uuid::new_v4();
    let created = manager
        .create_group(session(1, user_id, "alice"), "Existing".to_owned())
        .await;

    assert!(
        !manager
            .join_group(session(1, user_id, "alice"), Uuid::new_v4())
            .await
    );
    assert!(manager.get_group(created.group_id).await.is_some());
    assert!(manager.is_session_active("1").await);
}

#[tokio::test]
async fn sync_play_manager_emits_official_membership_updates() {
    let manager = SyncPlayManager::new();
    let alice = session(1, Uuid::new_v4(), "alice");
    let bob = session(2, Uuid::new_v4(), "bob");
    let (group, created) = manager
        .create_group_with_updates(alice.clone(), "Members".to_owned())
        .await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].session_ids, ["1"]);
    assert_eq!(update_type(&created[0]), "GroupJoined");

    let joined = manager
        .join_group_with_updates(bob.clone(), group.group_id)
        .await
        .unwrap();
    assert_eq!(joined.len(), 2);
    assert_eq!(joined[0].session_ids, ["2"]);
    assert_eq!(update_type(&joined[0]), "GroupJoined");
    assert_eq!(joined[1].session_ids, ["1"]);
    assert_eq!(update_type(&joined[1]), "UserJoined");
    assert_eq!(joined[1].payload["Data"], "bob");
    assert!(
        manager
            .join_group_with_updates(bob.clone(), group.group_id)
            .await
            .unwrap()
            .is_empty()
    );

    let left = manager.leave_group_with_updates(&bob).await.unwrap();
    assert_eq!(update_type(&left[0]), "GroupLeft");
    assert_eq!(left[0].session_ids, ["2"]);
    assert_eq!(update_type(&left[1]), "UserLeft");
    assert_eq!(left[1].session_ids, ["1"]);
}

#[tokio::test]
async fn sync_play_manager_applies_queue_requests_only_for_group_sessions() {
    let manager = SyncPlayManager::new();
    let user_id = Uuid::new_v4();
    let item_ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    manager
        .create_group(session(1, user_id, "alice"), "Queue".to_owned())
        .await;

    assert!(manager.set_new_queue("1", &item_ids, 1, 42).await);
    let initial = manager.queue_state_for_session("1").await.unwrap();
    assert_eq!(initial.playing_item_index, 1);
    assert_eq!(initial.position_ticks, 42);
    assert_eq!(queue_item_ids(&initial), item_ids);
    let playing_playlist_id = initial.items[1].playlist_item_id;

    let next = Uuid::new_v4();
    assert!(
        manager
            .queue_items("1", &[next], GroupQueueMode::QueueNext)
            .await
    );
    assert_eq!(
        queue_item_ids(&manager.queue_state_for_session("1").await.unwrap()),
        [item_ids[0], item_ids[1], next, item_ids[2]]
    );
    assert!(manager.set_playlist_item("1", playing_playlist_id).await);
    assert!(manager.remove_from_playlist("1", &[], true, false).await);
    let preserved = manager.queue_state_for_session("1").await.unwrap();
    assert_eq!(preserved.items.len(), 1);
    assert_eq!(preserved.items[0].playlist_item_id, playing_playlist_id);
    assert_eq!(preserved.playing_item_index, 0);

    assert!(manager.remove_from_playlist("1", &[], true, true).await);
    let cleared = manager.queue_state_for_session("1").await.unwrap();
    assert!(cleared.items.is_empty());
    assert_eq!(cleared.playing_item_index, -1);
    assert!(!manager.set_new_queue("missing", &item_ids, 0, 0).await);
    assert!(!manager.set_new_queue("1", &[], 0, 0).await);
    assert!(!manager.set_new_queue("1", &item_ids, 3, 0).await);
}

fn queue_item_ids(state: &jellyfin_server_implementations::SyncPlayQueueState) -> Vec<Uuid> {
    state.items.iter().map(|item| item.item_id).collect()
}

#[tokio::test]
async fn sync_play_manager_controls_playback_state_and_navigation() {
    let manager = SyncPlayManager::new();
    let user_id = Uuid::new_v4();
    let items = [Uuid::new_v4(), Uuid::new_v4()];
    let group = manager
        .create_group(session(1, user_id, "alice"), "Playback".to_owned())
        .await;
    assert!(manager.set_new_queue("1", &items, 0, 50).await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Waiting
    );

    assert!(manager.unpause("1").await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Playing
    );
    assert!(manager.pause("1").await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Paused
    );
    assert!(manager.seek("1", 500, 300).await);
    let sought = manager.queue_state_for_session("1").await.unwrap();
    assert_eq!(sought.position_ticks, 300);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Waiting
    );

    let current = sought.items[0].playlist_item_id;
    assert!(!manager.next_item("1", Uuid::new_v4()).await);
    assert!(manager.next_item("1", current).await);
    assert_eq!(
        manager
            .queue_state_for_session("1")
            .await
            .unwrap()
            .playing_item_index,
        1
    );
    assert!(
        manager
            .set_repeat_mode("1", GroupRepeatMode::RepeatAll)
            .await
    );
    let current = manager.queue_state_for_session("1").await.unwrap().items[1].playlist_item_id;
    assert!(manager.next_item("1", current).await);
    assert_eq!(
        manager
            .queue_state_for_session("1")
            .await
            .unwrap()
            .playing_item_index,
        0
    );
    assert!(
        manager
            .set_shuffle_mode("1", GroupShuffleMode::Shuffle)
            .await
    );
    assert!(
        manager
            .set_shuffle_mode("1", GroupShuffleMode::Sorted)
            .await
    );
    assert!(
        manager
            .set_repeat_mode("1", GroupRepeatMode::RepeatOne)
            .await
    );
    assert!(
        manager
            .set_shuffle_mode("1", GroupShuffleMode::Shuffle)
            .await
    );
    assert!(manager.set_new_queue("1", &items, 0, 5).await);
    let reset = manager.queue_state_for_session("1").await.unwrap();
    assert_eq!(reset.playing_item_index, 0);
    assert_eq!(reset.repeat_mode, GroupRepeatMode::RepeatNone);
    assert_eq!(reset.shuffle_mode, GroupShuffleMode::Sorted);

    assert!(manager.stop("1").await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Idle
    );
    assert_eq!(
        manager
            .queue_state_for_session("1")
            .await
            .unwrap()
            .position_ticks,
        0
    );
    assert!(!manager.seek("1", 10, 300).await);
}

#[tokio::test]
async fn sync_play_manager_waits_for_every_following_participant() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(
            session(1, Uuid::new_v4(), "alice"),
            "Coordination".to_owned(),
        )
        .await;
    assert!(
        manager
            .join_group(session(2, Uuid::new_v4(), "bob"), group.group_id)
            .await
    );
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 50).await);
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    assert!(
        manager
            .participant_state_for_session("1")
            .await
            .unwrap()
            .is_buffering
    );

    let ready = ReadyRequestDto {
        when: Utc::now(),
        position_ticks: 50,
        is_playing: false,
        playlist_item_id,
    };
    assert!(manager.ready("1", ready, 1_000).await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Waiting
    );
    assert!(manager.ready("2", ready, 1_000).await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Playing
    );

    assert!(manager.pause("1").await);
    assert!(manager.seek("1", 100, 10_000_000).await);
    assert!(manager.ready("1", ready, 10_000_000).await);
    assert!(
        manager
            .ready(
                "2",
                ReadyRequestDto {
                    position_ticks: 6_000_101,
                    ..ready
                },
                10_000_000,
            )
            .await
    );
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Waiting
    );
    assert!(manager.set_ignore_wait("2", true).await);
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Paused
    );
}

#[tokio::test]
async fn sync_play_manager_tracks_buffering_and_ping_per_session() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(
            session(1, Uuid::new_v4(), "alice"),
            "Diagnostics".to_owned(),
        )
        .await;
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 10).await);
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    assert!(
        manager
            .ready(
                "1",
                ReadyRequestDto {
                    when: Utc::now(),
                    position_ticks: 10,
                    is_playing: false,
                    playlist_item_id,
                },
                100,
            )
            .await
    );
    assert!(manager.update_ping("1", 37).await);
    assert!(!manager.update_ping("missing", 1).await);
    assert_eq!(
        manager
            .participant_state_for_session("1")
            .await
            .unwrap()
            .ping,
        37
    );

    assert!(
        manager
            .buffering(
                "1",
                BufferRequestDto {
                    when: Utc::now(),
                    position_ticks: 80,
                    is_playing: true,
                    playlist_item_id,
                },
                100,
            )
            .await
    );
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Waiting
    );
    assert_eq!(
        manager
            .queue_state_for_session("1")
            .await
            .unwrap()
            .position_ticks,
        80
    );
}

#[tokio::test]
async fn sync_play_manager_leaves_only_after_the_last_websocket_disconnects() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(session(1, Uuid::new_v4(), "alice"), "Sockets".to_owned())
        .await;
    assert!(
        manager
            .join_group(session(2, Uuid::new_v4(), "bob"), group.group_id)
            .await
    );
    manager.websocket_connected("1").await;
    manager.websocket_connected("1").await;

    assert!(
        manager
            .websocket_disconnected_with_updates("1")
            .await
            .is_none()
    );
    assert!(manager.get_group(group.group_id).await.is_some());
    let updates = manager
        .websocket_disconnected_with_updates("1")
        .await
        .unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(update_type(&updates[0]), "GroupLeft");
    assert_eq!(updates[0].session_ids, ["1"]);
    assert_eq!(update_type(&updates[1]), "UserLeft");
    assert_eq!(updates[1].session_ids, ["2"]);
    assert_eq!(updates[1].payload["Data"], "alice");
    assert_eq!(
        manager
            .get_group(group.group_id)
            .await
            .unwrap()
            .participants,
        ["bob"]
    );
    assert!(!manager.websocket_disconnected("1").await);
}

#[tokio::test]
async fn sync_play_manager_resumes_when_the_last_buffering_member_leaves() {
    let manager = SyncPlayManager::new();
    let alice = session(1, Uuid::new_v4(), "alice");
    let bob = session(2, Uuid::new_v4(), "bob");
    let group = manager
        .create_group(alice.clone(), "Departure".to_owned())
        .await;
    assert!(manager.join_group(bob.clone(), group.group_id).await);
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 42).await);
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    assert!(
        manager
            .ready(
                "1",
                ReadyRequestDto {
                    when: Utc::now(),
                    position_ticks: 42,
                    is_playing: false,
                    playlist_item_id,
                },
                100,
            )
            .await
    );

    let departure = manager.leave_group_with_departure(&bob).await.unwrap();
    let (sessions, command) = departure.playback_command.unwrap();
    assert_eq!(sessions, ["1", "2"]);
    assert_eq!(command.command, SendCommandType::Unpause);
    assert_eq!(command.group_id, group.group_id);
    assert_state_update(
        departure.state_update.as_ref().unwrap(),
        "Playing",
        "Unpause",
    );
    assert_eq!(departure.membership_updates.len(), 2);
    assert_eq!(update_type(&departure.membership_updates[0]), "GroupLeft");
    assert_eq!(update_type(&departure.membership_updates[1]), "UserLeft");
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Playing
    );
}

#[tokio::test]
async fn sync_play_manager_settles_paused_without_an_update_when_a_member_leaves() {
    let manager = SyncPlayManager::new();
    let alice = session(1, Uuid::new_v4(), "alice");
    let bob = session(2, Uuid::new_v4(), "bob");
    let group = manager
        .create_group(alice, "Paused Departure".to_owned())
        .await;
    assert!(manager.join_group(bob.clone(), group.group_id).await);
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 42).await);
    assert!(manager.unpause("1").await);
    assert!(manager.pause("1").await);
    assert!(manager.seek("1", 42, 100).await);
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    assert!(
        manager
            .ready(
                "1",
                ReadyRequestDto {
                    when: Utc::now(),
                    position_ticks: 42,
                    is_playing: false,
                    playlist_item_id,
                },
                100,
            )
            .await
    );

    let departure = manager.leave_group_with_departure(&bob).await.unwrap();
    assert!(departure.playback_command.is_none());
    assert!(departure.state_update.is_none());
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Paused
    );
}

#[tokio::test]
async fn sync_play_manager_builds_commands_for_every_group_session() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(session(2, Uuid::new_v4(), "alice"), "Commands".to_owned())
        .await;
    assert!(
        manager
            .join_group(session(1, Uuid::new_v4(), "bob"), group.group_id)
            .await
    );
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 42).await);

    let (sessions, command) = manager
        .playback_command_for_session("1", SendCommandType::Seek)
        .await
        .unwrap();
    assert_eq!(sessions, ["1", "2"]);
    assert_eq!(command.group_id, group.group_id);
    assert_eq!(command.command, SendCommandType::Seek);
    assert_eq!(command.position_ticks, Some(42));
    assert!(!command.playlist_item_id.is_nil());
    assert!(
        manager
            .playback_command_for_session("missing", SendCommandType::Stop)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn sync_play_manager_builds_official_queue_updates() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(
            session(1, Uuid::new_v4(), "alice"),
            "Queue Updates".to_owned(),
        )
        .await;
    let items = [Uuid::new_v4(), Uuid::new_v4()];
    assert!(manager.set_new_queue("1", &items, 1, 42).await);

    let (sessions, update) = manager
        .queue_update_for_session("1", PlayQueueUpdateReason::NewPlaylist)
        .await
        .unwrap();
    assert_eq!(sessions, ["1"]);
    assert_eq!(update.group_id, group.group_id);
    assert_eq!(update.data.reason, PlayQueueUpdateReason::NewPlaylist);
    assert_eq!(update.data.playing_item_index, 1);
    assert_eq!(update.data.start_position_ticks, 42);
    assert_eq!(update.data.playlist[0].item_id, items[0]);
    assert_ne!(
        update.data.playlist[0].playlist_item_id,
        update.data.playlist[1].playlist_item_id
    );
}

#[tokio::test]
async fn sync_play_manager_builds_official_state_updates() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(
            session(2, Uuid::new_v4(), "alice"),
            "State Updates".to_owned(),
        )
        .await;
    assert!(
        manager
            .join_group(session(1, Uuid::new_v4(), "bob"), group.group_id)
            .await
    );
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 42).await);

    let (sessions, update) = manager
        .state_update_for_session("1", PlaybackRequestType::Play)
        .await
        .unwrap();
    assert_eq!(sessions, ["1", "2"]);
    assert_eq!(update.group_id, group.group_id);
    assert_eq!(
        update.update_type,
        jellyfin_model::GroupUpdateType::StateUpdate
    );
    assert_eq!(update.data.state, GroupStateType::Waiting);
    assert_eq!(update.data.reason, PlaybackRequestType::Play);
    assert!(
        manager
            .state_update_for_session("missing", PlaybackRequestType::Stop)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn sync_play_manager_emits_state_updates_only_for_official_transitions() {
    let manager = SyncPlayManager::new();
    let group = manager
        .create_group(
            session(1, Uuid::new_v4(), "alice"),
            "Transitions".to_owned(),
        )
        .await;
    assert!(
        manager
            .join_group(session(2, Uuid::new_v4(), "bob"), group.group_id)
            .await
    );
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 42).await);

    let (applied, update) = manager.unpause_with_update("1").await;
    assert!(applied);
    assert_state_update(update.as_ref().unwrap(), "Playing", "Unpause");
    let (_, update) = manager.unpause_with_update("1").await;
    assert!(update.is_none());

    let (_, update) = manager.pause_with_update("1").await;
    assert_state_update(update.as_ref().unwrap(), "Paused", "Pause");
    let (_, update) = manager.pause_with_update("1").await;
    assert!(update.is_none());

    let (_, update) = manager.seek_with_update("1", 20, 100).await;
    assert_state_update(update.as_ref().unwrap(), "Waiting", "Seek");
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    let (_, update) = manager
        .ready_with_update(
            "1",
            ReadyRequestDto {
                when: Utc::now(),
                position_ticks: 20,
                is_playing: false,
                playlist_item_id,
            },
            100,
        )
        .await;
    assert!(update.is_none());

    let (_, update) = manager.set_ignore_wait_with_update("2", true).await;
    assert!(update.is_none());
    assert_eq!(
        manager.get_group(group.group_id).await.unwrap().state,
        GroupStateType::Paused
    );

    assert!(manager.set_ignore_wait("2", false).await);
    assert!(manager.set_new_queue("1", &[Uuid::new_v4()], 0, 10).await);
    let playlist_item_id =
        manager.queue_state_for_session("1").await.unwrap().items[0].playlist_item_id;
    let (_, update) = manager
        .ready_with_update(
            "1",
            ReadyRequestDto {
                when: Utc::now(),
                position_ticks: 10,
                is_playing: false,
                playlist_item_id,
            },
            100,
        )
        .await;
    assert!(update.is_none());
    let (_, update) = manager.set_ignore_wait_with_update("2", true).await;
    assert_state_update(update.as_ref().unwrap(), "Playing", "Unpause");

    let (_, update) = manager
        .buffering_with_update(
            "1",
            BufferRequestDto {
                when: Utc::now(),
                position_ticks: 15,
                is_playing: true,
                playlist_item_id,
            },
            100,
        )
        .await;
    assert_state_update(update.as_ref().unwrap(), "Waiting", "Buffer");
}

fn assert_state_update(update: &SyncPlayGroupUpdate, state: &str, reason: &str) {
    assert_eq!(update.payload["Type"], "StateUpdate");
    assert_eq!(update.payload["Data"]["State"], state);
    assert_eq!(update.payload["Data"]["Reason"], reason);
}
