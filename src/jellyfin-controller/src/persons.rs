use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, CanonicalPersonEntity,
    PersonError as PersonRepositoryError, PersonQuery, PersonRepository,
    entities::{base_item, person, user},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ItemByNameError, ItemByNameKind, ItemByNameService, UserError, UserLibraryError,
    UserLibraryService, UserService,
};

const PERSON_RECONCILIATION_BATCH_SIZE: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub model: person::Model,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonPage {
    pub people: Vec<Person>,
    pub total_record_count: u64,
    pub start_index: u64,
}

#[derive(Debug, Error)]
pub enum PersonError {
    #[error("person was not found")]
    NotFound,
    #[error("target user was not found")]
    UserNotFound,
    #[error("person query is forbidden")]
    Forbidden,
    #[error(transparent)]
    User(#[from] UserError),
    #[error(transparent)]
    Repository(#[from] PersonRepositoryError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    UserLibrary(#[from] UserLibraryError),
}

#[derive(Debug, Error)]
pub enum PersonReconciliationError {
    #[error(transparent)]
    Repository(#[from] PersonRepositoryError),
    #[error(transparent)]
    ItemByName(#[from] ItemByNameError),
}

/// Aggregate result of one bounded canonical-Person reconciliation run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PersonReconciliationSummary {
    pub people_considered: u64,
    pub batches_committed: u64,
    pub created_items: u64,
    pub updated_items: u64,
    pub copied_images: u64,
    pub cancelled: bool,
}

/// Creates canonical Person item rows for referenced credit names without
/// rewriting the internal people catalog or deleting legacy rows.
#[derive(Clone)]
pub struct PersonReconciliationService {
    people: PersonRepository,
    item_by_name: ItemByNameService,
}

impl PersonReconciliationService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            people: PersonRepository::new(std::sync::Arc::clone(&database)),
            item_by_name: ItemByNameService::new(database),
        }
    }

    /// Replaces the roots used for canonical Person paths and identifiers.
    pub fn set_item_by_name_directories(
        &self,
        program_data_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) {
        self.item_by_name
            .set_directories(program_data_directory, internal_metadata_directory);
    }

