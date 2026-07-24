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
