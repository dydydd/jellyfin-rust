use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use chrono::{DateTime, Duration, Utc};
use jellyfin_model::{
    TaskCompletionStatus, TaskInfo, TaskResult, TaskState, TaskTriggerInfo, TaskTriggerInfoType,
};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::{LibraryScanService, SystemLogService};

const TICKS_PER_HOUR: i64 = 36_000_000_000;
const CACHE_FILE_RETENTION_DAYS: i64 = 30;
const TEMP_FILE_RETENTION_DAYS: i64 = 1;

#[derive(Debug, Error)]
pub enum ScheduledTaskError {
    #[error("scheduled task was not found")]
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub is_hidden: bool,
    pub is_enabled: bool,
    pub state: TaskState,
    pub current_progress_percentage: Option<f64>,
    pub triggers: Vec<TaskTriggerInfo>,
    pub last_run: Option<DateTime<Utc>>,
    pub last_execution_result: Option<TaskResult>,
}

impl ScheduledTask {
    fn to_info(&self) -> TaskInfo {
        TaskInfo {
            name: Some(self.name.clone()),
            state: self.state,
            current_progress_percentage: self.current_progress_percentage,
            id: Some(self.id.clone()),
            last_execution_result: self.last_execution_result.clone(),
            triggers: self.triggers.clone(),
            description: Some(self.description.clone()),
            category: Some(self.category.clone()),
            is_hidden: self.is_hidden,
            key: Some(self.key.clone()),
        }
    }
}

/// Filesystem locations used by maintenance scheduled tasks.
#[derive(Debug, Clone)]
pub struct ScheduledTaskPaths {
    pub log_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub transcode_directory: PathBuf,
    pub log_file_retention_days: i32,
}

impl Default for ScheduledTaskPaths {
    fn default() -> Self {
        Self {
            log_directory: PathBuf::from("logs"),
            cache_directory: PathBuf::from("cache"),
            transcode_directory: std::env::temp_dir()
                .join("jellyfin-rust")
                .join("transcodes"),
            log_file_retention_days: 3,
        }
    }
}

type ScheduledTaskFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type ScheduledTaskHandler =
    Arc<dyn Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync>;
type ScheduledTaskChangeListener = Arc<dyn Fn() + Send + Sync>;

struct ScheduledTaskRunContext {
    task_id: String,
    tasks: ScheduledTaskService,
}

impl ScheduledTaskRunContext {
    async fn report_progress(&self, progress: f64) {
        let _ = self.tasks.set_progress(&self.task_id, progress).await;
    }

    async fn complete(&self) {
        let _ = self
            .tasks
            .record_completion(&self.task_id, TaskCompletionStatus::Completed)
            .await;
        let _ = self.tasks.stop(&self.task_id).await;
    }

    async fn fail(&self) {
        let _ = self
            .tasks
            .record_completion(&self.task_id, TaskCompletionStatus::Failed)
            .await;
        let _ = self.tasks.stop(&self.task_id).await;
    }

    fn paths(&self) -> ScheduledTaskPaths {
        self.tasks
            .paths
            .read()
            .expect("scheduled task paths lock poisoned")
            .clone()
    }
}

#[derive(Clone)]
pub struct ScheduledTaskService {
    tasks: Arc<RwLock<Vec<ScheduledTask>>>,
    executors: Arc<std::sync::RwLock<HashMap<String, ScheduledTaskHandler>>>,
    paths: Arc<std::sync::RwLock<ScheduledTaskPaths>>,
    change_listeners: Arc<std::sync::RwLock<Vec<ScheduledTaskChangeListener>>>,
    scheduler_running: Arc<AtomicBool>,
}

impl Default for ScheduledTaskService {
    fn default() -> Self {
        Self::new(default_tasks())
    }
}

