use jellyfin_model::{GroupQueueMode, GroupRepeatMode, GroupShuffleMode, GroupStateType};
use jellyfin_server_implementations::{SyncPlayManager, SyncPlaySession};
use uuid::Uuid;

fn session(session_id: i64, user_id: Uuid, user_name: &str) -> SyncPlaySession {
    SyncPlaySession {
        session_id: session_id.to_string(),
        user_id,
        user_name: user_name.to_owned(),
    }
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
