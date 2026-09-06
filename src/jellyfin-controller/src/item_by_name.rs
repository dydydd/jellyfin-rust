use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use jellyfin_data::{
    BaseItemError, BaseItemRepository, ItemByNameRepository, ItemByNameStoreError, ItemValueError,
    ItemValueRepository, NewItemByNameEntity, ServerConfigurationRepository,
    ServerConfigurationStoreError, entities::base_item,
};
use jellyfin_extensions::StringExtensions;
use md5::{Digest, Md5};
use thiserror::Error;
use tokio::sync::OnceCell;
use uuid::Uuid;

const RECONCILIATION_BATCH_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemByNameKind {
    Genre,
    MusicGenre,
    Studio,
    Year,
}

impl ItemByNameKind {
    #[must_use]
    pub const fn item_type(self) -> &'static str {
        match self {
            Self::Genre => "Genre",
            Self::MusicGenre => "MusicGenre",
            Self::Studio => "Studio",
            Self::Year => "Year",
        }
    }

    const fn clr_type(self) -> &'static str {
        match self {
            Self::Genre => "MediaBrowser.Controller.Entities.Genre",
            Self::MusicGenre => "MediaBrowser.Controller.Entities.Audio.MusicGenre",
            Self::Studio => "MediaBrowser.Controller.Entities.Studio",
            Self::Year => "MediaBrowser.Controller.Entities.Year",
        }
    }
}