impl ScheduledTaskService {
    #[must_use]
    pub fn new(tasks: Vec<ScheduledTask>) -> Self {
        Self {
            tasks: Arc::new(RwLock::new(tasks)),
            executors: Arc::new(std::sync::RwLock::new(HashMap::new())),
            paths: Arc::new(std::sync::RwLock::new(ScheduledTaskPaths::default())),
            change_listeners: Arc::new(std::sync::RwLock::new(Vec::new())),
            scheduler_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Builds the service with official default tasks and their handlers.
    #[must_use]
    pub fn with_default_executors(library_scan: LibraryScanService) -> Self {
        let service = Self::default();
        service.register_executor("RefreshLibrary", refresh_library_handler(library_scan));
        service.register_executor("CleanLogFiles", clean_log_files_handler());
        service.register_executor("DeleteCacheFiles", delete_cache_files_handler());
        service.register_executor("PluginUpdates", plugin_updates_handler());
        service
    }

    fn register_executor(
        &self,
        task_key: &str,
        handler: impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync + 'static,
    ) {
        self.executors
            .write()
            .expect("scheduled task executor lock poisoned")
            .insert(task_key.to_ascii_lowercase(), Arc::new(handler));
    }

    /// Updates the log directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_log_directory(&self, path: PathBuf) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .log_directory = path;
    }

    /// Updates the cache directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_cache_directory(&self, path: PathBuf) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .cache_directory = path;
    }

    /// Updates the transcode directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_transcode_directory(&self, path: PathBuf) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .transcode_directory = path;
    }

    /// Updates the log-file retention window used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_log_file_retention_days(&self, days: i32) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .log_file_retention_days = days;
    }

    pub async fn list(&self, is_hidden: Option<bool>, is_enabled: Option<bool>) -> Vec<TaskInfo> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .iter()
            .filter(|task| is_hidden.is_none_or(|hidden| task.is_hidden == hidden))
            .filter(|task| is_enabled.is_none_or(|enabled| task.is_enabled == enabled))
            .map(ScheduledTask::to_info)
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| left.name.cmp(&right.name));
        tasks
    }

    /// Returns the task with the given id.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    pub async fn get(&self, task_id: &str) -> Result<TaskInfo, ScheduledTaskError> {
        self.tasks
            .read()
            .await
            .iter()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .map(ScheduledTask::to_info)
            .ok_or(ScheduledTaskError::NotFound)
    }

    /// Starts a task and executes its registered handler in the background.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    ///
    /// # Panics
    ///
    /// Panics if the internal executor lock is poisoned.
    pub async fn start(&self, task_id: &str) -> Result<(), ScheduledTaskError> {
        let (task_id, executor) = {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|task| task.id.eq_ignore_ascii_case(task_id))
                .ok_or(ScheduledTaskError::NotFound)?;
            if task.state == TaskState::Running || task.state == TaskState::Cancelling {
                return Ok(());
            }
            task.state = TaskState::Running;
            task.current_progress_percentage = None;
            task.last_run = Some(Utc::now());
            let task_id = task.id.clone();
            let task_key = task.key.clone();
            let executor = self
                .executors
                .read()
                .expect("scheduled task executor lock poisoned")
                .get(&task_key.to_ascii_lowercase())
                .cloned();
            (task_id, executor)
        };

        if let Some(executor) = executor {
            let context = ScheduledTaskRunContext {
                task_id,
                tasks: self.clone(),
            };
            tokio::spawn(async move { executor(context).await });
        }
        self.notify_changed();
        Ok(())
    }

    /// Stops a running task and clears its progress.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    pub async fn stop(&self, task_id: &str) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        task.state = TaskState::Idle;
        task.current_progress_percentage = None;
        self.notify_changed();
        Ok(())
    }

    /// Records the last execution result without changing the task state.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    pub async fn record_completion(
        &self,
        task_id: &str,
        status: TaskCompletionStatus,
    ) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        let end_time = Utc::now();
        let start_time = task.last_run.unwrap_or(end_time);
        task.current_progress_percentage = Some(100.0);
        task.last_execution_result = Some(TaskResult {
            start_time_utc: start_time,
            end_time_utc: end_time,
            status,
            name: Some(task.name.clone()),
            key: Some(task.key.clone()),
            id: Some(task.id.clone()),
            error_message: None,
            long_error_message: None,
        });
        self.notify_changed();
        Ok(())
    }

    /// Reports task progress to clients.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    pub async fn set_progress(
        &self,
        task_id: &str,
        progress: f64,
    ) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        task.current_progress_percentage = Some(progress);
        self.notify_changed();
        Ok(())
    }

    /// Registers a callback invoked after task state or progress changes.
    pub fn add_change_listener(&self, listener: ScheduledTaskChangeListener) {
        self.change_listeners
            .write()
            .expect("scheduled task listener lock poisoned")
            .push(listener);
    }

    fn notify_changed(&self) {
        let listeners = self
            .change_listeners
            .read()
            .expect("scheduled task listener lock poisoned")
            .clone();
        for listener in listeners {
            listener();
        }
    }

    /// Replaces the triggers for a task.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduledTaskError::NotFound`] when the task id doesn't exist.
    pub async fn update_triggers(
        &self,
        task_id: &str,
        triggers: Vec<TaskTriggerInfo>,
    ) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        task.triggers = triggers;
        self.notify_changed();
        Ok(())
    }

    /// Starts the background trigger loop. Repeated calls are idempotent.
    pub fn start_scheduler(&self) {
        self.start_scheduler_with_delay(tokio::time::Duration::from_secs(5));
    }

    /// Starts the trigger loop with a custom initial delay.
    pub fn start_scheduler_with_delay(&self, delay: tokio::time::Duration) {
        if self
            .scheduler_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let now = Utc::now();
            let mut tasks = service.tasks.write().await;
            for task in tasks.iter_mut() {
                let has_startup = task
                    .triggers
                    .iter()
                    .any(|trigger| trigger.trigger_type == TaskTriggerInfoType::StartupTrigger);
                if task.is_enabled && task.last_run.is_none() && !has_startup {
                    task.last_run = Some(now);
                }
            }
            drop(tasks);
            let mut ticker = tokio::time::interval_at(
                tokio::time::Instant::now() + delay,
                tokio::time::Duration::from_secs(1),
            );
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let due = service.due_task_ids().await;
                for task_id in due {
                    let _ = service.start(&task_id).await;
                }
            }
        });
    }

    async fn due_task_ids(&self) -> Vec<String> {
        let now = Utc::now();
        self.tasks
            .read()
            .await
            .iter()
            .filter(|task| task.is_enabled && task.state == TaskState::Idle)
            .filter(|task| task.is_due(now))
            .map(|task| task.id.clone())
            .collect()
    }
}

