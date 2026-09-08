mod activity_log_repository;
mod authentication_repository;
mod base_item_image_repository;
mod base_item_repository;
mod chapter_repository;
mod collection_repository;
mod display_preference_repository;
pub mod entities;
mod item_by_name_repository;
mod item_types;
mod item_update_repository;
mod item_value_repository;
mod keyframe_data_repository;
mod linked_child_repository;
mod media_attachment_repository;
mod media_segment_repository;
mod media_stream_repository;
mod named_configuration_repository;
mod person_repository;
mod playlist_repository;
mod quick_connect_repository;
mod server_configuration_repository;
mod session_command_repository;
mod trickplay_info_repository;
mod tuner_host_repository;
mod user_data_repository;
mod user_profile_image_repository;
mod virtual_folder_repository;

pub use activity_log_repository::{
    ActivityLogError, ActivityLogPage, ActivityLogQuery, ActivityLogRepository, ActivityLogSortBy,
    NewActivityLog, SortDirection,
};
pub use authentication_repository::{
    ApiKeyRepository, AuthenticationStoreError, DeviceOptionsRepository, DevicePage, DeviceQuery,
    DeviceRepository, NewDevice,
};
pub use base_item_image_repository::{
    BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageSwap,
    BaseItemImageType, InvalidBaseItemImageType, NewBaseItemImage, StoredImageMutation,
};
pub use base_item_repository::{
    BaseItemCounts, BaseItemError, BaseItemHierarchyEntry, BaseItemOrder, BaseItemPage,
    BaseItemQuery, BaseItemRepository, DescendantScanCandidate, LatestTvGroup,
    MediaStreamLanguageLists, MetadataRefreshCandidate, NewBaseItem, NewItemByNameEntity,
    ProductionYearOrder, ProductionYearPage, ScoredBaseItem, ScoredBaseItemPage,
    TvHierarchyCandidate, USER_ROOT_FOLDER_ID,
};
pub use chapter_repository::{ChapterRecord, ChapterRepository, ChapterStoreError, NewChapter};
pub use collection_repository::{CollectionRepository, CollectionStoreError};
pub use display_preference_repository::{DisplayPreferenceRepository, DisplayPreferenceStoreError};
pub use item_by_name_repository::{ItemByNameRepository, ItemByNameStoreError};
pub use item_types::OFFICIAL_ITEM_TYPE_ALIASES;
pub use item_update_repository::{ItemMetadataPatch, ItemUpdateRepository, ItemUpdateStoreError};
pub use item_value_repository::{
    ItemByNameValue, ItemValueCounts, ItemValueError, ItemValueInfo, ItemValueOrder, ItemValuePage,
    ItemValuePair, ItemValueQuery, ItemValueRepository,
};
pub use keyframe_data_repository::{
    KeyframeDataExport, KeyframeDataRecord, KeyframeDataRepository, KeyframeDataStoreError,
    NewKeyframeData,
};
pub use linked_child_repository::{
    LinkedChild, LinkedChildRepository, LinkedChildStoreError, LinkedChildType,
};
pub use media_attachment_repository::{
    MediaAttachmentQuery, MediaAttachmentRepository, MediaAttachmentStoreError,
    PersistedMediaAttachment,
};
pub use media_segment_repository::{
    MediaSegmentRecord, MediaSegmentRepository, MediaSegmentStoreError, NewMediaSegment,
};
pub use media_stream_repository::{
    InvalidPersistedMediaStreamType, MediaStreamQuery, MediaStreamRepository,
    MediaStreamStoreError, PersistedMediaStream, PersistedMediaStreamType,
};
pub use named_configuration_repository::{
    NamedConfigurationRepository, NamedConfigurationStoreError,
};
pub use person_repository::{
    CanonicalPersonEntity, NewPerson, NewPersonCredit, PersonCredit, PersonError, PersonPage,
    PersonQuery, PersonReconciliationBatchResult, PersonRepository,
};
pub use playlist_repository::{
    PlaylistRecord, PlaylistRepository, PlaylistStoreError, PlaylistUserPermission,
};
pub use quick_connect_repository::{
    AuthorizedQuickConnect, NewQuickConnectRequest, QuickConnectRepository, QuickConnectStoreError,
};
pub use server_configuration_repository::{
    ServerConfigurationRepository, ServerConfigurationStoreError, ServerConfigurationUpdate,
    StartupConfigurationUpdate,
};
pub use session_command_repository::{
    NewSessionCommand, SessionCommandRepository, SessionCommandStoreError,
};
pub use trickplay_info_repository::{
    NewTrickplayInfo, TrickplayInfo, TrickplayInfoRepository, TrickplayInfoStoreError,
    TrickplayManifestStore, TrickplayManifestStores,
};
pub use tuner_host_repository::{NewTunerHost, TunerHostRepository, TunerHostStoreError};
pub use user_data_repository::{
    GenericUserDataPatch, NewUserData, PreferredUserDataKey, UserDataError, UserDataPatch,
    UserDataQuery, UserDataRepository,
};
pub use user_profile_image_repository::{
    NewUserProfileImage, UserProfileImageRepository, UserProfileImageStoreError,
};
pub use virtual_folder_repository::{
    NewMediaPath, NewVirtualFolder, VirtualFolderError, VirtualFolderRepository,
    VirtualFolderWithPaths,
};

