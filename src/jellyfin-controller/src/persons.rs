use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use jellyfin_data::{
    BaseItemCounts, BaseItemError, BaseItemQuery, BaseItemRepository, CanonicalPersonEntity,
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
    pub model: base_item::Model,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonDetail {
    pub item: base_item::Model,
    pub counts: BaseItemCounts,
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
    ItemByName(#[from] ItemByNameError),
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

/// Read-only coverage of referenced people by exact canonical Person items.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PersonCanonicalCoverageSummary {
    pub people_considered: u64,
    pub batches_checked: u64,
    pub present_items: u64,
    pub missing_items: u64,
    pub cancelled: bool,
}

impl PersonCanonicalCoverageSummary {
    #[must_use]
    pub const fn is_complete(self) -> bool {
        !self.cancelled && self.missing_items == 0
    }
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
        let mut after = None::<(String, Uuid)>;
        loop {
            if cancellation.load(Ordering::Acquire) {
                summary.cancelled = true;
                break;
            }
            let people = self
                .people
                .referenced_page_after(
                    after
                        .as_ref()
                        .map(|(clean_name, id)| (clean_name.as_str(), *id)),
                    PERSON_RECONCILIATION_BATCH_SIZE,
                )
                .await?;
            let Some(last_person) = people.last() else {
                break;
            };
            let next_after = (last_person.clean_name.clone(), last_person.id);
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
            after = Some(next_after);
            if page_len < PERSON_RECONCILIATION_BATCH_SIZE {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(summary)
    }

    /// Checks every referenced person against its exact deterministic item id.
    ///
    /// The verifier is bounded and read-only: each page performs one
    /// configuration read and one batch item query, without creating metadata
    /// directories or falling back to normalized-name matches. Cancellation is
    /// checked before every page.
    ///
    /// # Errors
    ///
    /// Returns configuration or persistence errors.
    pub async fn verify_canonical_coverage(
        &self,
        cancellation: &AtomicBool,
    ) -> Result<PersonCanonicalCoverageSummary, PersonReconciliationError> {
        self.verify_canonical_coverage_with_observer(cancellation, |_| {})
            .await
    }

    async fn verify_canonical_coverage_with_observer(
        &self,
        cancellation: &AtomicBool,
        mut on_batch: impl FnMut(PersonCanonicalCoverageSummary),
    ) -> Result<PersonCanonicalCoverageSummary, PersonReconciliationError> {
        let mut summary = PersonCanonicalCoverageSummary::default();
        let mut after = None::<(String, Uuid)>;
        loop {
            if cancellation.load(Ordering::Acquire) {
                summary.cancelled = true;
                break;
            }
            let people = self
                .people
                .referenced_page_after(
                    after
                        .as_ref()
                        .map(|(clean_name, id)| (clean_name.as_str(), *id)),
                    PERSON_RECONCILIATION_BATCH_SIZE,
                )
                .await?;
            let Some(last_person) = people.last() else {
                break;
            };
            let next_after = (last_person.clean_name.clone(), last_person.id);
            let page_len = people.len();
            let names = people
                .into_iter()
                .map(|person| person.name)
                .collect::<Vec<_>>();
            let lookups = self
                .item_by_name
                .existing_canonical_many_direct(ItemByNameKind::Person, &names)
                .await?;
            let present = lookups
                .iter()
                .filter(|lookup| lookup.item.is_some())
                .count();
            summary.people_considered = summary
                .people_considered
                .saturating_add(u64::try_from(page_len).unwrap_or(u64::MAX));
            summary.batches_checked = summary.batches_checked.saturating_add(1);
            summary.present_items = summary
                .present_items
                .saturating_add(u64::try_from(present).unwrap_or(u64::MAX));
            summary.missing_items = summary.missing_items.saturating_add(
                u64::try_from(page_len.saturating_sub(present)).unwrap_or(u64::MAX),
            );
            on_batch(summary);
            after = Some(next_after);
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
    people: PersonRepository,
    items: BaseItemRepository,
    user_library: UserLibraryService,
    item_by_name: ItemByNameService,
}

impl PersonService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        let item_by_name = ItemByNameService::new(std::sync::Arc::clone(&database));
        Self::with_item_by_name_service(database, item_by_name)
    }

    #[must_use]
    pub fn with_item_by_name_service(
        database: impl Into<jellyfin_data::SharedDatabase>,
        item_by_name: ItemByNameService,
    ) -> Self {
        let database = database.into();
        Self {
            users: UserService::new(std::sync::Arc::clone(&database)),
            people: PersonRepository::new(std::sync::Arc::clone(&database)),
            items: BaseItemRepository::new(std::sync::Arc::clone(&database)),
            user_library: UserLibraryService::new(database),
            item_by_name,
        }
    }

    pub fn set_item_by_name_directories(
        &self,
        program_data_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) {
        self.item_by_name
            .set_directories(program_data_directory, internal_metadata_directory);
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
    ) -> Result<PersonDetail, PersonError> {
        self.validate_user(authenticated_user, target_user_id)
            .await?;
        let person = self.catalog_person(name).await?;
        let canonical = self
            .canonical_items(std::slice::from_ref(&person))
            .await?
            .pop()
            .flatten()
            .ok_or(PersonError::NotFound)?;
        let mut query = BaseItemQuery {
            person_ids: vec![canonical.id],
            include_item_types: [
                "Audio",
                "AudioBook",
                "Book",
                "Episode",
                "Movie",
                "MusicAlbum",
                "MusicArtist",
                "MusicVideo",
                "Series",
                "Trailer",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            ..BaseItemQuery::default()
        };
        self.user_library
            .apply_user_policy(&mut query, target_user_id)
            .await?;
        let counts = self.items.item_counts(&query).await?;
        Ok(PersonDetail {
            item: canonical,
            counts,
        })
    }

    /// Resolves the persisted `Person` item that owns image metadata.
    ///
    /// # Errors
    ///
    /// Returns a database error when the item lookup fails.
    pub async fn image_item(&self, name: &str) -> Result<Option<base_item::Model>, PersonError> {
        let person = match self.catalog_person(name).await {
            Ok(person) => person,
            Err(PersonError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(self
            .canonical_items(std::slice::from_ref(&person))
            .await?
            .pop()
            .flatten())
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
        if let Some(is_favorite) = query.is_favorite.take() {
            query.canonical_favorite_names = Some(
                self.canonical_favorite_names(target_user_id, is_favorite)
                    .await?,
            );
        }
        let page = self.people.query(&query).await?;
        let canonical = self.canonical_items(&page.people).await?;
        Ok(PersonPage {
            people: page
                .people
                .into_iter()
                .zip(canonical)
                .filter_map(|(_, item)| item.map(|model| Person { model }))
                .collect(),
            total_record_count: page.total_record_count,
            start_index: page.start_index,
        })
    }

    /// Resolves exact canonical Person items for internal catalog rows.
    ///
    /// The result remains aligned with `people`; a missing canonical row is
    /// represented by `None` and never falls back to the internal credit id.
    ///
    /// # Errors
    ///
    /// Returns configuration or persistence errors.
    pub async fn canonical_items(
        &self,
        people: &[person::Model],
    ) -> Result<Vec<Option<base_item::Model>>, PersonError> {
        let names = people
            .iter()
            .map(|person| person.name.clone())
            .collect::<Vec<_>>();
        Ok(self
            .item_by_name
            .existing_canonical_many_direct(ItemByNameKind::Person, &names)
            .await?
            .into_iter()
            .map(|lookup| lookup.item)
            .collect())
    }

    async fn canonical_favorite_names(
        &self,
        target_user_id: Uuid,
        is_favorite: bool,
    ) -> Result<Vec<String>, PersonError> {
        let candidates = self
            .people
            .user_data_person_items(target_user_id, is_favorite)
            .await?;
        let names = candidates
            .iter()
            .map(|item| item.name.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        let lookups = self
            .item_by_name
            .existing_canonical_many_direct(ItemByNameKind::Person, &names)
            .await?;
        let mut seen = HashSet::new();
        Ok(candidates
            .into_iter()
            .zip(names)
            .zip(lookups)
            .filter_map(|((candidate, name), lookup)| {
                (!name.is_empty()
                    && candidate.id == lookup.expected_id
                    && lookup.item.is_some()
                    && seen.insert(name.clone()))
                .then_some(name)
            })
            .collect())
    }

    async fn catalog_person(&self, name: &str) -> Result<person::Model, PersonError> {
        match self.people.get_exact(name).await {
            Ok(Some(person)) => Ok(person),
            Ok(None) => match self.people.get_normalized(name).await {
                Ok(Some(person)) => Ok(person),
                Ok(None) | Err(PersonRepositoryError::InvalidName) => Err(PersonError::NotFound),
                Err(error) => Err(error.into()),
            },
            Err(PersonRepositoryError::InvalidName) => Err(PersonError::NotFound),
            Err(error) => Err(error.into()),
        }
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
    use std::{collections::HashSet, future::Future, sync::atomic::AtomicBool};

    use chrono::Utc;
    use jellyfin_data::{
        BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
        NewBaseItem, NewBaseItemImage, NewPerson, PersonRepository, entities::base_item,
    };
    use sea_orm::{
        ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Statement,
    };
    use serde_json::json;

    use super::*;

    const DATABASE_PREFIX: &str = "jellyfin_person_reconciliation_";

    #[test]
    fn canonical_coverage_is_complete_only_after_an_uncancelled_full_check() {
        assert!(PersonCanonicalCoverageSummary::default().is_complete());
        assert!(
            !PersonCanonicalCoverageSummary {
                missing_items: 1,
                ..PersonCanonicalCoverageSummary::default()
            }
            .is_complete()
        );
        assert!(
            !PersonCanonicalCoverageSummary {
                cancelled: true,
                ..PersonCanonicalCoverageSummary::default()
            }
            .is_complete()
        );
    }

    #[tokio::test]
    async fn reconciliation_merges_legacy_data_and_images_without_overwriting_or_deleting() {
        run_database_test(exercise_legacy_merge).await;
    }

    #[tokio::test]
    async fn cancellation_stops_before_the_next_fixed_keyset_batch() {
        run_database_test(exercise_cancellation).await;
    }

    #[tokio::test]
    async fn exact_coverage_ignores_legacy_name_matches_without_writing_directories() {
        run_database_test(exercise_exact_coverage).await;
    }

    #[tokio::test]
    async fn referenced_people_keyset_keeps_equal_clean_names_at_a_page_boundary() {
        run_database_test(exercise_equal_clean_name_keyset).await;
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

        cancellation.store(false, Ordering::Release);
        let partial_coverage = service
            .verify_canonical_coverage_with_observer(&cancellation, |_| {
                cancellation.store(true, Ordering::Release);
            })
            .await
            .expect("cancelled coverage verification must preserve its completed batch");
        assert!(partial_coverage.cancelled);
        assert_eq!(
            partial_coverage.people_considered,
            PERSON_RECONCILIATION_BATCH_SIZE as u64
        );
        assert_eq!(partial_coverage.batches_checked, 1);
        assert_eq!(partial_coverage.present_items, 128);
        assert_eq!(partial_coverage.missing_items, 0);

        cancellation.store(false, Ordering::Release);
        let full_coverage = service
            .verify_canonical_coverage(&cancellation)
            .await
            .expect("resumed coverage verification must succeed");
        assert!(full_coverage.is_complete());
        assert_eq!(full_coverage.people_considered, 129);
        assert_eq!(full_coverage.batches_checked, 2);
        assert_eq!(full_coverage.present_items, 129);

        std::fs::remove_dir_all(storage_root)
            .expect("temporary person metadata directory must be removable");
    }

    async fn exercise_exact_coverage(database_name: String) {
        let database = test_database(&database_name).await;
        let storage_root = std::env::temp_dir().join(format!(
            "jellyfin-person-exact-coverage-{}",
            Uuid::new_v4().simple()
        ));
        let metadata_root = storage_root.join("metadata");
        let items = BaseItemRepository::new(std::sync::Arc::clone(&database));
        let people = PersonRepository::new(std::sync::Arc::clone(&database));
        let media_id = create_item(&items, "Movie", "Exact coverage owner").await;
        let name = "--Élodie/Actor.";
        people
            .link(media_id, NewPerson::new(name), "Actor", None, Some(0), 0)
            .await
            .expect("person credit must persist");
        let legacy_id = create_legacy_person(&items, name, json!({}), None).await;
        let service = PersonReconciliationService::new(std::sync::Arc::clone(&database));
        service.set_item_by_name_directories(&storage_root, &metadata_root);

        let names = vec![name.to_owned(), name.to_owned()];
        let missing = service
            .item_by_name
            .existing_canonical_many_direct(ItemByNameKind::Person, &names)
            .await
            .expect("exact missing lookup must succeed");
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].expected_id, missing[1].expected_id);
        assert_ne!(missing[0].expected_id, legacy_id);
        assert!(missing.iter().all(|lookup| lookup.item.is_none()));
        assert!(!storage_root.exists());

        let cancellation = AtomicBool::new(false);
        let before = service
            .verify_canonical_coverage(&cancellation)
            .await
            .expect("missing coverage verification must succeed");
        assert_eq!(before.people_considered, 1);
        assert_eq!(before.present_items, 0);
        assert_eq!(before.missing_items, 1);
        assert!(!before.is_complete());
        assert!(!storage_root.exists());

        let canonical = service
            .item_by_name
            .resolve_direct(ItemByNameKind::Person, name)
            .await
            .expect("canonical fixture must be creatable");
        assert_eq!(canonical.id, missing[0].expected_id);
        std::fs::remove_dir_all(&storage_root)
            .expect("canonical fixture directory must be removable");

        let exact = service
            .item_by_name
            .existing_canonical_many_direct(ItemByNameKind::Person, &names)
            .await
            .expect("exact canonical lookup must succeed");
        assert!(
            exact
                .iter()
                .all(|lookup| lookup.item.as_ref().map(|item| item.id) == Some(canonical.id))
        );
        assert!(!storage_root.exists());
        let after = service
            .verify_canonical_coverage(&cancellation)
            .await
            .expect("complete coverage verification must succeed");
        assert!(after.is_complete());
        assert_eq!(after.present_items, 1);
        assert!(!storage_root.exists());
    }

    async fn exercise_equal_clean_name_keyset(database_name: String) {
        let database = test_database(&database_name).await;
        let items = BaseItemRepository::new(std::sync::Arc::clone(&database));
        let people = PersonRepository::new(std::sync::Arc::clone(&database));
        let media_id = create_item(&items, "Movie", "Equal clean-name owner").await;
        let seed = Uuid::new_v4().simple().to_string();
        database
            .execute_unprepared("DROP INDEX jellyfin.people_clean_name_key")
            .await
            .expect("temporary legacy duplicate fixture must allow equal clean names");
        database
            .execute(Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                r"
                WITH inserted AS (
                    INSERT INTO jellyfin.people (id, name, clean_name)
                    SELECT md5($2::text || value::text)::uuid,
                           'Boundary Person ' || value::text,
                           'equal boundary clean name'
                    FROM generate_series(1, 129) AS value
                    RETURNING id
                )
                INSERT INTO jellyfin.people_base_item_map (
                    item_id, person_id, person_type, role, sort_order, list_order
                )
                SELECT $1, id, 'Actor', '', NULL,
                       (row_number() OVER (ORDER BY id) - 1)::integer
                FROM inserted
                ",
                [media_id.into(), seed.into()],
            ))
            .await
            .expect("equal clean-name people fixture must persist");

        let first = people
            .referenced_page_after(None, PERSON_RECONCILIATION_BATCH_SIZE)
            .await
            .expect("first keyset page must load");
        assert_eq!(first.len(), PERSON_RECONCILIATION_BATCH_SIZE);
        let first_ids = first.iter().map(|person| person.id).collect::<HashSet<_>>();
        let cursor = first.last().expect("first page cursor");
        let second = people
            .referenced_page_after(
                Some((cursor.clean_name.as_str(), cursor.id)),
                PERSON_RECONCILIATION_BATCH_SIZE,
            )
            .await
            .expect("second keyset page must load");
        assert_eq!(second.len(), 1);
        assert!(!first_ids.contains(&second[0].id));
        assert_eq!(second[0].clean_name, cursor.clean_name);
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
