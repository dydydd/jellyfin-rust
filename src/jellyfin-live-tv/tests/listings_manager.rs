use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use jellyfin_live_tv::listings::{
    JsonListingsConfigurationStore, ListingProviderConfiguration, ListingsConfigurationStore,
    ListingsManager, LiveTvConfiguration, MemoryListingsConfigurationStore,
};
use serde_json::json;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn provider(id: &str, provider_type: &str) -> ListingProviderConfiguration {
    ListingProviderConfiguration {
        id: Some(id.to_owned()),
        provider_type: Some(provider_type.to_owned()),
        ..ListingProviderConfiguration::default()
    }
}

#[test]
fn delete_listings_provider_deletes_provider() {
    let target_id = "MockId";
    let store = Arc::new(MemoryListingsConfigurationStore::new(LiveTvConfiguration {
        listing_providers: vec![provider(target_id, "Mock")],
        ..LiveTvConfiguration::default()
    }));
    let manager = ListingsManager::new(store);

    assert!(manager.delete_listings_provider(target_id).unwrap());
    assert!(
        manager
            .configuration()
            .unwrap()
            .listing_providers
            .iter()
            .all(|provider| provider.id.as_deref() != Some(target_id))
    );
}

#[test]
fn file_store_removes_only_the_target_and_preserves_unknown_configuration() {
    let fixture = ConfigurationFixture::new();
    let store: Arc<dyn ListingsConfigurationStore> =
        Arc::new(JsonListingsConfigurationStore::new(&fixture.path));
    let manager = ListingsManager::new(Arc::clone(&store));
    let mut provider_extra = BTreeMap::new();
    provider_extra.insert(
        "FutureProviderOption".to_owned(),
        json!({ "enabled": true }),
    );
    let mut configuration_extra = BTreeMap::new();
    configuration_extra.insert("FutureLiveTvOption".to_owned(), json!([1, 2, 3]));

    store
        .mutate(&mut |configuration| {
            configuration.guide_days = Some(12);
            configuration.extra = configuration_extra.clone();
            configuration.listing_providers = vec![
                provider("first", "XmlTv"),
                provider("target", "SchedulesDirect"),
                ListingProviderConfiguration {
                    extra: provider_extra.clone(),
                    ..provider("last", "XmlTv")
                },
            ];
            true
        })
        .unwrap();

    assert!(manager.delete_listings_provider("TARGET").unwrap());

    let persisted = manager.configuration().unwrap();
    assert_eq!(persisted.guide_days, Some(12));
    assert_eq!(persisted.extra, configuration_extra);
    assert_eq!(
        persisted
            .listing_providers
            .iter()
            .filter_map(|provider| provider.id.as_deref())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    assert_eq!(persisted.listing_providers[1].extra, provider_extra);

    let from_disk: LiveTvConfiguration =
        serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
    assert_eq!(from_disk, persisted);
}

#[test]
fn unknown_and_repeated_deletes_are_idempotent_and_do_not_rewrite_the_file() {
    let fixture = ConfigurationFixture::new();
    let store: Arc<dyn ListingsConfigurationStore> =
        Arc::new(JsonListingsConfigurationStore::new(&fixture.path));
    let manager = ListingsManager::new(Arc::clone(&store));
    store
        .mutate(&mut |configuration| {
            configuration.listing_providers = vec![provider("target", "XmlTv")];
            true
        })
        .unwrap();

    let original = std::fs::read(&fixture.path).unwrap();
    assert!(!manager.delete_listings_provider("unknown").unwrap());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), original);

    assert!(manager.delete_listings_provider("target").unwrap());
    let after_delete = std::fs::read(&fixture.path).unwrap();
    assert!(!manager.delete_listings_provider("target").unwrap());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), after_delete);
}

#[test]
fn rejected_in_memory_mutations_do_not_change_configuration() {
    let store = MemoryListingsConfigurationStore::new(LiveTvConfiguration {
        guide_days: Some(7),
        ..LiveTvConfiguration::default()
    });

    assert!(
        !store
            .mutate(&mut |configuration| {
                configuration.guide_days = Some(99);
                false
            })
            .unwrap()
    );
    assert_eq!(store.load().unwrap().guide_days, Some(7));
}

#[test]
fn concurrent_deletes_do_not_restore_providers_removed_by_other_threads() {
    const DELETE_COUNT: usize = 16;

    let fixture = ConfigurationFixture::new();
    let store: Arc<dyn ListingsConfigurationStore> =
        Arc::new(JsonListingsConfigurationStore::new(&fixture.path));
    let manager = Arc::new(ListingsManager::new(Arc::clone(&store)));
    store
        .mutate(&mut |configuration| {
            configuration.listing_providers = (0..DELETE_COUNT)
                .map(|index| provider(&format!("delete-{index}"), "XmlTv"))
                .chain([
                    provider("keep-first", "SchedulesDirect"),
                    provider("keep-last", "XmlTv"),
                ])
                .collect();
            true
        })
        .unwrap();

    let barrier = Arc::new(Barrier::new(DELETE_COUNT + 1));
    let workers = (0..DELETE_COUNT)
        .map(|index| {
            let manager = Arc::clone(&manager);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                manager
                    .delete_listings_provider(&format!("DELETE-{index}"))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    for worker in workers {
        assert!(worker.join().unwrap());
    }

    let persisted = manager.configuration().unwrap();
    assert_eq!(
        persisted
            .listing_providers
            .iter()
            .filter_map(|provider| provider.id.as_deref())
            .collect::<Vec<_>>(),
        ["keep-first", "keep-last"]
    );
}

struct ConfigurationFixture {
    root: PathBuf,
    path: PathBuf,
}

impl ConfigurationFixture {
    fn new() -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "jellyfin-rust-listings-manager-{}-{sequence}",
            std::process::id()
        ));
        let path = root.join("config").join("livetv.json");
        Self { root, path }
    }
}

impl Drop for ConfigurationFixture {
    fn drop(&mut self) {
        if Path::new(&self.root).exists() {
            std::fs::remove_dir_all(&self.root).expect("temporary configuration cleanup");
        }
    }
}