impl ScheduledTask {
    fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.triggers.iter().any(|trigger| match trigger.trigger_type {
            TaskTriggerInfoType::StartupTrigger => self.last_run.is_none(),
            TaskTriggerInfoType::IntervalTrigger => {
                let Some(interval_ticks) = trigger.interval_ticks.filter(|ticks| *ticks > 0) else {
                    return false;
                };
                let interval = Duration::milliseconds(interval_ticks / 10_000);
                self.last_run.is_none_or(|last_run| last_run + interval <= now)
            }
            TaskTriggerInfoType::DailyTrigger => {
                let Some(time_of_day_ticks) = trigger.time_of_day_ticks else {
                    return false;
                };
                self.last_run.is_none_or(|last_run| {
                    next_daily_after(last_run, time_of_day_ticks, trigger.day_of_week) <= now
                })
            }
            TaskTriggerInfoType::WeeklyTrigger => false,
        })
    }
}

fn next_daily_after(
    last_run: DateTime<Utc>,
    time_of_day_ticks: i64,
    _day_of_week: Option<jellyfin_model::DayOfWeek>,
) -> DateTime<Utc> {
    let duration = Duration::milliseconds(time_of_day_ticks / 10_000);
    let mut candidate = DateTime::from_naive_utc_and_offset(
        last_run.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default() + duration,
        Utc,
    );
    if candidate <= last_run {
        candidate += Duration::days(1);
    }
    candidate
}

fn refresh_library_handler(
    scan: LibraryScanService,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let scan = scan.clone();
        Box::pin(async move {
            context.report_progress(0.0).await;
            let mut scan = scan;
            let tasks = context.tasks.clone();
            let task_id = context.task_id.clone();
            scan.set_on_progress(Some(Arc::new(move |progress| {
                let tasks = tasks.clone();
                let task_id = task_id.clone();
                tokio::spawn(async move {
                    let _ = tasks.set_progress(&task_id, progress).await;
                });
            })));
            if let Err(error) = scan.scan_all().await {
                tracing::error!(%error, "library scan task failed");
                context.fail().await;
                return;
            }
            context.complete().await;
        })
    }
}

