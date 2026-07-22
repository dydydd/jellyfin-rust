use chrono::{TimeZone, Utc};
use jellyfin_controller::library::{UserItemData, UserItemDataDto, VersionResumeData};
use uuid::Uuid;

#[test]
fn completed_other_version_propagates_completion_and_clears_stale_resume() {
    let last_played = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    let mut user_data = UserItemData::new("version");
    user_data.played = true;
    user_data.last_played_date = Some(last_played);
    let resume = VersionResumeData::new(Uuid::new_v4(), user_data);

    let mut dto = UserItemDataDto::new(Uuid::new_v4(), "primary");
    dto.playback_position_ticks = 1;
    dto.played_percentage = Some(50.0);

    resume.apply_to(&mut dto);

    assert!(dto.played);
    assert_eq!(dto.last_played_date, Some(last_played));
    assert_eq!(dto.playback_position_ticks, 0);
    assert_eq!(dto.played_percentage, None);
}

#[test]
fn primary_own_progress_keeps_resume_position() {
    let primary_id = Uuid::new_v4();
    let last_played = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    let mut user_data = UserItemData::new("primary");
    user_data.playback_position_ticks = 5;
    user_data.played = true;
    user_data.last_played_date = Some(last_played);
    let resume = VersionResumeData::new(primary_id, user_data);

    let mut dto = UserItemDataDto::new(primary_id, "primary");
    dto.playback_position_ticks = 5;
    dto.played = true;
    dto.played_percentage = Some(20.0);

    resume.apply_to(&mut dto);

    assert!(dto.played);
    assert_eq!(dto.playback_position_ticks, 5);
    assert_eq!(dto.played_percentage, Some(20.0));
}

#[test]
fn in_progress_other_version_keeps_primary_resume_position() {
    let last_played = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    let mut user_data = UserItemData::new("version");
    user_data.playback_position_ticks = 25;
    user_data.last_played_date = Some(last_played);
    let resume = VersionResumeData::new(Uuid::new_v4(), user_data);

    let mut dto = UserItemDataDto::new(Uuid::new_v4(), "primary");
    dto.playback_position_ticks = 1;
    dto.played_percentage = Some(50.0);

    resume.apply_to(&mut dto);

    assert!(!dto.played);
    assert_eq!(dto.playback_position_ticks, 1);
    assert_eq!(dto.played_percentage, Some(50.0));
}

#[test]
fn apply_does_not_unset_existing_played_or_regress_last_played() {
    let primary_last_played = Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap();
    let version_last_played = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();
    let mut user_data = UserItemData::new("version");
    user_data.last_played_date = Some(version_last_played);
    let resume = VersionResumeData::new(Uuid::new_v4(), user_data);

    let mut dto = UserItemDataDto::new(Uuid::new_v4(), "primary");
    dto.played = true;
    dto.last_played_date = Some(primary_last_played);

    resume.apply_to(&mut dto);

    assert!(dto.played);
    assert_eq!(dto.last_played_date, Some(primary_last_played));
}
