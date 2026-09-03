use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

static NEXT_HELPER_ID: AtomicU64 = AtomicU64::new(1);

/// Explicit reentrancy state for one logical asynchronous task.
///
/// Passing the same context through nested calls gives Rust the equivalent of
/// Jellyfin's async-local nested-lock state without relying on thread-local
/// storage. Acquisition takes a mutable reference, preventing concurrent use.
#[derive(Debug, Default)]
pub struct UserManagerLockContext {
    inner: Arc<ContextInner>,
}

/// Errors returned by [`UserManagerLockHelper`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum UserManagerLockError {
    #[error("user manager lock helper has been disposed")]
    Disposed,
}

/// A keyed asynchronous lock with explicit task-context reentrancy.
#[derive(Debug)]
pub struct UserManagerLockHelper {
    id: u64,
    inner: Arc<HelperInner>,
}

impl Default for UserManagerLockHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl UserManagerLockHelper {
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: NEXT_HELPER_ID.fetch_add(1, Ordering::Relaxed),
            inner: Arc::new(HelperInner::default()),
        }
    }

    /// Returns whether a call in this context would acquire a keyed lock.
    #[must_use]
    pub fn should_lock(&self, context: &UserManagerLockContext) -> bool {
        !context.inner.contains(self.id)
    }

    /// Acquires `key`, unless this context is already inside this helper.
    ///
    /// Disposal is checked before the nested shortcut. A waiting call also
    /// rechecks disposal after acquiring its keyed mutex.
    ///
    /// # Errors
    ///
    /// Returns [`UserManagerLockError::Disposed`] if the helper was disposed
    /// before registration or while this call was waiting for the key.
    pub async fn lock_async(
        &self,
        context: &mut UserManagerLockContext,
        key: Uuid,
    ) -> Result<UserManagerLockHandle, UserManagerLockError> {
        self.inner.ensure_not_disposed()?;

        if context.inner.enter_nested(self.id) {
            return Ok(UserManagerLockHandle::new(
                Arc::clone(&context.inner),
                self.id,
            ));
        }

        let registration = self.inner.register(key)?;
        let guard = Arc::clone(&registration.key_lock).lock_owned().await;
        let lease = KeyLease::new(guard, registration);

        if self.inner.is_disposed() {
            drop(lease);
            return Err(UserManagerLockError::Disposed);
        }

        context.inner.enter_outer(self.id, lease);
        Ok(UserManagerLockHandle::new(
            Arc::clone(&context.inner),
            self.id,
        ))
    }

    /// Prevents subsequent acquisitions and releases key-map metadata.
    ///
    /// Existing holders remain valid. Existing waiters release their acquired
    /// mutex and return [`UserManagerLockError::Disposed`].
    pub fn dispose(&self) {
        let mut state = lock_unpoisoned(&self.inner.state);
        if state.disposed {
            return;
        }

        state.disposed = true;
        state.entries.clear();
    }
}

impl Drop for UserManagerLockHelper {
    fn drop(&mut self) {
        self.dispose();
    }
}

/// RAII handle returned by [`UserManagerLockHelper::lock_async`].
#[derive(Debug)]
pub struct UserManagerLockHandle {
    context: Arc<ContextInner>,
    helper_id: u64,
    active: bool,
}

impl UserManagerLockHandle {
    fn new(context: Arc<ContextInner>, helper_id: u64) -> Self {
        Self {
            context,
            helper_id,
            active: true,
        }
    }
}

impl Drop for UserManagerLockHandle {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;

        // Dropping outside the context mutex releases the Tokio mutex before
        // updating the keyed-entry registration and waking another waiter.
        let lease = self.context.release(self.helper_id);
        drop(lease);
    }
}

#[derive(Debug, Default)]
struct ContextInner {
    active: Mutex<HashMap<u64, ActiveLock>>,
}

impl ContextInner {
    fn contains(&self, helper_id: u64) -> bool {
        lock_unpoisoned(&self.active).contains_key(&helper_id)
    }

    fn enter_nested(&self, helper_id: u64) -> bool {
        let mut active = lock_unpoisoned(&self.active);
        let Some(lock) = active.get_mut(&helper_id) else {
            return false;
        };
        lock.depth = lock
            .depth
            .checked_add(1)
            .expect("user manager nested lock depth overflowed");
        true
    }

    fn enter_outer(&self, helper_id: u64, lease: KeyLease) {
        let previous =
            lock_unpoisoned(&self.active).insert(helper_id, ActiveLock { depth: 1, lease });
        debug_assert!(previous.is_none(), "lock context was used concurrently");
        drop(previous);
    }

    fn release(&self, helper_id: u64) -> Option<KeyLease> {
        let mut active = lock_unpoisoned(&self.active);
        let lock = active
            .get_mut(&helper_id)
            .expect("user manager lock handle had no matching context state");
        if lock.depth > 1 {
            lock.depth -= 1;
            return None;
        }

        active.remove(&helper_id).map(|lock| lock.lease)
    }
}

#[derive(Debug)]
struct ActiveLock {
    depth: usize,
    lease: KeyLease,
}

#[derive(Debug, Default)]
struct HelperInner {
    state: Mutex<HelperState>,
}

impl HelperInner {
    fn ensure_not_disposed(&self) -> Result<(), UserManagerLockError> {
        if lock_unpoisoned(&self.state).disposed {
            Err(UserManagerLockError::Disposed)
        } else {
            Ok(())
        }
    }