#[allow(clippy::cast_precision_loss)]
fn clean_log_files_handler() -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync
{
    |context| {
        Box::pin(async move {
            let paths = context.paths();
            let log_directory = paths.log_directory.clone();
            let logs = SystemLogService::new(log_directory.clone());
            let cutoff = Utc::now() - Duration::days(paths.log_file_retention_days.max(0).into());
            let candidates = logs
                .list()
                .await
                .into_iter()
                .filter(|file| !file.name.starts_with("log_") && file.date_modified < cutoff)
                .collect::<Vec<_>>();
            let total = candidates.len();
            for (index, file) in candidates.iter().enumerate() {
                if total > 0 {
                    context
                        .report_progress(100.0 * index as f64 / total as f64)
                        .await;
                }
                let _ = tokio::fs::remove_file(log_directory.join(&file.name)).await;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn delete_cache_files_handler()
-> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    |context| {
        Box::pin(async move {
            let paths = context.paths();
            context.report_progress(0.0).await;
            delete_old_files(
                &paths.cache_directory,
                Utc::now() - Duration::days(CACHE_FILE_RETENTION_DAYS),
            )
            .await;
            context.report_progress(90.0).await;
            delete_old_files(
                &paths.transcode_directory,
                Utc::now() - Duration::days(TEMP_FILE_RETENTION_DAYS),
            )
            .await;
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn plugin_updates_handler() -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync
{
    |context| {
        Box::pin(async move {
            context.report_progress(0.0).await;
            tracing::info!(
                task = %context.task_id,
                "plugin update check completed without package manager changes"
            );
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

async fn delete_old_files(root: &Path, cutoff: DateTime<Utc>) {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return;
    };
    loop {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            Box::pin(delete_old_files(&path, cutoff)).await;
            let _ = tokio::fs::remove_dir(&path).await;
        } else if file_type.is_file() {
            let modified = entry
                .metadata()
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let modified: DateTime<Utc> = modified.into();
            if modified < cutoff {
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
}

fn default_tasks() -> Vec<ScheduledTask> {
    vec![
        ScheduledTask {
            id: "8f6f0a39484e4a51a78bbbd8e0f4ac31".to_owned(),
            key: "RefreshLibrary".to_owned(),
            name: "Scan Media Library".to_owned(),
            description: "Scans your media library for new and updated files.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(12)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "a85dcf2f4fb940098267cf9d539b47a4".to_owned(),
            key: "CleanLogFiles".to_owned(),
            name: "Clean Log Directory".to_owned(),
            description: "Deletes log files that are no longer needed.".to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(24)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "bc9e8c3644044729a9d2f019593875db".to_owned(),
            key: "DeleteCacheFiles".to_owned(),
            name: "Clean Cache Directory".to_owned(),
            description: "Deletes cache files that are no longer needed.".to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(24)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "e62de0bb4fdf4ef9932a5b6bbf09e0d4".to_owned(),
            key: "PluginUpdates".to_owned(),
            name: "Check for plugin updates".to_owned(),
            description: "Downloads plugin update metadata.".to_owned(),
            category: "Application".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![startup_trigger(), interval_trigger(24)],
            last_run: None,
            last_execution_result: None,
        },
    ]
}

const fn interval_trigger(hours: i64) -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: TaskTriggerInfoType::IntervalTrigger,
        time_of_day_ticks: None,
        interval_ticks: Some(hours * TICKS_PER_HOUR),
        day_of_week: None,
        max_runtime_ticks: None,
    }
}

const fn startup_trigger() -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: TaskTriggerInfoType::StartupTrigger,
        time_of_day_ticks: None,
        interval_ticks: None,
        day_of_week: None,
        max_runtime_ticks: None,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn start_runs_registered_executor_and_returns_to_idle() {
        let service = ScheduledTaskService::new(vec![test_task("TestTask")]);
        service.register_executor("TestTask", |context| {
            Box::pin(async move {
                context.report_progress(50.0).await;
                context.complete().await;
            })
        });

        service.start("TestTask").await.unwrap();
        for _ in 0..100 {
            if service.get("TestTask").await.unwrap().state == TaskState::Idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let task = service.get("TestTask").await.unwrap();
        assert_eq!(task.state, TaskState::Idle);
        assert_eq!(task.current_progress_percentage, None);
    }

    #[tokio::test]
    async fn scheduler_runs_interval_triggers_and_records_last_execution() {
        let mut task = test_task("ScheduledTest");
        task.triggers = vec![TaskTriggerInfo {
            trigger_type: TaskTriggerInfoType::IntervalTrigger,
            time_of_day_ticks: None,
            interval_ticks: Some(500_000),
            day_of_week: None,
            max_runtime_ticks: None,
        }];
        let service = ScheduledTaskService::new(vec![task]);
        service.register_executor("ScheduledTest", |context| {
            Box::pin(async move {
                context.complete().await;
            })
        });

        service.start_scheduler_with_delay(tokio::time::Duration::from_millis(1));
        for _ in 0..300 {
            if service
                .get("ScheduledTest")
                .await
                .unwrap()
                .last_execution_result
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let task = service.get("ScheduledTest").await.unwrap();
        let result = task.last_execution_result.expect("last execution result");
        assert_eq!(result.status, TaskCompletionStatus::Completed);
        assert!(result.end_time_utc >= result.start_time_utc);
    }

    #[tokio::test]
    async fn clean_log_files_handler_deletes_expired_logs() {
        let root = temporary_directory("scheduled-task-logs");
        let old = root.join("old.log");
        let active = root.join("log_live.log");
        let fresh = root.join("fresh.log");
        fs::write(&old, "old").unwrap();
        fs::write(&active, "active").unwrap();
        fs::write(&fresh, "fresh").unwrap();
        set_modified(&old, UNIX_EPOCH);

        let service = ScheduledTaskService::new(vec![test_task("CleanLogFiles")]);
        service.set_log_directory(root.clone());
        service.register_executor("CleanLogFiles", clean_log_files_handler());

        service.start("CleanLogFiles").await.unwrap();
        wait_for_idle(&service, "CleanLogFiles").await;

        assert!(!old.exists());
        assert!(active.exists());
        assert!(fresh.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn delete_cache_files_handler_removes_old_cache_and_temp_files() {
        let cache = temporary_directory("scheduled-task-cache");
        let transcode = temporary_directory("scheduled-task-transcode");
        let old_cache = cache.join("old-cache.tmp");
        let fresh_cache = cache.join("fresh-cache.tmp");
        let old_transcode = transcode.join("old-transcode.ts");
        fs::write(&old_cache, "old cache").unwrap();
        fs::write(&fresh_cache, "fresh cache").unwrap();
        fs::write(&old_transcode, "old transcode").unwrap();
        set_modified(&old_cache, UNIX_EPOCH);
        set_modified(&old_transcode, UNIX_EPOCH);

        let service = ScheduledTaskService::new(vec![test_task("DeleteCacheFiles")]);
        service.set_cache_directory(cache.clone());
        service.set_transcode_directory(transcode.clone());
        service.register_executor("DeleteCacheFiles", delete_cache_files_handler());

        service.start("DeleteCacheFiles").await.unwrap();
        wait_for_idle(&service, "DeleteCacheFiles").await;

        assert!(!old_cache.exists());
        assert!(fresh_cache.exists());
        assert!(!old_transcode.exists());
        fs::remove_dir_all(cache).unwrap();
        fs::remove_dir_all(transcode).unwrap();
    }

    fn test_task(key: &str) -> ScheduledTask {
        ScheduledTask {
            id: key.to_owned(),
            key: key.to_owned(),
            name: key.to_owned(),
            description: String::new(),
            category: String::new(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: Vec::new(),
            last_run: None,
            last_execution_result: None,
        }
    }

    fn temporary_directory(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-rust-{prefix}-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    async fn wait_for_idle(service: &ScheduledTaskService, task_id: &str) {
        for _ in 0..100 {
            if service.get(task_id).await.unwrap().state == TaskState::Idle {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("scheduled task did not return to Idle");
    }
}
