use std::sync::Arc;

use jellyfin_model::{TaskInfo, TaskState, TaskTriggerInfo, TaskTriggerInfoType};
use thiserror::Error;
use tokio::sync::RwLock;

const TICKS_PER_HOUR: i64 = 36_000_000_000;

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
    pub triggers: Vec<TaskTriggerInfo>,
}

impl ScheduledTask {
    fn to_info(&self) -> TaskInfo {
        TaskInfo {
            name: Some(self.name.clone()),
            state: self.state,
            current_progress_percentage: None,
            id: Some(self.id.clone()),
            last_execution_result: None,
            triggers: self.triggers.clone(),
            description: Some(self.description.clone()),
            category: Some(self.category.clone()),
            is_hidden: self.is_hidden,
            key: Some(self.key.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScheduledTaskService {
    tasks: Arc<RwLock<Vec<ScheduledTask>>>,
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
        }
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

    pub async fn get(&self, task_id: &str) -> Result<TaskInfo, ScheduledTaskError> {
        self.tasks
            .read()
            .await
            .iter()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .map(ScheduledTask::to_info)
            .ok_or(ScheduledTaskError::NotFound)
    }

    pub async fn start(&self, task_id: &str) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        task.state = TaskState::Running;
        Ok(())
    }

    pub async fn stop(&self, task_id: &str) -> Result<(), ScheduledTaskError> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .iter_mut()
            .find(|task| task.id.eq_ignore_ascii_case(task_id))
            .ok_or(ScheduledTaskError::NotFound)?;
        task.state = TaskState::Idle;
        Ok(())
    }

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
        Ok(())
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
            triggers: vec![interval_trigger(12)],
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
            triggers: vec![interval_trigger(24)],
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
            triggers: vec![interval_trigger(24)],
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
            triggers: vec![startup_trigger(), interval_trigger(24)],
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