    fn is_disposed(&self) -> bool {
        lock_unpoisoned(&self.state).disposed
    }

    fn register(self: &Arc<Self>, key: Uuid) -> Result<KeyRegistration, UserManagerLockError> {
        let mut state = lock_unpoisoned(&self.state);
        if state.disposed {
            return Err(UserManagerLockError::Disposed);
        }

        let entry = state.entries.entry(key).or_insert_with(|| KeyEntry {
            key_lock: Arc::new(AsyncMutex::new(())),
            users: 0,
        });
        entry.users = entry
            .users
            .checked_add(1)
            .expect("user manager keyed lock user count overflowed");

        Ok(KeyRegistration {
            helper: Arc::clone(self),
            key,
            key_lock: Arc::clone(&entry.key_lock),
            active: true,
        })
    }

    fn unregister(&self, key: Uuid, key_lock: &Arc<AsyncMutex<()>>) {
        let mut state = lock_unpoisoned(&self.state);
        let should_remove = if let Some(entry) = state.entries.get_mut(&key) {
            if Arc::ptr_eq(&entry.key_lock, key_lock) {
                entry.users = entry
                    .users
                    .checked_sub(1)
                    .expect("user manager keyed lock user count underflowed");
                entry.users == 0
            } else {
                false
            }
        } else {
            false
        };

        if should_remove {
            state.entries.remove(&key);
        }
    }

    #[cfg(test)]
    fn entry_snapshot(&self, key: Uuid) -> (usize, Option<usize>) {
        let state = lock_unpoisoned(&self.state);
        (
            state.entries.len(),
            state.entries.get(&key).map(|entry| entry.users),
        )
    }
}

#[derive(Debug, Default)]
struct HelperState {
    disposed: bool,
    entries: HashMap<Uuid, KeyEntry>,
}

#[derive(Debug)]
struct KeyEntry {
    key_lock: Arc<AsyncMutex<()>>,
    users: usize,
}

#[derive(Debug)]
struct KeyRegistration {
    helper: Arc<HelperInner>,
    key: Uuid,
    key_lock: Arc<AsyncMutex<()>>,
    active: bool,
}

impl Drop for KeyRegistration {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.helper.unregister(self.key, &self.key_lock);
    }
}

#[derive(Debug)]
struct KeyLease {
    guard: Option<OwnedMutexGuard<()>>,
    registration: Option<KeyRegistration>,
}

impl KeyLease {
    fn new(guard: OwnedMutexGuard<()>, registration: KeyRegistration) -> Self {
        Self {
            guard: Some(guard),
            registration: Some(registration),
        }
    }
}

impl Drop for KeyLease {
    fn drop(&mut self) {
        drop(self.guard.take());
        drop(self.registration.take());
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::{sync::Notify, time::timeout};

    use super::{UserManagerLockContext, UserManagerLockError, UserManagerLockHelper};

    #[tokio::test]
    async fn cancelled_waiter_releases_its_registration_and_last_holder_removes_entry() {
        let helper = Arc::new(UserManagerLockHelper::new());
        let key = uuid::Uuid::new_v4();
        let holder_acquired = Arc::new(Notify::new());
        let release_holder = Arc::new(Notify::new());

        let holder = tokio::spawn({
            let helper = Arc::clone(&helper);
            let holder_acquired = Arc::clone(&holder_acquired);
            let release_holder = Arc::clone(&release_holder);
            async move {
                let mut context = UserManagerLockContext::default();
                let _handle = helper.lock_async(&mut context, key).await.unwrap();
                holder_acquired.notify_one();
                release_holder.notified().await;
            }
        });
        holder_acquired.notified().await;
        assert_eq!(helper.inner.entry_snapshot(key), (1, Some(1)));

        let waiter = tokio::spawn({
            let helper = Arc::clone(&helper);
            async move {
                let mut context = UserManagerLockContext::default();
                let _handle = helper.lock_async(&mut context, key).await.unwrap();
            }
        });

        timeout(Duration::from_secs(1), async {
            while helper.inner.entry_snapshot(key) != (1, Some(2)) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("waiter did not register");

        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(helper.inner.entry_snapshot(key), (1, Some(1)));

        release_holder.notify_one();
        timeout(Duration::from_secs(1), holder)
            .await
            .expect("holder did not finish")
            .unwrap();
        assert_eq!(helper.inner.entry_snapshot(key), (0, None));
    }

    #[tokio::test]
    async fn registered_waiter_rechecks_disposal_after_acquiring_the_key() {
        let helper = Arc::new(UserManagerLockHelper::new());
        let key = uuid::Uuid::new_v4();
        let mut holder_context = UserManagerLockContext::default();
        let holder = helper.lock_async(&mut holder_context, key).await.unwrap();

        let waiter = tokio::spawn({
            let helper = Arc::clone(&helper);
            async move {
                let mut context = UserManagerLockContext::default();
                helper.lock_async(&mut context, key).await
            }
        });

        timeout(Duration::from_secs(1), async {
            while helper.inner.entry_snapshot(key) != (1, Some(2)) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("waiter did not register");

        helper.dispose();
        assert_eq!(helper.inner.entry_snapshot(key), (0, None));
        drop(holder);

        let result = timeout(Duration::from_secs(1), waiter)
            .await
            .expect("disposed waiter did not finish")
            .unwrap();
        assert_eq!(result.unwrap_err(), UserManagerLockError::Disposed);
    }
}
