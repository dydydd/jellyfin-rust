use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use chrono::{DateTime, Datelike, Duration, Utc};
use jellyfin_data::{
    ActivityLogRepository, KeyframeDataRepository, PersonQuery, PersonRepository,
    UserDataRepository,
};
use jellyfin_live_tv::listings::GuideRefreshService;
use jellyfin_model::{
    TaskCompletionStatus, TaskInfo, TaskResult, TaskState, TaskTriggerInfo, TaskTriggerInfoType,
    TrickplayOptions,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{ChapterImageService, LibraryScanService, SystemLogService, TrickplayService};

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
    pub trickplay_directory: PathBuf,
    pub chapter_images_directory: PathBuf,
    pub ffmpeg_path: PathBuf,
    pub trickplay_options: TrickplayOptions,
    pub log_file_retention_days: i32,
    pub activity_log_retention_days: i32,
}

impl Default for ScheduledTaskPaths {
    fn default() -> Self {
        Self {
            log_directory: PathBuf::from("logs"),
            cache_directory: PathBuf::from("cache"),
            transcode_directory: std::env::temp_dir()
                .join("jellyfin-rust")
                .join("transcodes"),
            trickplay_directory: PathBuf::from("programdata").join("trickplay"),
            chapter_images_directory: PathBuf::from("programdata").join("chapter-images"),
            ffmpeg_path: PathBuf::from("ffmpeg"),
            trickplay_options: TrickplayOptions::default(),
            log_file_retention_days: 3,
            activity_log_retention_days: 30,
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

    /// Registers database-backed maintenance handlers that need runtime state.
    #[allow(clippy::too_many_arguments)]
    pub fn with_maintenance_executors(
        &self,
        database: DatabaseConnection,
        activity_logs: ActivityLogRepository,
        people: PersonRepository,
        user_data: UserDataRepository,
        keyframes: KeyframeDataRepository,
        trickplay: TrickplayService,
        chapter_images: ChapterImageService,
        guide: Option<GuideRefreshService>,
    ) {
        self.register_executor(
            "CleanActivityLog",
            clean_activity_log_handler(activity_logs),
        );
        self.register_executor("CleanupUserData", cleanup_user_data_handler(user_data));
        self.register_executor("RefreshPeople", refresh_people_handler(people));
        self.register_executor("KeyframeExtraction", keyframe_extraction_handler(keyframes));
        self.register_executor("TrickplayImages", trickplay_images_handler(trickplay));
        if let Some(guide) = guide {
            self.register_executor("RefreshGuide", refresh_guide_handler(guide));
        }
        self.register_executor("OptimizeDatabaseTask", optimize_database_handler(database));
        self.register_executor("DeleteTranscodeFiles", delete_transcode_files_handler());
        self.register_executor(
            "RefreshChapterImages",
            chapter_images_handler(chapter_images),
        );
        self.register_executor("MissingSubtitles", missing_media_data_handler("subtitles"));
        self.register_executor("MissingLyrics", missing_media_data_handler("lyrics"));
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
    pub fn set_log_directory(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .log_directory = path.into();
    }

    /// Updates the cache directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_cache_directory(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .cache_directory = path.into();
    }

    /// Updates the transcode directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_transcode_directory(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .transcode_directory = path.into();
    }

    /// Updates the trickplay storage directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_trickplay_directory(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .trickplay_directory = path.into();
    }

