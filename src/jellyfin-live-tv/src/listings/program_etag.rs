use std::error::Error;
use std::fmt;

use chrono::{DateTime, Timelike, Utc};
use jellyfin_model::ProviderIdMap;
use sha2::{Digest, Sha256};

use super::{ProgramFlag, ProgramInfo};

/// Prefix identifying `ETags` generated from normalized XMLTV programme content.
pub const XMLTV_ETAG_PREFIX: &str = "xmltv-sha256-v1:";

/// A required programme identity field is unsuitable for `ETag` generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramEtagError {
    EmptyProgramId,
    EmptyChannelId,
    EmptyStartDate,
    EmptyEndDate,
    EndDateNotAfterStartDate,
}

impl fmt::Display for ProgramEtagError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyProgramId => "program id is empty",
            Self::EmptyChannelId => "channel id is empty",
            Self::EmptyStartDate => "start date is empty",
            Self::EmptyEndDate => "end date is empty",
            Self::EndDateNotAfterStartDate => "end date is not after start date",
        })
    }
}

impl Error for ProgramEtagError {}

#[must_use]
pub fn is_xmltv_etag(etag: Option<&str>) -> bool {
    etag.is_some_and(|etag| !etag.trim().is_empty() && etag.starts_with(XMLTV_ETAG_PREFIX))
}

/// Returns true only for equal XMLTV `ETags`, preserving other providers' update paths.
#[must_use]
pub fn xmltv_etag_matches_stored(incoming: Option<&str>, stored: Option<&str>) -> bool {
    is_xmltv_etag(incoming)
        && incoming
            .zip(stored)
            .is_some_and(|(incoming, stored)| incoming.eq_ignore_ascii_case(stored))
}

/// Creates the same versioned content hash used by Jellyfin's XMLTV provider.
///
/// # Errors
///
/// Returns [`ProgramEtagError`] when stable programme identity or a valid time
/// range is missing.
pub fn create_xmltv_program_etag(program: &ProgramInfo) -> Result<String, ProgramEtagError> {
    let id = required(program.id.as_deref(), ProgramEtagError::EmptyProgramId)?;
    let channel_id = required(
        program.channel_id.as_deref(),
        ProgramEtagError::EmptyChannelId,
    )?;
    let start_date = program
        .start_date
        .as_ref()
        .ok_or(ProgramEtagError::EmptyStartDate)?;
    let end_date = program
        .end_date
        .as_ref()
        .ok_or(ProgramEtagError::EmptyEndDate)?;
    if end_date <= start_date {
        return Err(ProgramEtagError::EndDateNotAfterStartDate);
    }

    let mut content = String::with_capacity(1024);
    append_core_fields(&mut content, program, id, channel_id, start_date, end_date);
    append_image_fields(&mut content, program);
    append_classification_fields(&mut content, program);
    append_identity_fields(&mut content, program);

    let digest = Sha256::digest(content.as_bytes());
    Ok(format!("{XMLTV_ETAG_PREFIX}{digest:X}"))
}

fn append_core_fields(
    content: &mut String,
    program: &ProgramInfo,
    id: &str,
    channel_id: &str,
    start_date: &DateTime<Utc>,
    end_date: &DateTime<Utc>,
) {
    append_value(content, "schema", Some("xmltv-programinfo-v1"));
    append_value(content, "Id", Some(id));
    append_value(content, "ChannelId", Some(channel_id));
    append_value(content, "Name", program.name.as_deref());
    append_value(
        content,
        "OfficialRating",
        program.official_rating.as_deref(),
    );
    append_value(content, "Overview", program.overview.as_deref());
    append_date(content, "StartDate", Some(start_date));
    append_date(content, "EndDate", Some(end_date));
    append_list(content, "Genres", &program.genres);
    append_date(
        content,
        "OriginalAirDate",
        program.original_air_date.as_ref(),
    );
    append_optional_bool(content, "IsHD", program.is_hd);
    let audio = program.audio.map(|value| value.to_string());
    append_value(content, "Audio", audio.as_deref());
    let community_rating = program.community_rating.map(format_f32);
    append_value(content, "CommunityRating", community_rating.as_deref());
    append_bool(
        content,
        "IsRepeat",
        program.flags.contains(ProgramFlag::Repeat),
    );
    append_value(content, "EpisodeTitle", program.episode_title.as_deref());
}

