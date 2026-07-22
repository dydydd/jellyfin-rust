use sea_orm_migration::prelude::*;

mod m20260722_000001_create_users;
mod m20260722_000002_create_activity_logs;
mod m20260722_000003_create_authentication;
mod m20260722_000004_create_user_data;
mod m20260722_000005_create_base_items;
mod m20260722_000006_create_item_values;
mod m20260722_000007_create_virtual_folders;
mod m20260722_000008_create_people;
mod m20260722_000009_optimize_item_queries;
mod m20260722_000010_create_quick_connect;
mod m20260722_000011_normalize_base_item_search;
mod m20260722_000012_add_alternate_item_versions;
mod m20260722_000013_optimize_version_playback;
mod m20260722_000014_create_tuner_hosts;
mod m20260722_000015_add_user_policy_providers;
mod m20260722_000016_create_server_configuration;
mod m20260722_000017_add_content_type_overrides;

#[doc(hidden)]
pub use m20260722_000001_create_users::Migration as CreateUsersMigration;
#[doc(hidden)]
pub use m20260722_000002_create_activity_logs::Migration as CreateActivityLogsMigration;
#[doc(hidden)]
pub use m20260722_000003_create_authentication::Migration as CreateAuthenticationMigration;
#[doc(hidden)]
pub use m20260722_000004_create_user_data::Migration as CreateUserDataMigration;
#[doc(hidden)]
pub use m20260722_000005_create_base_items::Migration as CreateBaseItemsMigration;
#[doc(hidden)]
pub use m20260722_000006_create_item_values::Migration as CreateItemValuesMigration;
#[doc(hidden)]
pub use m20260722_000007_create_virtual_folders::Migration as CreateVirtualFoldersMigration;
#[doc(hidden)]
pub use m20260722_000008_create_people::Migration as CreatePeopleMigration;
#[doc(hidden)]
pub use m20260722_000009_optimize_item_queries::Migration as OptimizeItemQueriesMigration;
#[doc(hidden)]
pub use m20260722_000010_create_quick_connect::Migration as CreateQuickConnectMigration;
#[doc(hidden)]
pub use m20260722_000011_normalize_base_item_search::Migration as NormalizeBaseItemSearchMigration;
#[doc(hidden)]
pub use m20260722_000012_add_alternate_item_versions::Migration as AddAlternateItemVersionsMigration;
#[doc(hidden)]
pub use m20260722_000013_optimize_version_playback::Migration as OptimizeVersionPlaybackMigration;
#[doc(hidden)]
pub use m20260722_000014_create_tuner_hosts::Migration as CreateTunerHostsMigration;
#[doc(hidden)]
pub use m20260722_000015_add_user_policy_providers::Migration as AddUserPolicyProvidersMigration;
#[doc(hidden)]
pub use m20260722_000016_create_server_configuration::Migration as CreateServerConfigurationMigration;
#[doc(hidden)]
pub use m20260722_000017_add_content_type_overrides::Migration as AddContentTypeOverridesMigration;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260722_000001_create_users::Migration),
            Box::new(m20260722_000002_create_activity_logs::Migration),
            Box::new(m20260722_000003_create_authentication::Migration),
            Box::new(m20260722_000004_create_user_data::Migration),
            Box::new(m20260722_000005_create_base_items::Migration),
            Box::new(m20260722_000006_create_item_values::Migration),
            Box::new(m20260722_000007_create_virtual_folders::Migration),
            Box::new(m20260722_000008_create_people::Migration),
            Box::new(m20260722_000009_optimize_item_queries::Migration),
            Box::new(m20260722_000010_create_quick_connect::Migration),
            Box::new(m20260722_000011_normalize_base_item_search::Migration),
            Box::new(m20260722_000012_add_alternate_item_versions::Migration),
            Box::new(m20260722_000013_optimize_version_playback::Migration),
            Box::new(m20260722_000014_create_tuner_hosts::Migration),
            Box::new(m20260722_000015_add_user_policy_providers::Migration),
            Box::new(m20260722_000016_create_server_configuration::Migration),
            Box::new(m20260722_000017_add_content_type_overrides::Migration),
        ]
    }
}