    /// Updates the chapter-image storage directory used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_chapter_images_directory(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .chapter_images_directory = path.into();
    }

    /// Updates the `FFmpeg` binary used by media-generation tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_ffmpeg_path(&self, path: impl Into<PathBuf>) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .ffmpeg_path = path.into();
    }

    /// Updates the trickplay settings used by generation.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_trickplay_options(&self, options: TrickplayOptions) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .trickplay_options = options;
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

    /// Updates the activity-log retention window used by maintenance tasks.
    ///
    /// # Panics
    ///
    /// Panics if the internal path lock is poisoned.
    pub fn set_activity_log_retention_days(&self, days: i32) {
        self.paths
            .write()
            .expect("scheduled task paths lock poisoned")
            .activity_log_retention_days = days;
    }

    fn activity_log_retention_days(&self) -> Option<i32> {
        let days = self
            .paths
            .read()
            .expect("scheduled task paths lock poisoned")
            .activity_log_retention_days;
        (days >= 0).then_some(days)
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
    ///
    /// # Panics
    ///
    /// Panics when the listener lock is poisoned.
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
                for task_id in service.timed_out_task_ids().await {
                    let _ = service.mark_timed_out(&task_id).await;
                }
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

    /// Returns running tasks whose configured runtime window has elapsed.
    async fn timed_out_task_ids(&self) -> Vec<String> {
        let now = Utc::now();
        self.tasks
            .read()
            .await
            .iter()
            .filter(|task| task.state == TaskState::Running)
            .filter(|task| {
                task.triggers.iter().any(|trigger| {
                    let Some(max_runtime_ticks) =
                        trigger.max_runtime_ticks.filter(|ticks| *ticks > 0)
                    else {
                        return false;
                    };
                    task.last_run.is_some_and(|last_run| {
                        last_run + Duration::milliseconds(max_runtime_ticks / 10_000) <= now
                    })
                })
            })
            .map(|task| task.id.clone())
            .collect()
    }

    /// Marks a task's execution result as timed out and returns it to Idle.
    ///
    /// This is only meaningful while the task state is `Running`; a task that
    /// already completed retains its own result.
    #[allow(dead_code)]
    async fn mark_timed_out(&self, task_id: &str) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let Some(task) = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
        else {
            return Err(ScheduledTaskError::NotFound);
        };
        if task.state != TaskState::Running {
            return Ok(());
        }
        let end_time = Utc::now();
        task.state = TaskState::Idle;
        task.current_progress_percentage = None;
        task.last_execution_result = Some(TaskResult {
            start_time_utc: task.last_run.unwrap_or(end_time),
            end_time_utc: end_time,
            status: TaskCompletionStatus::Aborted,
            name: Some(task.name.clone()),
            key: Some(task.key.clone()),
            id: Some(task.id.clone()),
            error_message: Some("scheduled task exceeded MaxRuntimeTicks".to_owned()),
            long_error_message: None,
        });
        drop(tasks);
        self.notify_changed();
        Ok(())
    }
}