use std::{sync::Arc, time::Duration};

use jellyfin_migration::Migrator;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, Statement,
    TransactionTrait,
};
use sea_orm_migration::MigratorTrait;

pub const DEFAULT_DATABASE_URL: &str = "postgres://postgres:123456@127.0.0.1:5432/postgres";
const MIGRATION_ADVISORY_LOCK_KEY: i64 = 0x4a45_4c4c_5946_494e;
const DATABASE_MAX_CONNECTIONS_ENV: &str = "JELLYFIN_DATABASE_MAX_CONNECTIONS";
const DATABASE_CONNECTIONS_PER_CPU: u32 = 4;
const DATABASE_MIN_MAX_CONNECTIONS: u32 = 4;
const DATABASE_MAX_MAX_CONNECTIONS: u32 = 32;

pub type SharedDatabase = Arc<DatabaseConnection>;

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned()),
            max_connections: pool_max_connections(
                std::thread::available_parallelism().map_or(1, |count| count.get()),
                std::env::var(DATABASE_MAX_CONNECTIONS_ENV)
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .filter(|&value| value > 0),
            ),
            min_connections: 1,
        }
    }
}

fn pool_max_connections(available_cpus: usize, configured_max: Option<u32>) -> u32 {
    configured_max.unwrap_or_else(|| {
        u32::try_from(available_cpus)
            .unwrap_or(u32::MAX)
            .saturating_mul(DATABASE_CONNECTIONS_PER_CPU)
            .clamp(DATABASE_MIN_MAX_CONNECTIONS, DATABASE_MAX_MAX_CONNECTIONS)
    })
}

/// Creates a `PostgreSQL` connection pool from `config`.
///
/// Every pooled connection receives the `jellyfin-rust` application name as
/// part of its `PostgreSQL` startup parameters.
///
/// # Errors
///
/// Returns a database connection error when the URL is invalid or `PostgreSQL`
/// cannot establish the configured minimum number of connections.
pub async fn connect(config: &DatabaseConfig) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(&config.url);
    options
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .connect_timeout(Duration::from_secs(5))
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(false)
        .map_sqlx_postgres_opts(|options| options.application_name("jellyfin-rust"));

    Database::connect(options).await
}

/// Applies all pending database migrations.
///
/// A transaction-scoped `PostgreSQL` advisory lock serializes migration planning
/// across application instances. This prevents two fresh instances from both
/// observing and applying the same pending migration.
///
/// # Errors
///
/// Returns a database error if the lock cannot be acquired, a migration fails,
/// or the migration transaction cannot be committed.
pub async fn migrate(database: &DatabaseConnection) -> Result<(), DbErr> {
    let transaction = database.begin().await?;
    transaction
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [MIGRATION_ADVISORY_LOCK_KEY.into()],
        ))
        .await?;
    Migrator::up(&transaction, None).await?;
    transaction.commit().await
}

/// Verifies that `PostgreSQL` accepts a simple query.
///
/// # Errors
///
/// Returns a database error when the query fails or unexpectedly returns no
/// row.
pub async fn healthcheck(database: &DatabaseConnection) -> Result<(), DbErr> {
    database
        .query_one(Statement::from_string(
            database.get_database_backend(),
            "SELECT 1".to_owned(),
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("PostgreSQL healthcheck returned no row".to_owned()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pool_max_connections;

    #[test]
    fn pool_size_scales_with_cpu_and_keeps_an_explicit_override() {
        assert_eq!(pool_max_connections(0, None), 4);
        assert_eq!(pool_max_connections(2, None), 8);
        assert_eq!(pool_max_connections(8, None), 32);
        assert_eq!(pool_max_connections(128, None), 32);
        assert_eq!(pool_max_connections(8, Some(48)), 48);
    }
}
