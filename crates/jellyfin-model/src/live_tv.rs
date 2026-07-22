use chrono::{DateTime, NaiveDate, Utc};

/// Internal timer data shared by Live TV recording services.
///
/// This is the subset of Jellyfin's controller `TimerInfo` contract used to
/// derive recording names. Additional timer fields can be added here as their
/// owning Live TV features are ported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerInfo {
    pub name: String,
    pub start_date: DateTime<Utc>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub is_movie: bool,
    pub production_year: Option<i32>,
    pub episode_title: Option<String>,
    pub original_air_date: Option<DateTime<Utc>>,
    pub is_program_series: bool,
}

/// Configuration and discovered identity for a Live TV tuner host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TunerHostInfo {
    pub id: Option<String>,
    pub url: String,
    pub tuner_type: String,
    pub device_id: Option<String>,
    pub friendly_name: Option<String>,
    pub import_favorites_only: bool,
    pub tuner_count: i32,
}

impl Default for TimerInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            start_date: dotnet_min_utc(),
            season_number: None,
            episode_number: None,
            is_movie: false,
            production_year: None,
            episode_title: None,
            original_air_date: None,
            is_program_series: false,
        }
    }
}

fn dotnet_min_utc() -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(1, 1, 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| DateTime::from_naive_utc_and_offset(date, Utc))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}
