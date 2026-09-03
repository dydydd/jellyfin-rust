use jellyfin_controller::library::{
    BaseItemInfo, BaseItemManager, ServerConfiguration, SourceType, TypeOptions,
};

fn manager_with_official_test_configuration() -> BaseItemManager {
    let mut configuration = ServerConfiguration::default();
    for options in &mut configuration.metadata_options {
        options.disabled_metadata_fetchers = vec!["ServerDisabled".to_owned()];
        options.disabled_image_fetchers = vec!["ServerDisabled".to_owned()];
    }
    BaseItemManager::new(configuration)
}

fn library_options(item_type: &str) -> TypeOptions {
    TypeOptions {
        item_type: item_type.to_owned(),
        metadata_fetchers: vec!["LibraryEnabled".to_owned()],
        image_fetchers: vec!["LibraryEnabled".to_owned()],
        ..TypeOptions::default()
    }
}

fn assert_metadata_fetcher(item_type: &str, name: &str, expected: bool) {
    let manager = manager_with_official_test_configuration();
    let item = BaseItemInfo::new(item_type);
    let options = (item_type == "Book").then(|| library_options(item_type));

    assert_eq!(
        manager.is_metadata_fetcher_enabled(&item, options.as_ref(), name),
        expected
    );
}

fn assert_image_fetcher(item_type: &str, name: &str, expected: bool) {
    let manager = manager_with_official_test_configuration();
    let item = BaseItemInfo::new(item_type);
    let options = (item_type == "Book").then(|| library_options(item_type));

    assert_eq!(
        manager.is_image_fetcher_enabled(&item, options.as_ref(), name),
        expected
    );
}

macro_rules! fetcher_test {
    ($name:ident, $assertion:ident, $item_type:literal, $fetcher:literal, $expected:literal) => {
        #[test]
        fn $name() {
            $assertion($item_type, $fetcher, $expected);
        }
    };
}

fetcher_test!(
    metadata_fetcher_book_library_enabled,
    assert_metadata_fetcher,
    "Book",
    "LibraryEnabled",
    true
);
fetcher_test!(
    metadata_fetcher_book_library_disabled,
    assert_metadata_fetcher,
    "Book",
    "LibraryDisabled",
    false
);
fetcher_test!(
    metadata_fetcher_music_artist_enabled,
    assert_metadata_fetcher,
    "MusicArtist",
    "Enabled",
    true
);
fetcher_test!(
    metadata_fetcher_music_artist_server_disabled,
    assert_metadata_fetcher,
    "MusicArtist",
    "ServerDisabled",
    false
);
fetcher_test!(
    image_fetcher_book_library_enabled,
    assert_image_fetcher,
    "Book",
    "LibraryEnabled",
    true
);
fetcher_test!(
    image_fetcher_book_library_disabled,
    assert_image_fetcher,
    "Book",
    "LibraryDisabled",
    false
);
fetcher_test!(
    image_fetcher_music_artist_enabled,
    assert_image_fetcher,
    "MusicArtist",
    "Enabled",
    true
);
fetcher_test!(
    image_fetcher_music_artist_server_disabled,
    assert_image_fetcher,
    "MusicArtist",
    "ServerDisabled",
    false
);

#[test]
fn fetcher_names_and_item_types_are_matched_case_insensitively() {
    let mut configuration = ServerConfiguration::default();
    configuration
        .metadata_options
        .iter_mut()
        .find(|options| options.item_type == "Book")
        .unwrap()
        .disabled_metadata_fetchers = vec!["MixedCaseProvider".to_owned()];
    let manager = BaseItemManager::new(configuration);

    assert!(!manager.is_metadata_fetcher_enabled(
        &BaseItemInfo::new("bOoK"),
        None,
        "mixedcaseprovider"
    ));

    let options = TypeOptions {
        metadata_fetchers: vec!["MixedCaseProvider".to_owned()],
        ..TypeOptions::default()
    };
    assert!(manager.is_metadata_fetcher_enabled(
        &BaseItemInfo::new("Book"),
        Some(&options),
        "mixedcaseprovider"
    ));
}

#[test]
fn channel_fetcher_rules_match_official_special_cases() {
    let manager = BaseItemManager::new(ServerConfiguration::default());
    let mut channel = BaseItemInfo::new("Channel");
    channel.is_channel = true;
    channel.enable_media_source_display = true;
    assert!(manager.is_metadata_fetcher_enabled(&channel, None, "Disabled"));
    assert!(manager.is_image_fetcher_enabled(&channel, None, "Disabled"));

    let mut channel_item = BaseItemInfo::new("Movie");
    channel_item.source_type = SourceType::Channel;
    assert!(manager.is_metadata_fetcher_enabled(&channel_item, None, "Disabled"));
    assert!(manager.is_image_fetcher_enabled(&channel_item, None, "Disabled"));

    channel_item.enable_media_source_display = true;
    assert!(!manager.is_metadata_fetcher_enabled(&channel_item, None, "Enabled"));
    assert!(!manager.is_image_fetcher_enabled(&channel_item, None, "Enabled"));
}

#[test]
fn missing_server_metadata_configuration_leaves_fetchers_enabled() {
    let manager = BaseItemManager::new(ServerConfiguration::default());
    let item = BaseItemInfo::new("UnknownItemType");

    assert!(manager.is_metadata_fetcher_enabled(&item, None, "AnyProvider"));
    assert!(manager.is_image_fetcher_enabled(&item, None, "AnyProvider"));
}
