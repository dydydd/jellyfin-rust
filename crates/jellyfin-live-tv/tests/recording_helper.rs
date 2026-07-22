use chrono::{FixedOffset, TimeZone, Utc};
use jellyfin_live_tv::recordings::get_recording_name;
use jellyfin_model::TimerInfo;

fn utc(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .expect("official test date should be valid")
}

#[test]
fn official_recording_names_match() {
    let cases = [
        (
            "The Incredibles 2020_04_20_21_06_00",
            TimerInfo {
                name: "The Incredibles".into(),
                start_date: utc(2020, 4, 20, 21, 6, 0),
                is_movie: true,
                ..TimerInfo::default()
            },
        ),
        (
            "The Incredibles (2004)",
            TimerInfo {
                name: "The Incredibles".into(),
                is_movie: true,
                production_year: Some(2004),
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory 2020_04_20_21_06_00",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                start_date: utc(2020, 4, 20, 21, 6, 0),
                is_program_series: true,
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory S12E10",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                is_program_series: true,
                season_number: Some(12),
                episode_number: Some(10),
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory S12E10 The VCR Illumination",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                is_program_series: true,
                season_number: Some(12),
                episode_number: Some(10),
                episode_title: Some("The VCR Illumination".into()),
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory 2018-12-06",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                is_program_series: true,
                original_air_date: Some(utc(2018, 12, 6, 0, 0, 0)),
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory 2018-12-06 - The VCR Illumination",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                is_program_series: true,
                original_air_date: Some(utc(2018, 12, 6, 0, 0, 0)),
                episode_title: Some("The VCR Illumination".into()),
                ..TimerInfo::default()
            },
        ),
        (
            "The Big Bang Theory 2018_12_06_21_06_00 - The VCR Illumination",
            TimerInfo {
                name: "The Big Bang Theory".into(),
                start_date: utc(2018, 12, 6, 21, 6, 0),
                is_program_series: true,
                original_air_date: Some(utc(2018, 12, 6, 0, 0, 0)),
                episode_title: Some("The VCR Illumination".into()),
                ..TimerInfo::default()
            },
        ),
        (
            "Lorem ipsum dolor sit amet: consect 2018_12_06_21_06_00",
            TimerInfo {
                name: "Lorem ipsum dolor sit amet: consect".into(),
                is_program_series: true,
                start_date: utc(2018, 12, 6, 21, 6, 0),
                original_air_date: Some(utc(2018, 12, 6, 0, 0, 0)),
                episode_title: Some("Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor".into()),
                ..TimerInfo::default()
            },
        ),
    ];

    for (expected, timer) in cases {
        assert_eq!(expected, get_recording_name(&timer, &Utc));
    }
}

#[test]
fn dates_use_the_explicit_local_timezone() {
    let timer = TimerInfo {
        name: "Evening News".into(),
        start_date: utc(2020, 4, 20, 13, 6, 0),
        ..TimerInfo::default()
    };
    let china_standard_time = FixedOffset::east_opt(8 * 60 * 60).expect("valid fixed offset");

    assert_eq!(
        "Evening News 2020_04_20_21_06_00",
        get_recording_name(&timer, &china_standard_time)
    );
}

#[test]
fn logical_name_preserves_characters_for_the_filesystem_layer_to_sanitize() {
    let timer = TimerInfo {
        name: "News: Morning/Evening".into(),
        start_date: utc(2020, 4, 20, 21, 6, 0),
        ..TimerInfo::default()
    };

    assert_eq!(
        "News: Morning/Evening 2020_04_20_21_06_00",
        get_recording_name(&timer, &Utc)
    );
}

#[test]
fn episode_title_limit_uses_utf8_bytes_and_is_strictly_less_than_250() {
    let base = TimerInfo {
        name: "Show".into(),
        is_program_series: true,
        season_number: Some(1),
        episode_number: Some(1),
        ..TimerInfo::default()
    };

    let mut accepted = base.clone();
    accepted.episode_title = Some("a".repeat(237));
    assert_eq!(249, get_recording_name(&accepted, &Utc).len());

    let mut rejected_at_250 = base.clone();
    rejected_at_250.episode_title = Some("a".repeat(238));
    assert_eq!("Show S01E01", get_recording_name(&rejected_at_250, &Utc));

    let mut rejected_multibyte = base;
    rejected_multibyte.episode_title = Some("界".repeat(80));
    assert_eq!("Show S01E01", get_recording_name(&rejected_multibyte, &Utc));
}