fn append_image_fields(content: &mut String, program: &ProgramInfo) {
    append_value(content, "ImagePath", program.image_path.as_deref());
    append_value(content, "ImageUrl", program.image_url.as_deref());
    append_value(content, "ThumbImageUrl", program.thumb_image_url.as_deref());
    append_value(content, "LogoImageUrl", program.logo_image_url.as_deref());
    append_value(
        content,
        "BackdropImageUrl",
        program.backdrop_image_url.as_deref(),
    );
}

fn append_classification_fields(content: &mut String, program: &ProgramInfo) {
    for (name, flag) in [
        ("IsMovie", ProgramFlag::Movie),
        ("IsSports", ProgramFlag::Sports),
        ("IsSeries", ProgramFlag::Series),
        ("IsLive", ProgramFlag::Live),
        ("IsNews", ProgramFlag::News),
        ("IsKids", ProgramFlag::Kids),
        ("IsPremiere", ProgramFlag::Premiere),
    ] {
        append_bool(content, name, program.flags.contains(flag));
    }
}

fn append_identity_fields(content: &mut String, program: &ProgramInfo) {
    append_optional_i32(content, "ProductionYear", program.production_year);
    append_value(content, "SeriesId", program.series_id.as_deref());
    append_value(content, "ShowId", program.show_id.as_deref());
    append_optional_i32(content, "SeasonNumber", program.season_number);
    append_optional_i32(content, "EpisodeNumber", program.episode_number);
    append_dictionary(content, "ProviderIds", &program.provider_ids);
    append_dictionary(content, "SeriesProviderIds", &program.series_provider_ids);
}

fn required(value: Option<&str>, error: ProgramEtagError) -> Result<&str, ProgramEtagError> {
    value.filter(|value| !value.trim().is_empty()).ok_or(error)
}

fn append_value(builder: &mut String, name: &str, value: Option<&str>) {
    builder.push_str(name);
    builder.push('|');
    match value {
        Some(value) => {
            builder.push_str("S|");
            builder.push_str(&value.encode_utf16().count().to_string());
            builder.push('|');
            builder.push_str(value);
        }
        None => builder.push_str("N|0|"),
    }
    builder.push('\n');
}

fn append_date(builder: &mut String, name: &str, value: Option<&DateTime<Utc>>) {
    let formatted = value.map(format_datetime);
    append_value(builder, name, formatted.as_deref());
}

fn format_datetime(value: &DateTime<Utc>) -> String {
    let fraction = value.nanosecond() / 100;
    format!("{}.{fraction:07}Z", value.format("%Y-%m-%dT%H:%M:%S"))
}

fn append_bool(builder: &mut String, name: &str, value: bool) {
    append_value(builder, name, Some(if value { "true" } else { "false" }));
}

fn append_optional_bool(builder: &mut String, name: &str, value: Option<bool>) {
    append_value(
        builder,
        name,
        value.map(|value| if value { "true" } else { "false" }),
    );
}

fn append_optional_i32(builder: &mut String, name: &str, value: Option<i32>) {
    let formatted = value.map(|value| value.to_string());
    append_value(builder, name, formatted.as_deref());
}

fn append_list(builder: &mut String, name: &str, values: &[String]) {
    append_value(
        builder,
        &format!("{name}.Count"),
        Some(&values.len().to_string()),
    );
    for (index, value) in values.iter().enumerate() {
        append_value(builder, &format!("{name}[{index}]"), Some(value));
    }
}

fn append_dictionary(builder: &mut String, name: &str, values: &ProviderIdMap) {
    append_value(
        builder,
        &format!("{name}.Count"),
        Some(&values.len().to_string()),
    );
    let mut entries: Vec<_> = values.iter().collect();
    entries.sort_by(|(left, _), (right, _)| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    for (index, (key, value)) in entries.into_iter().enumerate() {
        append_value(builder, &format!("{name}[{index}].Key"), Some(key));
        append_value(builder, &format!("{name}[{index}].Value"), Some(value));
    }
}

fn format_f32(value: f32) -> String {
    value.to_string()
}