#[derive(Debug, Error)]
pub enum ItemByNameError {
    #[error(transparent)]
    Store(#[from] ItemByNameStoreError),
    #[error(transparent)]
    Configuration(#[from] ServerConfigurationStoreError),
    #[error(transparent)]
    BaseItem(#[from] BaseItemError),
    #[error(transparent)]
    ItemValue(#[from] ItemValueError),
    #[error("item-by-name directory operation failed")]
    FileSystem(#[from] std::io::Error),
}

/// Resolves and creates persisted Genre and MusicGenre entities with the
/// official Jellyfin item-by-name path and identifier rules.
#[derive(Clone)]
pub struct ItemByNameService {
    items: ItemByNameRepository,
    base_items: BaseItemRepository,
    values: ItemValueRepository,
    configuration: ServerConfigurationRepository,
    directories: Arc<RwLock<ItemByNameDirectories>>,
    reconciled: Arc<OnceCell<()>>,
    studio_year_reconciled: Arc<OnceCell<()>>,
}

#[derive(Debug)]
struct ItemByNameDirectories {
    program_data: Arc<PathBuf>,
    internal_metadata: Arc<PathBuf>,
}

impl ItemByNameService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        let database = database.into();
        Self {
            items: ItemByNameRepository::new(Arc::clone(&database)),
            base_items: BaseItemRepository::new(Arc::clone(&database)),
            values: ItemValueRepository::new(Arc::clone(&database)),
            configuration: ServerConfigurationRepository::new(database),
            directories: Arc::new(RwLock::new(ItemByNameDirectories {
                program_data: Arc::new(PathBuf::from("programdata")),
                internal_metadata: Arc::new(PathBuf::from("metadata")),
            })),
            reconciled: Arc::new(OnceCell::new()),
            studio_year_reconciled: Arc::new(OnceCell::new()),
        }
    }

    /// Replaces the roots used for persisted item-by-name directories and ids.
    ///
    /// # Panics
    ///
    /// Panics when the path lock has been poisoned.
    pub fn set_directories(
        &self,
        program_data_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) {
        *self
            .directories
            .write()
            .expect("item-by-name directory lock poisoned") = ItemByNameDirectories {
            program_data: Arc::new(program_data_directory.into()),
            internal_metadata: Arc::new(internal_metadata_directory.into()),
        };
    }

    /// Resolves the official slug form from persisted entities only, or
    /// creates the deterministic entity for an ordinary name.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn resolve(
        &self,
        kind: ItemByNameKind,
        name: &str,
    ) -> Result<Option<base_item::Model>, ItemByNameError> {
        if name.contains('-') {
            for separator in ['&', '/', '?'] {
                let candidate = name.replace('-', &separator.to_string());
                if let Some(item) = self.items.get_by_name(kind.item_type(), &candidate).await? {
                    return Ok(Some(hydrate_item_type(item, kind)));
                }
            }
            return Ok(None);
        }

        self.get_or_create(kind, name).await.map(Some)
    }

    /// Resolves or creates one direct item-by-name entity, including names
    /// containing the Genre slug separator.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn resolve_direct(
        &self,
        kind: ItemByNameKind,
        name: &str,
    ) -> Result<base_item::Model, ItemByNameError> {
        self.get_or_create(kind, name).await
    }

    /// Loads existing persisted entities for a bounded set of names.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the lookup fails.
    pub async fn existing_many(
        &self,
        kind: ItemByNameKind,
        names: &[String],
    ) -> Result<HashMap<String, base_item::Model>, ItemByNameError> {
        let mut result = HashMap::new();
        for item in self.items.get_by_names(kind.item_type(), names).await? {
            if let Some(name) = item.name.clone() {
                result
                    .entry(name)
                    .or_insert_with(|| hydrate_item_type(item, kind));
            }
        }
        Ok(result)
    }

    /// Ensures a bounded set of direct-name entities with one existence read
    /// and one batched insert, then returns the persisted representatives.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn ensure_many_direct(
        &self,
        kind: ItemByNameKind,
        names: &[String],
    ) -> Result<HashMap<String, base_item::Model>, ItemByNameError> {
        let existing = self.existing_many(kind, names).await?;
        let existing_clean_names = existing
            .keys()
            .map(|name| name.clean_value())
            .collect::<std::collections::HashSet<_>>();
        let missing = names
            .iter()
            .filter(|name| !existing_clean_names.contains(&name.clean_value()))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let configuration = self.configuration.load().await?;
            let (program_data, internal_metadata) = self.directories();
            let entities = self
                .entities_for_names(
                    kind,
                    missing,
                    &program_data,
                    &internal_metadata,
                    &configuration,
                )
                .await?;
            self.base_items
                .create_missing_item_by_name_entities(&entities)
                .await?;
        }
        self.existing_many(kind, names).await
    }

    /// Backfills persisted genre entities once for this server process.
    ///
    /// The source values are keyset-paged, directories are prepared in bounded
    /// batches, and each page is inserted with one set-based statement. A
    /// successful run is shared by every clone; a failed run remains retryable.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn reconcile_once(&self) -> Result<(), ItemByNameError> {
        self.reconciled
            .get_or_try_init(|| async { self.reconcile().await })
            .await?;
        Ok(())
    }

    /// Backfills Studio and Year entities once for this server process.
    ///
    /// # Errors
    ///
    /// Returns configuration, filesystem, or persistence errors.
    pub async fn reconcile_studios_and_years_once(&self) -> Result<(), ItemByNameError> {
        self.studio_year_reconciled
            .get_or_try_init(|| async { self.reconcile_studios_and_years().await })
            .await?;
        Ok(())
    }

    async fn reconcile_studios_and_years(&self) -> Result<(), ItemByNameError> {
        let configuration = self.configuration.load().await?;
        let (program_data, internal_metadata) = self.directories();

        let mut start_index = 0_u64;
        loop {
            let page = self
                .values
                .query_values(
                    jellyfin_data::entities::item_value::ItemValueType::Studios,
                    &jellyfin_data::ItemValueQuery {
                        start_index,
                        limit: Some(RECONCILIATION_BATCH_SIZE as u64),
                        enable_total_record_count: Some(false),
                        ..jellyfin_data::ItemValueQuery::default()
                    },
                )
                .await?;
            if page.values.is_empty() {
                break;
            }
            let count = page.values.len();
            let entities = self
                .entities_for_names(
                    ItemByNameKind::Studio,
                    page.values.into_iter().map(|value| value.value),
                    &program_data,
                    &internal_metadata,
                    &configuration,
                )
                .await?;
            self.base_items
                .create_missing_item_by_name_entities(&entities)
                .await?;
            start_index = start_index.saturating_add(count as u64);
            if count < RECONCILIATION_BATCH_SIZE {
                break;
            }
        }

        let mut start_index = 0_u64;
        loop {
            let page = self
                .base_items
                .production_years(
                    &jellyfin_data::BaseItemQuery {
                        start_index,
                        limit: Some(RECONCILIATION_BATCH_SIZE as u64),
                        ..jellyfin_data::BaseItemQuery::default()
                    },
                    jellyfin_data::ProductionYearOrder::Ascending,
                )
                .await?;
            if page.years.is_empty() {
                break;
            }
            let count = page.years.len();
            let entities = self
                .entities_for_names(
                    ItemByNameKind::Year,
                    page.years.into_iter().map(|year| year.to_string()),
                    &program_data,
                    &internal_metadata,
                    &configuration,
                )
                .await?;
            self.base_items
                .create_missing_item_by_name_entities(&entities)
                .await?;
            start_index = start_index.saturating_add(count as u64);
            if count < RECONCILIATION_BATCH_SIZE {
                break;
            }
        }
        Ok(())
    }

    async fn entities_for_names(
        &self,
        kind: ItemByNameKind,
        names: impl IntoIterator<Item = String>,
        program_data: &Path,
        internal_metadata: &Path,
        configuration: &jellyfin_data::entities::server_configuration::Model,
    ) -> Result<Vec<NewItemByNameEntity>, ItemByNameError> {
        let mut entities = Vec::new();
        for name in names {
            let path = internal_metadata
                .join(kind.item_type())
                .join(item_by_name_folder_name(&name));
            tokio::fs::create_dir_all(&path).await?;
            let metadata = tokio::fs::metadata(&path).await?;
            let modified = metadata.modified()?;
            let created = metadata.created().unwrap_or(modified);
            entities.push(NewItemByNameEntity {
                id: official_item_by_name_id(
                    &path,
                    program_data,
                    kind.clr_type(),
                    configuration.enable_normalized_item_by_name_ids,
                    configuration.enable_case_sensitive_item_ids,
                ),
                item_type: kind.item_type().to_owned(),
                name: name.clone(),
                path: path.to_string_lossy().into_owned(),
                presentation_unique_key: format!(
                    "{}-{}",
                    kind.item_type(),
                    name.remove_diacritics()
                ),
                date_created: chrono::DateTime::<chrono::Utc>::from(created),
                date_modified: chrono::DateTime::<chrono::Utc>::from(modified),
            });
        }
        Ok(entities)
    }

    async fn reconcile(&self) -> Result<(), ItemByNameError> {
        let configuration = self.configuration.load().await?;
        let (program_data, internal_metadata) = self.directories();
        let mut after = None;
        loop {
            let required = self
                .values
                .required_genre_entities_page(after.as_ref(), RECONCILIATION_BATCH_SIZE)
                .await?;
            let Some(next_after) = required.last().cloned() else {
                break;
            };
            let mut entities = Vec::with_capacity(required.len());
            for required in required {
                let kind = match required.item_type.as_str() {
                    "Genre" => ItemByNameKind::Genre,
                    "MusicGenre" => ItemByNameKind::MusicGenre,
                    _ => continue,
                };
                let path = internal_metadata
                    .join(kind.item_type())
                    .join(item_by_name_folder_name(&required.name));
                tokio::fs::create_dir_all(&path).await?;
                let metadata = tokio::fs::metadata(&path).await?;
                let modified = metadata.modified()?;
                let created = metadata.created().unwrap_or(modified);
                entities.push(NewItemByNameEntity {
                    id: official_item_by_name_id(
                        &path,
                        &program_data,
                        kind.clr_type(),
                        configuration.enable_normalized_item_by_name_ids,
                        configuration.enable_case_sensitive_item_ids,
                    ),
                    item_type: required.item_type,
                    name: required.name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    presentation_unique_key: format!(
                        "{}-{}",
                        kind.item_type(),
                        required.name.remove_diacritics()
                    ),
                    date_created: chrono::DateTime::<chrono::Utc>::from(created),
                    date_modified: chrono::DateTime::<chrono::Utc>::from(modified),
                });
            }
            self.base_items
                .create_missing_item_by_name_entities(&entities)
                .await?;
            after = Some(next_after);
        }
        Ok(())
    }

    async fn get_or_create(
        &self,
        kind: ItemByNameKind,
        name: &str,
    ) -> Result<base_item::Model, ItemByNameError> {
        let configuration = self.configuration.load().await?;
        let (program_data, internal_metadata) = self.directories();
        let path = internal_metadata
            .join(kind.item_type())
            .join(item_by_name_folder_name(name));
        let id = official_item_by_name_id(
            &path,
            &program_data,
            kind.clr_type(),
            configuration.enable_normalized_item_by_name_ids,
            configuration.enable_case_sensitive_item_ids,
        );
        if let Some(item) = self.items.get(id, kind.item_type()).await? {
            return Ok(hydrate_item_type(item, kind));
        }

        tokio::fs::create_dir_all(&path).await?;
        let metadata = tokio::fs::metadata(&path).await?;
        let modified = metadata.modified()?;
        let created = metadata.created().unwrap_or(modified);
        let item = self
            .items
            .ensure(&NewItemByNameEntity {
                id,
                item_type: kind.item_type().to_owned(),
                name: name.to_owned(),
                path: path.to_string_lossy().into_owned(),
                presentation_unique_key: format!(
                    "{}-{}",
                    kind.item_type(),
                    name.remove_diacritics()
                ),
                date_created: chrono::DateTime::<chrono::Utc>::from(created),
                date_modified: chrono::DateTime::<chrono::Utc>::from(modified),
            })
            .await?;
        Ok(hydrate_item_type(item, kind))
    }

    fn directories(&self) -> (Arc<PathBuf>, Arc<PathBuf>) {
        let directories = self
            .directories
            .read()
            .expect("item-by-name directory lock poisoned");
        (
            Arc::clone(&directories.program_data),
            Arc::clone(&directories.internal_metadata),
        )
    }
}