    /// Reconciles all referenced people in fixed keyset pages.
    ///
    /// Cancellation is observed before starting every page. A page already in
    /// its PostgreSQL transaction either commits fully or rolls back.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn reconcile(
        &self,
        cancellation: &AtomicBool,
    ) -> Result<PersonReconciliationSummary, PersonReconciliationError> {
        self.reconcile_with_observer(cancellation, |_| {}).await
    }

    async fn reconcile_with_observer(
        &self,
        cancellation: &AtomicBool,
        mut on_batch: impl FnMut(PersonReconciliationSummary),
    ) -> Result<PersonReconciliationSummary, PersonReconciliationError> {
        let mut summary = PersonReconciliationSummary::default();
        let mut after_clean_name = None::<String>;
        loop {
            if cancellation.load(Ordering::Acquire) {
                summary.cancelled = true;
                break;
            }
            let people = self
                .people
                .referenced_page_after(
                    after_clean_name.as_deref(),
                    PERSON_RECONCILIATION_BATCH_SIZE,
                )
                .await?;
            let Some(last_person) = people.last() else {
                break;
            };
            let next_after = last_person.clean_name.clone();
            let page_len = people.len();
            let names = people
                .iter()
                .map(|person| person.name.clone())
                .collect::<Vec<_>>();
            let entities = self
                .item_by_name
                .prepare_many_direct(ItemByNameKind::Person, names)
                .await?;
            let entries = people
                .into_iter()
                .zip(entities)
                .map(|(person, entity)| CanonicalPersonEntity {
                    person_id: person.id,
                    provider_ids: person.provider_ids,
                    entity,
                })
                .collect::<Vec<_>>();
            let batch = self.people.reconcile_canonical_batch(&entries).await?;
            summary.people_considered = summary
                .people_considered
                .saturating_add(u64::try_from(page_len).unwrap_or(u64::MAX));
            summary.batches_committed = summary.batches_committed.saturating_add(1);
            summary.created_items = summary.created_items.saturating_add(batch.created_items);
            summary.updated_items = summary.updated_items.saturating_add(batch.updated_items);
            summary.copied_images = summary.copied_images.saturating_add(batch.copied_images);
            on_batch(summary);
            after_clean_name = Some(next_after);
            if page_len < PERSON_RECONCILIATION_BATCH_SIZE {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(summary)
    }
}

#[derive(Clone)]
pub struct PersonService {
    users: UserService,
    items: BaseItemRepository,
    people: PersonRepository,
    user_library: UserLibraryService,
}

impl PersonService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            people: PersonRepository::new(std::sync::Arc::clone(&database)),
            user_library: UserLibraryService::new(database),
        }
    }

    /// Resolves a person by exact display name and then Unicode clean name.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn get(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        name: &str,
    ) -> Result<Person, PersonError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let person = match self.people.get_exact(name).await {
            Ok(Some(person)) => person,
            Ok(None) => match self.people.get_normalized(name).await {
                Ok(Some(person)) => person,
                Ok(None) | Err(PersonRepositoryError::InvalidName) => {
                    return Err(PersonError::NotFound);
                }
                Err(error) => return Err(error.into()),
            },
            Err(PersonRepositoryError::InvalidName) => return Err(PersonError::NotFound),
            Err(error) => return Err(error.into()),
        };
        Ok(Person { model: person })
    }

    /// Resolves the persisted `Person` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, PersonError> {
        Ok(self.items.get_by_type_and_name("Person", name).await?)
    }

    /// Lists people credited to filtered library items.
    ///
    /// # Errors
    ///
    /// Returns not-found, forbidden, validation, or persistence errors.
    pub async fn list(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
        mut query: PersonQuery,
    ) -> Result<PersonPage, PersonError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let mut access_filter = BaseItemQuery::default();
        self.user_library
            .apply_user_policy(&mut access_filter, target_user_id)
            .await?;
        query.access_filter = Some(access_filter);
        let page = self.people.query(&query).await?;
        Ok(PersonPage {
            people: page
                .people
                .into_iter()
                .map(|person| Person { model: person })
                .collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    async fn validate_user(
        &self,
        authenticated_user: &user::Model,
        target_user_id: Uuid,
    ) -> Result<(), PersonError> {
        match self.users.get(target_user_id).await {
            Ok(_) => {}
            Err(UserError::NotFound) => return Err(PersonError::UserNotFound),
            Err(error) => return Err(error.into()),
        }
        if authenticated_user.id != target_user_id && !authenticated_user.is_administrator {
            return Err(PersonError::Forbidden);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{future::Future, sync::atomic::AtomicBool};

    use chrono::Utc;
    use jellyfin_data::{
        BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
        NewBaseItem, NewBaseItemImage, NewPerson, PersonRepository, entities::base_item,
    };
    use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use serde_json::json;

    use super::*;

    const DATABASE_PREFIX: &str = "jellyfin_person_reconciliation_";

    #[tokio::test]
    async fn reconciliation_merges_legacy_data_and_images_without_overwriting_or_deleting() {
        run_database_test(exercise_legacy_merge).await;
    }

    #[tokio::test]
    async fn cancellation_stops_before_the_next_fixed_keyset_batch() {
        run_database_test(exercise_cancellation).await;
    }

    async fn run_database_test<F, Fut>(exercise: F)
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");

        let task_database_name = database_name.clone();
        let outcome = tokio::spawn(async move { exercise(task_database_name).await }).await;

        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        if let Err(error) = outcome {
            if error.is_panic() {
                std::panic::resume_unwind(error.into_panic());
            }
            panic!("temporary database test task was cancelled: {error}");
        }
    }

    async fn test_database(database_name: &str) -> jellyfin_data::SharedDatabase {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 8,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        std::sync::Arc::new(database)
    }

    async fn exercise_legacy_merge(database_name: String) {
        let database = test_database(&database_name).await;
        let storage_root = std::env::temp_dir().join(format!(
            "jellyfin-person-reconciliation-{}",
            Uuid::new_v4().simple()
        ));
        let metadata_root = storage_root.join("metadata");
        let items = BaseItemRepository::new(std::sync::Arc::clone(&database));
        let people = PersonRepository::new(std::sync::Arc::clone(&database));
        let images = BaseItemImageRepository::new(std::sync::Arc::clone(&database));
        let first_media = create_item(&items, "Movie", "First credit owner").await;
        let second_media = create_item(&items, "Episode", "Second credit owner").await;
        let name = "Merge Person";
        let person = people
            .link(
                first_media,
                NewPerson {
                    name: name.to_owned(),
                    provider_ids: json!({ "Tmdb": "people-new", "Imdb": "nm-people" }),
                },
                "Actor",
                None,
                Some(0),
                0,
            )
            .await
            .expect("first credit must persist");
        let duplicate_credit = people
            .link(
                second_media,
                NewPerson::new(name),
                "Director",
                None,
                Some(0),
                0,
            )
            .await
            .expect("second credit must persist");
        assert_eq!(duplicate_credit.id, person.id);

        let locked_legacy_id = create_legacy_person(
            &items,
            name,
            json!({
                "IsLocked": true,
                "Biography": "locked biography",
                "ProviderIds": { "Tmdb": "legacy", "Tvdb": "tvdb-legacy" }
            }),
            Some("locked overview"),
        )
        .await;
        let newer_legacy_id = create_legacy_person(
            &items,
            name,
            json!({ "Biography": "newer biography" }),
            Some("newer overview"),
        )
        .await;
        for (item_id, path) in [
            (locked_legacy_id, "locked-person.jpg"),
            (newer_legacy_id, "newer-person.jpg"),
        ] {
            images
                .set_or_append(
                    item_id,
                    NewBaseItemImage {
                        image_type: BaseItemImageType::Primary,
                        image_index: 0,
                        path: storage_root.join(path).to_string_lossy().into_owned(),
                        date_modified: Utc::now(),
                        width: Some(400),
                        height: Some(600),
                        blurhash: Some(path.to_owned()),
                    },
                )
                .await
                .expect("legacy image must persist");
        }

        let service = PersonReconciliationService::new(std::sync::Arc::clone(&database));
        service.set_item_by_name_directories(&storage_root, &metadata_root);
        let mut canonical = service
            .item_by_name
            .resolve_direct(ItemByNameKind::Person, name)
            .await
            .expect("canonical person must be creatable");
        canonical.data = Some(json!({
            "CanonicalOnly": "keep",
            "ProviderIds": { "tmdb": "canonical" }
        }));
        canonical.overview = None;
        let canonical = items
            .update(canonical)
            .await
            .expect("canonical fixture update must succeed");
        let original_row_version = canonical.row_version;

        let cancellation = AtomicBool::new(false);
        let first = service
            .reconcile(&cancellation)
            .await
            .expect("first reconciliation must succeed");
        assert_eq!(first.people_considered, 1);
        assert_eq!(first.batches_committed, 1);
        assert_eq!(first.created_items, 0);
        assert_eq!(first.updated_items, 1);
        assert_eq!(first.copied_images, 1);
        assert!(!first.cancelled);

        let canonical = items
            .get(canonical.id)
            .await
            .expect("canonical lookup must succeed")
            .expect("canonical person must remain");
        assert_eq!(canonical.overview.as_deref(), Some("locked overview"));
        let data = canonical.data.as_ref().expect("canonical data must exist");
        assert_eq!(data["CanonicalOnly"], "keep");
        assert_eq!(data["Biography"], "locked biography");
        assert_eq!(data["ProviderIds"]["tmdb"], "canonical");
        assert!(data["ProviderIds"].get("Tmdb").is_none());
        assert_eq!(data["ProviderIds"]["Imdb"], "nm-people");
        assert_eq!(data["ProviderIds"]["Tvdb"], "tvdb-legacy");
        let canonical_image = images
            .primary(canonical.id)
            .await
            .expect("canonical image lookup must succeed")
            .expect("canonical image reference must be copied");
        assert!(canonical_image.path.ends_with("locked-person.jpg"));
        assert!(items.get(locked_legacy_id).await.unwrap().is_some());
        assert!(items.get(newer_legacy_id).await.unwrap().is_some());
        assert!(images.primary(locked_legacy_id).await.unwrap().is_some());
        assert!(images.primary(newer_legacy_id).await.unwrap().is_some());

        let second = service
            .reconcile(&cancellation)
            .await
            .expect("repeat reconciliation must succeed");
        assert_eq!(second.people_considered, 1);
        assert_eq!(second.created_items, 0);
        assert_eq!(second.updated_items, 0);
        assert_eq!(second.copied_images, 0);
        let repeated = items.get(canonical.id).await.unwrap().unwrap();
        assert_eq!(repeated.row_version, canonical.row_version);
        assert!(repeated.row_version > original_row_version);

        std::fs::remove_dir_all(storage_root)
            .expect("temporary person metadata directory must be removable");
    }

    async fn exercise_cancellation(database_name: String) {
        let database = test_database(&database_name).await;
        let storage_root = std::env::temp_dir().join(format!(
            "jellyfin-person-cancellation-{}",
            Uuid::new_v4().simple()
        ));
        let metadata_root = storage_root.join("metadata");
        let items = BaseItemRepository::new(std::sync::Arc::clone(&database));
        let people = PersonRepository::new(std::sync::Arc::clone(&database));
        let media_id = create_item(&items, "Movie", "Cancellation credit owner").await;
        for index in 0..129 {
            people
                .link(
                    media_id,
                    NewPerson::new(format!("Batch Person {index:03}")),
                    "Actor",
                    Some(&format!("Role {index:03}")),
                    Some(index),
                    index,
                )
                .await
                .expect("person credit must persist");
        }

        let service = PersonReconciliationService::new(std::sync::Arc::clone(&database));
        service.set_item_by_name_directories(&storage_root, &metadata_root);
        let cancellation = AtomicBool::new(false);
        let first = service
            .reconcile_with_observer(&cancellation, |_| {
                cancellation.store(true, Ordering::Release);
            })
            .await
            .expect("cancelled reconciliation must keep its committed batch");
        assert!(first.cancelled);
        assert_eq!(
            first.people_considered,
            PERSON_RECONCILIATION_BATCH_SIZE as u64
        );
        assert_eq!(first.batches_committed, 1);
        assert_eq!(first.created_items, PERSON_RECONCILIATION_BATCH_SIZE as u64);
        assert_eq!(person_item_count(&database).await, 128);

        cancellation.store(false, Ordering::Release);
        let resumed = service
            .reconcile(&cancellation)
            .await
            .expect("resumed reconciliation must succeed");
        assert!(!resumed.cancelled);
        assert_eq!(resumed.people_considered, 129);
        assert_eq!(resumed.batches_committed, 2);
        assert_eq!(resumed.created_items, 1);
        assert_eq!(person_item_count(&database).await, 129);

        std::fs::remove_dir_all(storage_root)
            .expect("temporary person metadata directory must be removable");
    }

    async fn create_item(items: &BaseItemRepository, item_type: &str, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        let mut item = NewBaseItem::new(id, item_type);
        item.name = Some(name.to_owned());
        item.sort_name = Some(name.to_owned());
        items.create(item).await.expect("base item must persist");
        id
    }

    async fn create_legacy_person(
        items: &BaseItemRepository,
        name: &str,
        data: serde_json::Value,
        overview: Option<&str>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let mut item = NewBaseItem::new(id, "Person");
        item.name = Some(name.to_owned());
        item.sort_name = Some(name.to_owned());
        item.data = Some(data);
        item.overview = overview.map(str::to_owned);
        item.is_virtual_item = true;
        items
            .create(item)
            .await
            .expect("legacy person must persist");
        id
    }

    async fn person_item_count(database: &jellyfin_data::SharedDatabase) -> u64 {
        base_item::Entity::find()
            .filter(base_item::Column::ItemType.eq("Person"))
            .count(database.as_ref())
            .await
            .expect("person item count must succeed")
    }
}
