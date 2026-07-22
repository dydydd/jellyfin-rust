use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"
                CREATE TABLE IF NOT EXISTS jellyfin.user_profile_images (
                    user_id uuid PRIMARY KEY,
                    path varchar(512) NOT NULL,
                    last_modified timestamptz NOT NULL,
                    CONSTRAINT user_profile_images_path_not_blank
                        CHECK (path !~ '^[[:space:]]*$'),
                    CONSTRAINT user_profile_images_user_id_fkey
                        FOREIGN KEY (user_id)
                        REFERENCES jellyfin.users (id)
                        ON DELETE CASCADE
                );
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.user_profile_images CASCADE;")
            .await?;
        Ok(())
    }
}
