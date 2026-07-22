use std::{sync::Arc, time::Duration};

use jellyfin_server_implementations::{
    UserManagerLockContext, UserManagerLockError, UserManagerLockHelper,
};
use tokio::{sync::Notify, time::timeout};
use uuid::Uuid;

#[tokio::test]
async fn nested_lock_does_not_acquire_twice_and_last_handle_restores_state() {
    let helper = UserManagerLockHelper::new();
    let mut context = UserManagerLockContext::default();
    let key = Uuid::new_v4();

    assert!(helper.should_lock(&context));

    let outer = helper.lock_async(&mut context, key).await.unwrap();
    assert!(!helper.should_lock(&context));

    let inner = helper.lock_async(&mut context, key).await.unwrap();
    assert!(!helper.should_lock(&context));

    drop(inner);
    assert!(!helper.should_lock(&context));

    drop(outer);
    assert!(helper.should_lock(&context));
}

#[tokio::test]
async fn same_key_in_different_tasks_blocks_until_first_handle_is_released() {
    let helper = Arc::new(UserManagerLockHelper::new());
    let key = Uuid::new_v4();
    let first_acquired = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let second_started = Arc::new(Notify::new());
    let second_acquired = Arc::new(Notify::new());

    let first = tokio::spawn({
        let helper = Arc::clone(&helper);
        let first_acquired = Arc::clone(&first_acquired);
        let release_first = Arc::clone(&release_first);
        async move {
            let mut context = UserManagerLockContext::default();
            let _handle = helper.lock_async(&mut context, key).await.unwrap();
            first_acquired.notify_one();
            release_first.notified().await;
        }
    });
    first_acquired.notified().await;

    let second = tokio::spawn({
        let helper = Arc::clone(&helper);
        let second_started = Arc::clone(&second_started);
        let second_acquired = Arc::clone(&second_acquired);
        async move {
            let mut context = UserManagerLockContext::default();
            second_started.notify_one();
            let _handle = helper.lock_async(&mut context, key).await.unwrap();
            second_acquired.notify_one();
        }
    });
    second_started.notified().await;

    assert!(
        timeout(Duration::from_millis(50), second_acquired.notified())
            .await
            .is_err()
    );
    release_first.notify_one();
    timeout(Duration::from_secs(1), second_acquired.notified())
        .await
        .expect("second task did not acquire the released key");
    timeout(Duration::from_secs(1), async {
        first.await.unwrap();
        second.await.unwrap();
    })
    .await
    .expect("same-key tasks did not finish");
}

#[tokio::test]
async fn lock_after_dispose_returns_typed_error() {
    let helper = UserManagerLockHelper::new();
    let mut context = UserManagerLockContext::default();
    helper.dispose();

    let result = helper.lock_async(&mut context, Uuid::new_v4()).await;

    assert_eq!(result.unwrap_err(), UserManagerLockError::Disposed);
}

#[test]
fn repeated_dispose_is_idempotent() {
    let helper = UserManagerLockHelper::new();

    helper.dispose();
    helper.dispose();
}

#[tokio::test]
async fn different_keys_acquire_concurrently() {
    let helper = Arc::new(UserManagerLockHelper::new());
    let first_key = Uuid::new_v4();
    let second_key = Uuid::new_v4();
    let first_acquired = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let second_acquired = Arc::new(Notify::new());

    let first = tokio::spawn({
        let helper = Arc::clone(&helper);
        let first_acquired = Arc::clone(&first_acquired);
        let release_first = Arc::clone(&release_first);
        async move {
            let mut context = UserManagerLockContext::default();
            let _handle = helper.lock_async(&mut context, first_key).await.unwrap();
            first_acquired.notify_one();
            release_first.notified().await;
        }
    });
    first_acquired.notified().await;

    let second = tokio::spawn({
        let helper = Arc::clone(&helper);
        let second_acquired = Arc::clone(&second_acquired);
        async move {
            let mut context = UserManagerLockContext::default();
            let _handle = helper.lock_async(&mut context, second_key).await.unwrap();
            second_acquired.notify_one();
        }
    });

    timeout(Duration::from_secs(1), second_acquired.notified())
        .await
        .expect("different key was unnecessarily blocked");
    release_first.notify_one();
    timeout(Duration::from_secs(1), async {
        first.await.unwrap();
        second.await.unwrap();
    })
    .await
    .expect("different-key tasks did not finish");
}
