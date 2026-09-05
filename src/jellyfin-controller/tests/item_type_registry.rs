use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use jellyfin_controller::{
    ItemTypeRegistrationError, ItemTypeRegistry, UserLibraryError, UserLibraryService, UserService,
};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, NewBaseItem,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;

// Official DeserializeBaseItem_WithUnknownType_ReturnsNull.
#[test]
fn unknown_persisted_type_returns_none() {
    let registry = ItemTypeRegistry::default();
    assert!(
        registry
            .hydrate(item("NonExistent.Plugin.CustomItemType"))
            .is_none()
    );
}

// Official DeserializeBaseItem_WithUnknownType_LogsWarning.
#[test]
fn unknown_persisted_type_logs_one_warning() {
    let registry = ItemTypeRegistry::default();
    let model = item("NonExistent.Plugin.CustomItemType");
    let item_id = model.id.to_string();
    let output = capture_tracing(|| assert!(registry.hydrate(model).is_none()));

    assert_eq!(output.matches("unknown type").count(), 1, "{output}");
    assert!(
        output.contains("NonExistent.Plugin.CustomItemType"),
        "{output}"
    );
    assert!(output.contains(&item_id), "{output}");
}

// Official DeserializeBaseItem_WithKnownType_ReturnsItem.
#[test]
fn official_qualified_type_hydrates_to_canonical_kind() {
    let registry = ItemTypeRegistry::default();
    let hydrated = registry
        .hydrate(item("MediaBrowser.Controller.Entities.Movies.Movie"))
        .expect("official fully-qualified Movie type must hydrate");
    assert_eq!(hydrated.item_type().name(), "Movie");
    assert_eq!(
        hydrated.model().item_type,
        "MediaBrowser.Controller.Entities.Movies.Movie"
    );
    assert_eq!(hydrated.into_model().item_type, "Movie");
}

#[test]
fn current_short_item_types_have_explicit_default_mappings() {
    let registry = ItemTypeRegistry::default();
    for item_type in [
        "Folder",
        "Movie",
        "MusicGenre",
        "Person",
        "Series",
        "UserRootFolder",
        "Video",
    ] {
        assert_eq!(
            registry
                .resolve(item_type)
                .expect("current short item type must be registered")
                .name(),
            item_type
        );
    }
}

#[test]
fn names_are_case_sensitive_and_plugin_registration_is_shared_and_atomic() {
    let registry = ItemTypeRegistry::default();
    let shared = registry.clone();
    assert!(registry.resolve("movie").is_none());
    assert!(registry.resolve("").is_none());

    registry
        .register(
            "CustomItem",
            [
                "Example.Plugin.CustomItem",
                "Example.Plugin.LegacyCustomItem",
            ],
        )
        .expect("plugin item type registration");
    assert_eq!(
        shared
            .resolve("Example.Plugin.CustomItem")
            .expect("clones must observe plugin registrations")
            .name(),
        "CustomItem"
    );
    assert!(registry.resolve("example.plugin.customitem").is_none());

    assert_eq!(
        registry.register("OtherItem", ["Example.Plugin.CustomItem"]),
        Err(ItemTypeRegistrationError::DuplicatePersistedName(
            "Example.Plugin.CustomItem".to_owned()
        ))
    );
    assert!(registry.resolve("OtherItem").is_none());

    assert_eq!(
        registry.register("AnotherItem", ["Example.Plugin.Valid", " "]),
        Err(ItemTypeRegistrationError::InvalidName)
    );
    assert!(registry.resolve("AnotherItem").is_none());
    assert!(registry.resolve("Example.Plugin.Valid").is_none());
}

#[tokio::test]
async fn controller_rejects_unknown_raw_row_until_plugin_type_is_registered() {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let suffix = Uuid::new_v4().simple().to_string();
    let user_service = UserService::new(database.clone());
    let user = user_service
        .create(&format!("item-type-{suffix}"))
        .await
        .expect("item-type test user");
    let items = BaseItemRepository::new(database.clone());
    let raw = items
        .create(NewBaseItem::new(
            Uuid::new_v4(),
            "Example.Plugin.CustomItem",
        ))
        .await
        .expect("unknown raw item must remain persistable");
    let registry = ItemTypeRegistry::default();
    let service = UserLibraryService::with_item_type_registry(database.clone(), registry.clone());

    assert!(matches!(
        service.item(&user, user.id, raw.id).await,
        Err(UserLibraryError::ItemNotFound)
    ));
    assert!(
        items.get(raw.id).await.expect("raw item lookup").is_some(),
        "controller rejection must not delete or hide the data repository row"
    );

    registry
        .register("CustomItem", ["Example.Plugin.CustomItem"])
        .expect("runtime plugin type registration");
    let hydrated = service
        .item(&user, user.id, raw.id)
        .await
        .expect("shared registry update must reach the existing service");
    assert_eq!(hydrated.id, raw.id);

    items.delete(raw.id).await.expect("raw item cleanup");
    user::Entity::delete_many()
        .filter(user::Column::Id.eq(user.id))
        .exec(&database)
        .await
        .expect("item-type test user cleanup");
}

fn item(item_type: &str) -> base_item::Model {
    base_item::Model {
        id: Uuid::new_v4(),
        item_type: item_type.to_owned(),
        data: None,
        path: None,
        parent_id: None,
        top_parent_id: None,
        name: None,
        clean_name: None,
        sort_name: None,
        media_type: None,
        overview: None,
        official_rating: None,
        index_number: None,
        parent_index_number: None,
        production_year: None,
        premiere_date: None,
        runtime_ticks: None,
        is_folder: false,
        is_virtual_item: false,
        presentation_unique_key: None,
        primary_version_id: None,
        series_id: None,
        season_id: None,
        series_presentation_unique_key: None,
        date_created: Utc::now(),
        date_modified: Utc::now(),
        row_version: 1,
    }
}

#[derive(Clone, Default)]
struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
}

struct CaptureWriter(Capture);

impl Write for CaptureWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Capture {
    type Writer = CaptureWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CaptureWriter(self.clone())
    }
}

fn capture_tracing(test: impl FnOnce()) -> String {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(capture.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, test);
    let bytes = capture
        .bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    String::from_utf8(bytes).expect("captured tracing output must be UTF-8")
}
