use std::cell::Cell;

use jellyfin_model::ProviderIdMap;
use jellyfin_providers::manager::metadata_service::{
    MediaUrl, MetadataField, MetadataItem, MetadataResult, MetadataService,
    MetadataServiceCapability, PersonInfo, Video3dFormat,
};

#[derive(Default)]
struct FixtureCapability {
    person_key_calls: Cell<usize>,
}

impl MetadataServiceCapability for FixtureCapability {
    fn person_key(&self, name: &str) -> String {
        self.person_key_calls.set(self.person_key_calls.get() + 1);
        name.to_lowercase()
    }
}

fn merge(
    source: &MetadataResult,
    mut target: MetadataResult,
    locked_fields: &[MetadataField],
    replace_data: bool,
    merge_metadata_settings: bool,
) -> (MetadataResult, FixtureCapability) {
    let capability = FixtureCapability::default();
    MetadataService::merge_base_item_data(
        source,
        &mut target,
        locked_fields,
        replace_data,
        merge_metadata_settings,
        &capability,
    );
    (target, capability)
}

#[test]
fn merge_base_item_data_merge_metadata_settings_merges_when_set() {
    for (merge_metadata_settings, default_date) in [(false, false), (true, false), (true, true)] {
        let source = MetadataResult {
            item: MetadataItem {
                locked_fields: vec![MetadataField::Genres, MetadataField::Cast],
                is_locked: true,
                preferred_metadata_country_code: Some("new".to_owned()),
                preferred_metadata_language: Some("new".to_owned()),
                date_created: if default_date { 0 } else { 2 },
                ..MetadataItem::default()
            },
            people: None,
        };
        let target = MetadataResult {
            item: MetadataItem {
                locked_fields: vec![MetadataField::Genres],
                is_locked: false,
                preferred_metadata_country_code: Some("old".to_owned()),
                preferred_metadata_language: Some("old".to_owned()),
                date_created: 1,
                ..MetadataItem::default()
            },
            people: None,
        };
        let (actual, _) = merge(&source, target, &[], true, merge_metadata_settings);
        if merge_metadata_settings {
            assert_eq!(
                actual.item.locked_fields,
                vec![MetadataField::Genres, MetadataField::Cast]
            );
            assert!(actual.item.is_locked);
            assert_eq!(
                actual.item.preferred_metadata_country_code.as_deref(),
                Some("new")
            );
            assert_eq!(
                actual.item.preferred_metadata_language.as_deref(),
                Some("new")
            );
            assert_eq!(actual.item.date_created, if default_date { 1 } else { 2 });
        } else {
            assert_eq!(actual.item.locked_fields, vec![MetadataField::Genres]);
            assert!(!actual.item.is_locked);
            assert_eq!(
                actual.item.preferred_metadata_country_code.as_deref(),
                Some("old")
            );
            assert_eq!(
                actual.item.preferred_metadata_language.as_deref(),
                Some("old")
            );
            assert_eq!(actual.item.date_created, 1);
        }
    }
}

#[derive(Clone, Copy)]
enum StringProperty {
    Name,
    OriginalTitle,
    OfficialRating,
    CustomRating,
    Tagline,
    Overview,
    DisplayOrder,
    ForcedSortName,
}

fn set_string(item: &mut MetadataItem, property: StringProperty, value: Option<String>) {
    match property {
        StringProperty::Name => item.core.name = value,
        StringProperty::OriginalTitle => item.original_title = value,
        StringProperty::OfficialRating => item.official_rating = value,
        StringProperty::CustomRating => item.custom_rating = value,
        StringProperty::Tagline => item.tagline = value,
        StringProperty::Overview => item.core.overview = value,
        StringProperty::DisplayOrder => item.display_order = value,
        StringProperty::ForcedSortName => item.forced_sort_name = value,
    }
}

fn get_string(item: &MetadataItem, property: StringProperty) -> Option<&str> {
    match property {
        StringProperty::Name => item.core.name.as_deref(),
        StringProperty::OriginalTitle => item.original_title.as_deref(),
        StringProperty::OfficialRating => item.official_rating.as_deref(),
        StringProperty::CustomRating => item.custom_rating.as_deref(),
        StringProperty::Tagline => item.tagline.as_deref(),
        StringProperty::Overview => item.core.overview.as_deref(),
        StringProperty::DisplayOrder => item.display_order.as_deref(),
        StringProperty::ForcedSortName => item.forced_sort_name.as_deref(),
    }
}