fn hydrate_item_type(mut item: base_item::Model, kind: ItemByNameKind) -> base_item::Model {
    item.item_type = kind.item_type().to_owned();
    item
}

pub(crate) fn item_by_name_folder_name(name: &str) -> String {
    const MAX_BYTES: usize = 128;
    let mut valid_name = name
        .chars()
        .map(|character| {
            if character <= '\u{1f}'
                || matches!(
                    character,
                    '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    valid_name = valid_name.trim().trim_end_matches('.').to_owned();
    if valid_name.len() <= MAX_BYTES {
        return valid_name;
    }

    let suffix = format!("-{}", official_md5_guid(&valid_name).simple());
    let prefix_budget = MAX_BYTES.saturating_sub(suffix.len());
    let prefix_end = valid_name
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(valid_name.len()))
        .take_while(|index| *index <= prefix_budget)
        .last()
        .unwrap_or_default();
    let prefix = valid_name[..prefix_end].trim_end().trim_end_matches('.');
    format!("{prefix}{suffix}")
}

pub(crate) fn official_item_by_name_id(
    path: &Path,
    program_data: &Path,
    clr_type: &str,
    force_case_insensitive: bool,
    enable_case_sensitive_item_ids: bool,
) -> Uuid {
    let path = path.to_string_lossy();
    let program_data = program_data.to_string_lossy();
    let mut path_key = if let Some(relative) = path.strip_prefix(program_data.as_ref()) {
        relative.trim_start_matches(['/', '\\']).replace('/', "\\")
    } else {
        path.into_owned()
    };
    if force_case_insensitive || !enable_case_sensitive_item_ids {
        path_key = path_key.to_lowercase();
    }
    official_md5_guid(&format!("{clr_type}{path_key}"))
}

fn official_md5_guid(value: &str) -> Uuid {
    let utf16_le = value
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let digest = Md5::digest(utf16_le);
    Uuid::from_bytes([
        digest[3], digest[2], digest[1], digest[0], digest[5], digest[4], digest[7], digest[6],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ])
}
