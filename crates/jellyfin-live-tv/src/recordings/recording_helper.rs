use std::fmt::Display;

use chrono::{DateTime, TimeZone, Utc};
use jellyfin_model::TimerInfo;

const MAX_RECORDING_FILE_NAME_BYTES: usize = 250;

/// Builds the logical recording name used before filesystem sanitization.
///
/// Jellyfin formats timer dates in the server's local timezone. Taking that
/// timezone explicitly keeps this operation deterministic for callers and
/// tests while still allowing the server to pass `chrono::Local`.
#[must_use]
pub fn get_recording_name<Tz>(info: &TimerInfo, local_timezone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    let mut name = info.name.clone();

    if info.is_program_series {
        let mut add_hyphen = true;

        if let (Some(season), Some(episode)) = (info.season_number, info.episode_number) {
            name.push_str(&format!(" S{season:02}E{episode:02}"));
            add_hyphen = false;
        } else if let Some(original_air_date) = info.original_air_date {
            name.push(' ');
            if original_air_date.date_naive() == info.start_date.date_naive() {
                name.push_str(&format_date_time(info.start_date, local_timezone));
            } else {
                name.push_str(
                    &original_air_date
                        .with_timezone(local_timezone)
                        .format("%Y-%m-%d")
                        .to_string(),
                );
            }
        } else {
            name.push(' ');
            name.push_str(&format_date_time(info.start_date, local_timezone));
        }

        if let Some(episode_title) = info
            .episode_title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
        {
            let mut candidate = name.clone();
            if add_hyphen {
                candidate.push_str(" -");
            }

            candidate.push(' ');
            candidate.push_str(episode_title);
            if candidate.len() < MAX_RECORDING_FILE_NAME_BYTES {
                name = candidate;
            }
        }
    } else if let Some(production_year) = info.production_year.filter(|_| info.is_movie) {
        name.push_str(&format!(" ({production_year})"));
    } else {
        name.push(' ');
        name.push_str(&format_date_time(info.start_date, local_timezone));
    }

    name
}

fn format_date_time<Tz>(date: DateTime<Utc>, local_timezone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    date.with_timezone(local_timezone)
        .format("%Y_%m_%d_%H_%M_%S")
        .to_string()
}