fn test_string_merge(
    property: StringProperty,
    old: Option<&str>,
    new: Option<&str>,
    lock: Option<MetadataField>,
    replace_data: bool,
) -> bool {
    let mut source = MetadataResult::default();
    set_string(&mut source.item, property, new.map(str::to_owned));
    let mut target = MetadataResult::default();
    set_string(&mut target.item, property, old.map(str::to_owned));
    let locked = lock.map_or_else(Vec::new, |field| vec![field]);
    let (actual, _) = merge(&source, target, &locked, replace_data, false);
    get_string(&actual.item, property) == new
}

#[test]
fn merge_base_item_data_string_field_replaces_appropriately() {
    let cases = [
        (StringProperty::Name, Some(MetadataField::Name), false),
        (StringProperty::OriginalTitle, None, true),
        (
            StringProperty::OfficialRating,
            Some(MetadataField::OfficialRating),
            true,
        ),
        (StringProperty::CustomRating, None, true),
        (StringProperty::Tagline, None, true),
        (
            StringProperty::Overview,
            Some(MetadataField::Overview),
            true,
        ),
        (StringProperty::DisplayOrder, None, false),
        (StringProperty::ForcedSortName, None, false),
    ];

    for (property, lock, replaces_with_empty) in cases {
        assert!(!test_string_merge(
            property,
            Some("Old"),
            Some("New"),
            None,
            false
        ));
        if let Some(lock) = lock {
            assert!(!test_string_merge(
                property,
                Some("Old"),
                Some("New"),
                Some(lock),
                true
            ));
            assert!(!test_string_merge(
                property,
                None,
                Some("New"),
                Some(lock),
                false
            ));
            assert!(!test_string_merge(
                property,
                Some(""),
                Some("New"),
                Some(lock),
                false
            ));
        }
        assert!(test_string_merge(
            property,
            Some("Old"),
            Some("New"),
            None,
            true
        ));
        assert!(test_string_merge(property, None, Some("New"), None, false));
        assert!(test_string_merge(
            property,
            Some(""),
            Some("New"),
            None,
            false
        ));
        assert_eq!(
            test_string_merge(property, Some("Old"), Some(""), None, true),
            replaces_with_empty
        );
    }
}

#[derive(Clone, Copy)]
enum ArrayProperty {
    Genres,
    Studios,
    Tags,
    ProductionLocations,
    AlbumArtists,
}

fn set_array(item: &mut MetadataItem, property: ArrayProperty, value: Vec<String>) {
    match property {
        ArrayProperty::Genres => item.genres = value,
        ArrayProperty::Studios => item.studios = value,
        ArrayProperty::Tags => item.tags = value,
        ArrayProperty::ProductionLocations => item.production_locations = value,
        ArrayProperty::AlbumArtists => item.album_artists = value,
    }
}

fn get_array(item: &MetadataItem, property: ArrayProperty) -> &[String] {
    match property {
        ArrayProperty::Genres => &item.genres,
        ArrayProperty::Studios => &item.studios,
        ArrayProperty::Tags => &item.tags,
        ArrayProperty::ProductionLocations => &item.production_locations,
        ArrayProperty::AlbumArtists => &item.album_artists,
    }
}

fn test_array_merge(
    property: ArrayProperty,
    old: &[&str],
    new: &[&str],
    lock: Option<MetadataField>,
    replace_data: bool,
) -> bool {
    let mut source = MetadataResult::default();
    set_array(
        &mut source.item,
        property,
        new.iter().map(ToString::to_string).collect(),
    );
    let mut target = MetadataResult::default();
    set_array(
        &mut target.item,
        property,
        old.iter().map(ToString::to_string).collect(),
    );
    let locked = lock.map_or_else(Vec::new, |field| vec![field]);
    let (actual, _) = merge(&source, target, &locked, replace_data, false);
    get_array(&actual.item, property) == new.iter().map(ToString::to_string).collect::<Vec<_>>()
}

