use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum TaskState {
    #[default]
    Idle,
    Cancelling,
    Running,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum TaskCompletionStatus {
    #[default]
    Completed,
    Failed,
    Cancelled,
    Aborted,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum TaskTriggerInfoType {
    #[default]
    DailyTrigger,
    WeeklyTrigger,
    IntervalTrigger,
    StartupTrigger,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum DayOfWeek {
    #[default]
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    #[serde(rename = "Type")]
    pub trigger_type: TaskTriggerInfoType,
    pub time_of_day_ticks: Option<i64>,
    pub interval_ticks: Option<i64>,
    pub day_of_week: Option<DayOfWeek>,
    pub max_runtime_ticks: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskResult {
    pub start_time_utc: DateTime<Utc>,
    pub end_time_utc: DateTime<Utc>,
    pub status: TaskCompletionStatus,
    pub name: Option<String>,
    pub key: Option<String>,
    pub id: Option<String>,
    pub error_message: Option<String>,
    pub long_error_message: Option<String>,
}

impl Default for TaskResult {
    fn default() -> Self {
        Self {
            start_time_utc: DateTime::<Utc>::UNIX_EPOCH,
            end_time_utc: DateTime::<Utc>::UNIX_EPOCH,
            status: TaskCompletionStatus::default(),
            name: None,
            key: None,
            id: None,
            error_message: None,
            long_error_message: None,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "PascalCase")]
pub struct TaskInfo {
    pub name: Option<String>,
    pub state: TaskState,
    pub current_progress_percentage: Option<f64>,
    pub id: Option<String>,
    pub last_execution_result: Option<TaskResult>,
    pub triggers: Vec<TaskTriggerInfo>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub is_hidden: bool,
    pub key: Option<String>,
}
