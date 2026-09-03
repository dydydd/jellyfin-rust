use chrono::{TimeZone, Utc};
use jellyfin_live_tv::listings::{
    ProgramEtagError, ProgramFlag, ProgramInfo, XMLTV_ETAG_PREFIX, XmlTvOptions,
    create_xmltv_program_etag, is_xmltv_etag, parse_xmltv_programs, xmltv_etag_matches_stored,
};

const EMPTY_CATEGORY: &str = include_str!("fixtures/emptycategory.xml");
const NO_TITLE: &str = include_str!("fixtures/notitle.xml");
const ETAG_BASE: &str = include_str!("fixtures/etag-base.xml");
const ETAG_TITLE_CHANGE: &str = include_str!("fixtures/etag-title-change.xml");
const ETAG_DESCRIPTION_CHANGE: &str = include_str!("fixtures/etag-description-change.xml");
const ETAG_ICON_CHANGE: &str = include_str!("fixtures/etag-icon-change.xml");
const ETAG_CATEGORY_CHANGE: &str = include_str!("fixtures/etag-category-change.xml");
const ETAG_PROGID_CHANGE: &str = include_str!("fixtures/etag-progid-change.xml");
const ETAG_REORDERED: &str = include_str!("fixtures/etag-reordered.xml");
const ETAG_UNKNOWN_FIELD: &str = include_str!("fixtures/etag-unknown-field.xml");

fn window() -> (chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    (
        Utc.with_ymd_and_hms(2022, 11, 4, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2022, 11, 5, 0, 0, 0).unwrap(),
    )
}

fn options() -> XmlTvOptions {
    XmlTvOptions {
        preferred_language: Some("en".to_owned()),
        sports_categories: vec!["sports".to_owned()],
        ..XmlTvOptions::default()
    }
}

fn single(xml: &str) -> ProgramInfo {
    let (start, end) = window();
    parse_xmltv_programs(xml, "3297", start, end, &options())
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

#[test]
fn no_title_fixture_maps_without_synthetic_text() {
    let program = single(NO_TITLE);
    assert_eq!(program.name, None);
    assert_eq!(program.series_id, None);
    assert_eq!(program.episode_title, None);
    assert!(program.flags.contains(ProgramFlag::Sports));
    assert_eq!(program.has_image, Some(true));
    assert_eq!(
        program.image_url.as_deref(),
        Some("https://domain.tld/image.png")
    );
    assert_eq!(program.channel_id.as_deref(), Some("3297"));
    assert!(
        program
            .etag
            .as_deref()
            .unwrap()
            .starts_with(XMLTV_ETAG_PREFIX)
    );
}

#[test]
fn empty_categories_are_ignored() {
    let program = single(EMPTY_CATEGORY);
    assert_eq!(program.genres, ["sports"]);
    assert!(program.genres.iter().all(|genre| !genre.is_empty()));
    assert!(program.etag.is_some());
}

#[test]
fn mapped_base_fixture_contains_episode_and_rating_data() {
    let program = single(ETAG_BASE);
    assert_eq!(program.name.as_deref(), Some("Base Program"));
    assert_eq!(program.episode_title.as_deref(), Some("Base Episode"));
    assert_eq!(program.season_number, Some(1));
    assert_eq!(program.episode_number, Some(2));
    assert_eq!(program.show_id.as_deref(), Some("EP123456789012"));
    assert_eq!(program.official_rating.as_deref(), Some("TV-G"));
    assert_eq!(program.community_rating, None);
    assert!(program.flags.contains(ProgramFlag::Series));
}

#[test]
fn etag_is_stable_and_changes_with_mapped_content() {
    let base = single(ETAG_BASE).etag.unwrap();
    assert_eq!(base, single(ETAG_BASE).etag.unwrap());
    for changed in [
        ETAG_TITLE_CHANGE,
        ETAG_DESCRIPTION_CHANGE,
        ETAG_ICON_CHANGE,
        ETAG_CATEGORY_CHANGE,
        ETAG_PROGID_CHANGE,
    ] {
        assert_ne!(base, single(changed).etag.unwrap());
    }
}

#[test]
fn etag_ignores_xml_order_and_unmapped_fields() {
    let base = single(ETAG_BASE).etag.unwrap();
    assert_eq!(base, single(ETAG_REORDERED).etag.unwrap());
    assert_eq!(base, single(ETAG_UNKNOWN_FIELD).etag.unwrap());
}

#[test]
fn etag_genre_order_is_significant() {
    let (start, end) = window();
    let mut first = ProgramInfo {
        id: Some("program-id".to_owned()),
        channel_id: Some("channel-id".to_owned()),
        name: Some("Program Name".to_owned()),
        start_date: Some(start),
        end_date: Some(end),
        genres: vec!["Drama".to_owned(), "Action".to_owned()],
        ..ProgramInfo::default()
    };
    let first_etag = create_xmltv_program_etag(&first).unwrap();
    first.genres.reverse();
    assert_ne!(first_etag, create_xmltv_program_etag(&first).unwrap());
}

#[test]
fn matching_only_accepts_xmltv_etags() {
    let etag = format!("{XMLTV_ETAG_PREFIX}ABCDEF0123456789");
    assert!(is_xmltv_etag(Some(&etag)));
    assert!(xmltv_etag_matches_stored(
        Some(&etag),
        Some(&etag.to_ascii_lowercase())
    ));
    assert!(!xmltv_etag_matches_stored(
        Some(&format!("{XMLTV_ETAG_PREFIX}AAAA")),
        Some(&format!("{XMLTV_ETAG_PREFIX}BBBB"))
    ));
    assert!(!xmltv_etag_matches_stored(
        Some("sd-abc123"),
        Some("sd-abc123")
    ));
}

#[test]
fn etag_requires_stable_identity_and_time_range() {
    let program = ProgramInfo::default();
    assert_eq!(
        create_xmltv_program_etag(&program),
        Err(ProgramEtagError::EmptyProgramId)
    );

    let (start, _) = window();
    let invalid = ProgramInfo {
        id: Some("id".to_owned()),
        channel_id: Some("channel".to_owned()),
        start_date: Some(start),
        end_date: Some(start),
        ..ProgramInfo::default()
    };
    assert_eq!(
        create_xmltv_program_etag(&invalid),
        Err(ProgramEtagError::EndDateNotAfterStartDate)
    );
}

#[test]
fn parser_filters_other_channels_and_outside_programmes() {
    let (start, end) = window();
    assert!(
        parse_xmltv_programs(ETAG_BASE, "other", start, end, &options())
            .unwrap()
            .is_empty()
    );
    let later_start = Utc.with_ymd_and_hms(2022, 11, 6, 0, 0, 0).unwrap();
    let later_end = Utc.with_ymd_and_hms(2022, 11, 7, 0, 0, 0).unwrap();
    assert!(
        parse_xmltv_programs(ETAG_BASE, "3297", later_start, later_end, &options())
            .unwrap()
            .is_empty()
    );
}