#[test]
fn merge_base_item_data_string_array_field_replaces_appropriately() {
    let cases = [
        (ArrayProperty::Genres, Some(MetadataField::Genres)),
        (ArrayProperty::Studios, Some(MetadataField::Studios)),
        (ArrayProperty::Tags, Some(MetadataField::Tags)),
        (
            ArrayProperty::ProductionLocations,
            Some(MetadataField::ProductionLocations),
        ),
        (ArrayProperty::AlbumArtists, None),
    ];
    for (property, lock) in cases {
        assert!(!test_array_merge(property, &["Old"], &["New"], None, false));
        if let Some(lock) = lock {
            assert!(!test_array_merge(
                property,
                &["Old"],
                &["New"],
                Some(lock),
                true
            ));
            assert!(!test_array_merge(
                property,
                &[],
                &["New"],
                Some(lock),
                false
            ));
        }
        assert!(test_array_merge(property, &["Old"], &["New"], None, true));
        assert!(test_array_merge(property, &[], &["New"], None, false));
        assert!(test_array_merge(property, &["Old"], &[], None, true));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SimpleValue {
    Integer(i32),
    Float(f32),
    Date(i64),
    Video3d(Video3dFormat),
}

#[derive(Clone, Copy)]
enum SimpleProperty {
    IndexNumber,
    ParentIndexNumber,
    ProductionYear,
    CommunityRating,
    CriticRating,
    EndDate,
    PremiereDate,
    Video3dFormat,
}

fn set_simple(item: &mut MetadataItem, property: SimpleProperty, value: Option<SimpleValue>) {
    match (property, value) {
        (SimpleProperty::IndexNumber, value) => {
            item.core.index_number = value.map(expect_integer);
        }
        (SimpleProperty::ParentIndexNumber, value) => {
            item.core.parent_index_number = value.map(expect_integer);
        }
        (SimpleProperty::ProductionYear, value) => {
            item.production_year = value.map(expect_integer);
        }
        (SimpleProperty::CommunityRating, value) => {
            item.community_rating = value.map(expect_float);
        }
        (SimpleProperty::CriticRating, value) => {
            item.critic_rating = value.map(expect_float);
        }
        (SimpleProperty::EndDate, value) => item.end_date = value.map(expect_date),
        (SimpleProperty::PremiereDate, value) => item.premiere_date = value.map(expect_date),
        (SimpleProperty::Video3dFormat, value) => {
            item.video_3d_format = value.map(expect_video_3d);
        }
    }
}

fn get_simple(item: &MetadataItem, property: SimpleProperty) -> Option<SimpleValue> {
    match property {
        SimpleProperty::IndexNumber => item.core.index_number.map(SimpleValue::Integer),
        SimpleProperty::ParentIndexNumber => {
            item.core.parent_index_number.map(SimpleValue::Integer)
        }
        SimpleProperty::ProductionYear => item.production_year.map(SimpleValue::Integer),
        SimpleProperty::CommunityRating => item.community_rating.map(SimpleValue::Float),
        SimpleProperty::CriticRating => item.critic_rating.map(SimpleValue::Float),
        SimpleProperty::EndDate => item.end_date.map(SimpleValue::Date),
        SimpleProperty::PremiereDate => item.premiere_date.map(SimpleValue::Date),
        SimpleProperty::Video3dFormat => item.video_3d_format.map(SimpleValue::Video3d),
    }
}

fn expect_integer(value: SimpleValue) -> i32 {
    let SimpleValue::Integer(value) = value else {
        panic!("expected integer")
    };
    value
}

fn expect_float(value: SimpleValue) -> f32 {
    let SimpleValue::Float(value) = value else {
        panic!("expected float")
    };
    value
}

fn expect_date(value: SimpleValue) -> i64 {
    let SimpleValue::Date(value) = value else {
        panic!("expected date")
    };
    value
}

fn expect_video_3d(value: SimpleValue) -> Video3dFormat {
    let SimpleValue::Video3d(value) = value else {
        panic!("expected video 3d")
    };
    value
}

fn test_simple_merge(
    property: SimpleProperty,
    old: Option<SimpleValue>,
    new: Option<SimpleValue>,
    replace_data: bool,
) -> bool {
    let mut source = MetadataResult::default();
    set_simple(&mut source.item, property, new);
    let mut target = MetadataResult::default();
    set_simple(&mut target.item, property, old);
    let (actual, _) = merge(&source, target, &[], replace_data, false);
    get_simple(&actual.item, property) == new
}

#[test]
fn merge_base_item_data_simple_field_replaces_appropriately() {
    let cases = [
        (
            SimpleProperty::IndexNumber,
            SimpleValue::Integer(1),
            SimpleValue::Integer(2),
        ),
        (
            SimpleProperty::ParentIndexNumber,
            SimpleValue::Integer(1),
            SimpleValue::Integer(2),
        ),
        (
            SimpleProperty::ProductionYear,
            SimpleValue::Integer(1),
            SimpleValue::Integer(2),
        ),
        (
            SimpleProperty::CommunityRating,
            SimpleValue::Float(1.0),
            SimpleValue::Float(2.0),
        ),
        (
            SimpleProperty::CriticRating,
            SimpleValue::Float(1.0),
            SimpleValue::Float(2.0),
        ),
        (
            SimpleProperty::EndDate,
            SimpleValue::Date(1),
            SimpleValue::Date(2),
        ),
        (
            SimpleProperty::PremiereDate,
            SimpleValue::Date(1),
            SimpleValue::Date(2),
        ),
        (
            SimpleProperty::Video3dFormat,
            SimpleValue::Video3d(Video3dFormat::HalfSideBySide),
            SimpleValue::Video3d(Video3dFormat::FullSideBySide),
        ),
    ];
    for (property, old, new) in cases {
        assert!(!test_simple_merge(property, Some(old), Some(new), false));
        assert!(test_simple_merge(property, Some(old), Some(new), true));
        assert!(test_simple_merge(property, None, Some(new), false));
        assert_eq!(
            test_simple_merge(property, Some(old), None, true),
            !matches!(property, SimpleProperty::Video3dFormat)
        );
    }
}

fn trailer(name: &str, url: &str) -> MediaUrl {
    MediaUrl {
        name: name.to_owned(),
        url: url.to_owned(),
    }
}

fn test_trailer_merge(old: Vec<MediaUrl>, new: &[MediaUrl], replace_data: bool) -> bool {
    let source = MetadataResult {
        item: MetadataItem {
            remote_trailers: new.to_vec(),
            ..MetadataItem::default()
        },
        people: None,
    };
    let target = MetadataResult {
        item: MetadataItem {
            remote_trailers: old,
            ..MetadataItem::default()
        },
        people: None,
    };
    let (actual, _) = merge(&source, target, &[], replace_data, false);
    actual.item.remote_trailers == new
}

#[test]
fn merge_base_item_data_merge_trailers_replaces_appropriately() {
    let old = vec![trailer("Name 1", "URL 1")];
    let new = vec![trailer("Name 2", "URL 2")];
    assert!(!test_trailer_merge(old.clone(), &new, false));
    assert!(test_trailer_merge(old.clone(), &new, true));
    assert!(test_trailer_merge(Vec::new(), &new, false));
    assert!(test_trailer_merge(old, &[], true));
}

fn provider_ids(entries: &[(&str, &str)]) -> ProviderIdMap {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn merge_provider_id_maps(
    old: ProviderIdMap,
    new: ProviderIdMap,
    replace_data: bool,
) -> ProviderIdMap {
    let source = MetadataResult {
        item: MetadataItem {
            core: jellyfin_providers::tv::EpisodeMetadata {
                provider_ids: new,
                ..jellyfin_providers::tv::EpisodeMetadata::default()
            },
            ..MetadataItem::default()
        },
        people: None,
    };
    let target = MetadataResult {
        item: MetadataItem {
            core: jellyfin_providers::tv::EpisodeMetadata {
                provider_ids: old,
                ..jellyfin_providers::tv::EpisodeMetadata::default()
            },
            ..MetadataItem::default()
        },
        people: None,
    };
    merge(&source, target, &[], replace_data, false)
        .0
        .item
        .core
        .provider_ids
}

#[test]
fn merge_base_item_data_provider_ids_merges_appropriately() {
    let old = provider_ids(&[("provider 1", "id 1")]);
    let overwrite = provider_ids(&[("provider 1", "id 2")]);
    assert_ne!(
        merge_provider_id_maps(old.clone(), overwrite.clone(), false),
        overwrite
    );
    assert_eq!(
        merge_provider_id_maps(old.clone(), overwrite.clone(), true),
        overwrite
    );

    let additions = provider_ids(&[("provider 1", "id 2"), ("provider 2", "id 3")]);
    let merged = merge_provider_id_maps(old.clone(), additions, false);
    assert_eq!(merged["provider 1"], "id 1");
    assert_eq!(merged["provider 2"], "id 3");
    assert_eq!(
        merge_provider_id_maps(old.clone(), ProviderIdMap::new(), true),
        old
    );
}

fn old_people() -> Vec<PersonInfo> {
    vec![PersonInfo {
        name: "Name 1".to_owned(),
        provider_ids: provider_ids(&[("Provider 1", "1234")]),
        ..PersonInfo::default()
    }]
}

fn merge_people(
    old: Option<Vec<PersonInfo>>,
    new: Option<&[PersonInfo]>,
    lock: Option<MetadataField>,
    replace_data: bool,
) -> (Option<Vec<PersonInfo>>, bool, usize) {
    let source = MetadataResult {
        item: MetadataItem::default(),
        people: new.map(<[PersonInfo]>::to_vec),
    };
    let target = MetadataResult {
        item: MetadataItem::default(),
        people: old,
    };
    let locked = lock.map_or_else(Vec::new, |field| vec![field]);
    let (actual, capability) = merge(&source, target, &locked, replace_data, false);
    let matches_source = actual.people.as_deref() == new;
    (
        actual.people,
        matches_source,
        capability.person_key_calls.get(),
    )
}

#[test]
fn merge_base_item_data_merge_people_merges_appropriately() {
    let different_person = vec![PersonInfo {
        name: "Name 2".to_owned(),
        ..PersonInfo::default()
    }];
    let (actual, matches, _) =
        merge_people(Some(old_people()), Some(&different_person), None, false);
    assert!(!matches);
    assert_eq!(actual.unwrap()[0].name, "Name 1");
    assert!(merge_people(Some(old_people()), Some(&different_person), None, true).1);
    assert!(merge_people(Some(Vec::new()), Some(&different_person), None, false).1);
    assert!(merge_people(None, Some(&different_person), None, false).1);
    assert!(
        !merge_people(
            Some(old_people()),
            Some(&different_person),
            Some(MetadataField::Cast),
            true
        )
        .1
    );

    let matching_person = vec![PersonInfo {
        name: "Name 1".to_owned(),
        provider_ids: provider_ids(&[("Provider 1", "5678"), ("Provider 2", "5678")]),
        ..PersonInfo::default()
    }];
    let (actual, _, key_calls) =
        merge_people(Some(old_people()), Some(&matching_person), None, false);
    assert!(key_calls > 0);
    let actual = actual.unwrap();
    assert_eq!(actual.len(), 1);
    assert_eq!(actual[0].name, "Name 1");
    assert_eq!(actual[0].provider_ids.len(), 2);
    assert_eq!(actual[0].provider_ids["Provider 1"], "1234");
    assert_eq!(actual[0].provider_ids["Provider 2"], "5678");

    let picture_1 = vec![PersonInfo {
        name: "Name 1".to_owned(),
        image_url: Some("URL 1".to_owned()),
        ..PersonInfo::default()
    }];
    let (actual, _, _) = merge_people(Some(old_people()), Some(&picture_1), None, false);
    assert_eq!(actual.unwrap()[0].image_url.as_deref(), Some("URL 1"));
    let picture_2 = vec![PersonInfo {
        name: "Name 1".to_owned(),
        image_url: Some("URL 2".to_owned()),
        ..PersonInfo::default()
    }];
    let (actual, _, _) = merge_people(Some(picture_1), Some(&picture_2), None, false);
    assert_eq!(actual.unwrap()[0].image_url.as_deref(), Some("URL 1"));
    assert!(merge_people(Some(old_people()), Some(&[]), None, true).1);
}
