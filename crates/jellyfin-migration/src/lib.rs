use sea_orm_migration::prelude::*;

mod m20260722_000001_create_users;
mod m20260722_000002_create_activity_logs;
mod m20260722_000003_create_authentication;
mod m20260722_000004_create_user_data;
mod m20260722_000005_create_base_items;
mod m20260722_000006_create_item_values;
mod m20260722_000007_create_virtual_folders;

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
        ]
    }
}