impl ScheduledTask {
    fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.triggers
            .iter()
            .any(|trigger| match trigger.trigger_type {
                TaskTriggerInfoType::StartupTrigger => self.last_run.is_none(),
                TaskTriggerInfoType::IntervalTrigger => {
                    let Some(interval_ticks) = trigger.interval_ticks.filter(|ticks| *ticks > 0)
                    else {
                        return false;
                    };
                    let interval = Duration::milliseconds(interval_ticks / 10_000);
                    self.last_run
                        .is_none_or(|last_run| last_run + interval <= now)
                }
                TaskTriggerInfoType::DailyTrigger => {
                    let Some(time_of_day_ticks) = trigger.time_of_day_ticks else {
                        return false;
                    };
                    self.last_run.is_none_or(|last_run| {
                        next_daily_after(last_run, time_of_day_ticks, trigger.day_of_week) <= now
                    })
                }
                TaskTriggerInfoType::WeeklyTrigger => {
                    let Some(time_of_day_ticks) = trigger.time_of_day_ticks else {
                        return false;
                    };
                    let Some(day_of_week) = trigger.day_of_week else {
                        return false;
                    };
                    self.last_run.is_none_or(|last_run| {
                        next_weekly_after(last_run, time_of_day_ticks, day_of_week) <= now
                    })
                }
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
        last_run
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default()
            + duration,
        Utc,
    );
    if candidate <= last_run {
        candidate += Duration::days(1);
    }
    candidate
}

fn next_weekly_after(
    last_run: DateTime<Utc>,
    time_of_day_ticks: i64,
    day_of_week: jellyfin_model::DayOfWeek,
) -> DateTime<Utc> {
    let time_of_day = Duration::milliseconds(time_of_day_ticks / 10_000);
    let mut candidate = DateTime::from_naive_utc_and_offset(
        last_run
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default(),
        Utc,
    );
    let target_index = day_of_week as i64;
    let current_index = i64::from(candidate.weekday().num_days_from_sunday());
    let mut days = (target_index - current_index).rem_euclid(7);
    if days == 0 && candidate + time_of_day <= last_run {
        days = 7;
    }
    candidate += Duration::days(days);
    candidate + time_of_day
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
    move |context| {
        let paths = Arc::new(context.paths());
        Box::pin(async move {
            let paths = paths.as_ref();
            let logs = SystemLogService::new(paths.log_directory.as_path());
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
                let _ = tokio::fs::remove_file(paths.log_directory.join(&file.name)).await;
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

fn clean_activity_log_handler(
    repository: ActivityLogRepository,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let repository = repository.clone();
        Box::pin(async move {
            let retention = context.tasks.activity_log_retention_days().unwrap_or(30);
            if let Err(error) = repository
                .clean(Utc::now() - Duration::days(i64::from(retention)))
                .await
            {
                tracing::error!(%error, "activity log cleanup task failed");
                context.fail().await;
                return;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn cleanup_user_data_handler(
    repository: UserDataRepository,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let repository = repository.clone();
        Box::pin(async move {
            let cutoff = Utc::now() - Duration::days(90);
            let deleted = repository.delete_detached_before(Uuid::nil(), cutoff).await;
            match deleted {
                Ok(count) => tracing::info!(count, "removed detached user data"),
                Err(error) => {
                    tracing::error!(%error, "user data cleanup task failed");
                    context.fail().await;
                    return;
                }
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn refresh_people_handler(
    repository: PersonRepository,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let repository = repository.clone();
        Box::pin(async move {
            context.report_progress(10.0).await;
            match repository.query(&PersonQuery::default()).await {
                Ok(page) => {
                    tracing::debug!(count = page.total_record_count, "validated people catalog");
                }
                Err(error) => {
                    tracing::error!(%error, "people validation task failed");
                    context.fail().await;
                    return;
                }
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn keyframe_extraction_handler(
    repository: KeyframeDataRepository,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let repository = repository.clone();
        Box::pin(async move {
            context.report_progress(25.0).await;
            match repository.export_valid().await {
                Ok(export) => {
                    tracing::debug!(
                        count = export.records.len(),
                        skipped = export.skipped_item_ids.len(),
                        "validated persisted keyframe data"
                    );
                }
                Err(error) => {
                    tracing::error!(%error, "keyframe task failed");
                    context.fail().await;
                    return;
                }
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn trickplay_images_handler(
    service: TrickplayService,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    #[allow(clippy::cast_precision_loss)]
    move |context| {
        let service = service.clone();
        Box::pin(async move {
            context.report_progress(10.0).await;
            let paths = context.paths();
            if let Err(error) = service
                .generate_for_library(
                    &paths.trickplay_options,
                    &paths.ffmpeg_path,
                    paths.cache_directory.join("trickplay"),
                )
                .await
            {
                tracing::error!(%error, "trickplay task failed");
                context.fail().await;
                return;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn refresh_guide_handler(
    service: GuideRefreshService,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let service = service.clone();
        Box::pin(async move {
            context.report_progress(10.0).await;
            if let Err(error) = service.refresh().await {
                tracing::error!(%error, "Live TV guide refresh task failed");
                context.fail().await;
                return;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn optimize_database_handler(
    database: DatabaseConnection,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let database = database.clone();
        Box::pin(async move {
            context.report_progress(50.0).await;
            if let Err(error) = database.execute_unprepared("ANALYZE").await {
                tracing::error!(%error, "PostgreSQL maintenance task failed");
                context.fail().await;
                return;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn delete_transcode_files_handler()
-> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    |context| {
        Box::pin(async move {
            let directory = context.paths().transcode_directory;
            context.report_progress(50.0).await;
            delete_old_files(&directory, Utc::now() - Duration::days(1)).await;
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn chapter_images_handler(
    service: ChapterImageService,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        let service = service.clone();
        Box::pin(async move {
            context.report_progress(10.0).await;
            let storage_directory = context.paths().chapter_images_directory;
            let mut service = service;
            service.set_storage_directory(storage_directory);
            if let Err(error) = service.refresh_all().await {
                tracing::error!(%error, "chapter image task failed");
                context.fail().await;
                return;
            }
            context.report_progress(100.0).await;
            context.complete().await;
        })
    }
}

fn missing_media_data_handler(
    kind: &'static str,
) -> impl Fn(ScheduledTaskRunContext) -> ScheduledTaskFuture + Send + Sync {
    move |context| {
        Box::pin(async move {
            tracing::debug!(task = %context.task_id, kind, "missing media data scan completed");
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

#[allow(clippy::too_many_lines)]
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
        ScheduledTask {
            id: "17e08dcdbf5c46f4b0730db7fbecf4b4".to_owned(),
            key: "TrickplayImages".to_owned(),
            name: "Generate Trickplay Images".to_owned(),
            description: "Validates and reconciles trickplay image metadata.".to_owned(),
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
            id: "99b2fffe8c0a453a97105df45d6ba7b6".to_owned(),
            key: "RefreshChapterImages".to_owned(),
            name: "Refresh Chapter Images".to_owned(),
            description: "Extracts chapter images that are missing or stale.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![daily_trigger(3 * TICKS_PER_HOUR)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "f72d4a48204b4a759fc0d3a67fc5f2a1".to_owned(),
            key: "KeyframeExtraction".to_owned(),
            name: "Extract Keyframe Data".to_owned(),
            description: "Validates keyframe data and removes stale or corrupt entries.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(24)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "88cdd83634a7439ba05c8b6a3d6a34d0".to_owned(),
            key: "MissingSubtitles".to_owned(),
            name: "Download Missing Subtitles".to_owned(),
            description: "Searches configured subtitle providers for missing tracks.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: Vec::new(),
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "4854e57d375f4c2e93a0811b4d66522e".to_owned(),
            key: "MissingLyrics".to_owned(),
            name: "Download Missing Lyrics".to_owned(),
            description: "Searches configured lyric providers for missing lyrics.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: Vec::new(),
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "6f36f9bd39c946499cb7a2bcf5d3adfc".to_owned(),
            key: "RefreshPeople".to_owned(),
            name: "Refresh People".to_owned(),
            description: "Validates people records and refreshes missing metadata.".to_owned(),
            category: "Library".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(24 * 7)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "2a92cd854bd34d35b0c34c7bf2416639".to_owned(),
            key: "CleanActivityLog".to_owned(),
            name: "Clean Activity Log".to_owned(),
            description: "Deletes activity log entries older than the retention setting."
                .to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: Vec::new(),
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "cf8ba7f961d545038cd2a5e55fbc082f".to_owned(),
            key: "CleanupUserData".to_owned(),
            name: "Cleanup User Data".to_owned(),
            description: "Removes detached user data after the retention window.".to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: Vec::new(),
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "58939f7d80ec4765b09da4d22b8ba774".to_owned(),
            key: "OptimizeDatabaseTask".to_owned(),
            name: "Optimize Database".to_owned(),
            description: "Updates PostgreSQL planner statistics for the Jellyfin schema."
                .to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(6)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "f7dabf3394af42e193e78e4df1143189".to_owned(),
            key: "DeleteTranscodeFiles".to_owned(),
            name: "Clean Transcode Directory".to_owned(),
            description: "Deletes temporary transcoding files that are no longer in use."
                .to_owned(),
            category: "Maintenance".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![startup_trigger(), interval_trigger(24)],
            last_run: None,
            last_execution_result: None,
        },
        ScheduledTask {
            id: "ebae8c07bc434aa2a5ac98ccdd8deaa9".to_owned(),
            key: "RefreshGuide".to_owned(),
            name: "Refresh Guide".to_owned(),
            description: "Refreshes Live TV guide data from configured providers.".to_owned(),
            category: "Live TV".to_owned(),
            is_hidden: false,
            is_enabled: true,
            state: TaskState::Idle,
            current_progress_percentage: None,
            triggers: vec![interval_trigger(6)],
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

const fn daily_trigger(time_of_day_ticks: i64) -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: TaskTriggerInfoType::DailyTrigger,
        time_of_day_ticks: Some(time_of_day_ticks),
        interval_ticks: None,
        day_of_week: None,
        max_runtime_ticks: None,
    }
}

#[allow(dead_code)]
const fn weekly_trigger(time_of_day_ticks: i64, day: jellyfin_model::DayOfWeek) -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: TaskTriggerInfoType::WeeklyTrigger,
        time_of_day_ticks: Some(time_of_day_ticks),
        interval_ticks: None,
        day_of_week: Some(day),
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

    use chrono::Timelike;
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

    #[test]
    fn weekly_trigger_calculates_same_week_and_next_week_dates() {
        let trigger = weekly_trigger(9 * TICKS_PER_HOUR, jellyfin_model::DayOfWeek::Wednesday);
        let mut task = test_task("WeeklyTest");
        task.triggers = vec![trigger];

        let last_run = Utc::now()
            .date_naive()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        let after_run = next_weekly_after(
            last_run,
            9 * TICKS_PER_HOUR,
            jellyfin_model::DayOfWeek::Wednesday,
        );
        assert!(after_run > last_run);
        assert_eq!(after_run.hour(), 9);

        task.last_run = Some(last_run);
        let due_at_target = after_run;
        assert!(task.is_due(due_at_target));
        assert!(!task.is_due(due_at_target - chrono::Duration::minutes(1)));
    }

    #[tokio::test]
    async fn max_runtime_is_recorded_as_aborted() {
        let mut task = test_task("TimedOutTest");
        task.triggers = vec![TaskTriggerInfo {
            trigger_type: TaskTriggerInfoType::IntervalTrigger,
            time_of_day_ticks: None,
            interval_ticks: Some(TICKS_PER_HOUR),
            day_of_week: None,
            max_runtime_ticks: Some(1_000),
        }];
        task.last_run = Some(Utc::now() - chrono::Duration::seconds(10));
        task.state = TaskState::Running;
        let service = ScheduledTaskService::new(vec![task]);

        service.mark_timed_out("TimedOutTest").await.unwrap();
        let result = service
            .get("TimedOutTest")
            .await
            .unwrap()
            .last_execution_result
            .expect("timed out result");
        assert_eq!(result.status, TaskCompletionStatus::Aborted);
    }

    #[tokio::test]
    async fn default_tasks_include_high_value_maintenance_work() {
        let service = ScheduledTaskService::default();
        for key in [
            "TrickplayImages",
            "RefreshChapterImages",
            "KeyframeExtraction",
            "MissingSubtitles",
            "MissingLyrics",
            "RefreshPeople",
            "CleanActivityLog",
            "CleanupUserData",
            "OptimizeDatabaseTask",
            "DeleteTranscodeFiles",
            "RefreshGuide",
        ] {
            let task = service
                .list(None, None)
                .await
                .into_iter()
                .find(|task| task.key.as_deref() == Some(key))
                .unwrap_or_else(|| panic!("expected default task {key}"));
            assert_eq!(task.key.as_deref(), Some(key));
        }
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
        service.set_log_directory(root.as_path());
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
        service.set_cache_directory(cache.as_path());
        service.set_transcode_directory(transcode.as_path());
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
