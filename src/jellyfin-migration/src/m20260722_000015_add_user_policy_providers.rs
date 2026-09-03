use sea_orm_migration::prelude::*;

const DEFAULT_AUTHENTICATION_PROVIDER_ID: &str =
    "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider";
const DEFAULT_PASSWORD_RESET_PROVIDER_ID: &str =
    "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = format!(
            r"
            ALTER TABLE jellyfin.users
                ADD COLUMN IF NOT EXISTS authentication_provider_id varchar(255),
                ADD COLUMN IF NOT EXISTS password_reset_provider_id varchar(255);

            UPDATE jellyfin.users
            SET authentication_provider_id = CASE
                    WHEN authentication_provider_id IS NOT NULL
                         AND btrim(authentication_provider_id) <> ''
                         AND char_length(authentication_provider_id) <= 255
                        THEN authentication_provider_id
                    WHEN btrim(policy ->> 'AuthenticationProviderId') <> ''
                         AND char_length(policy ->> 'AuthenticationProviderId') <= 255
                        THEN policy ->> 'AuthenticationProviderId'
                    ELSE '{DEFAULT_AUTHENTICATION_PROVIDER_ID}'
                END,
                password_reset_provider_id = CASE
                    WHEN password_reset_provider_id IS NOT NULL
                         AND btrim(password_reset_provider_id) <> ''
                         AND char_length(password_reset_provider_id) <= 255
                        THEN password_reset_provider_id
                    WHEN btrim(policy ->> 'PasswordResetProviderId') <> ''
                         AND char_length(policy ->> 'PasswordResetProviderId') <= 255
                        THEN policy ->> 'PasswordResetProviderId'
                    ELSE '{DEFAULT_PASSWORD_RESET_PROVIDER_ID}'
                END;

            ALTER TABLE jellyfin.users
                ALTER COLUMN authentication_provider_id
                    SET DEFAULT '{DEFAULT_AUTHENTICATION_PROVIDER_ID}',
                ALTER COLUMN authentication_provider_id SET NOT NULL,
                ALTER COLUMN password_reset_provider_id
                    SET DEFAULT '{DEFAULT_PASSWORD_RESET_PROVIDER_ID}',
                ALTER COLUMN password_reset_provider_id SET NOT NULL;

            ALTER TABLE jellyfin.users
                DROP CONSTRAINT IF EXISTS users_authentication_provider_not_blank,
                DROP CONSTRAINT IF EXISTS users_password_reset_provider_not_blank;
            ALTER TABLE jellyfin.users
                ADD CONSTRAINT users_authentication_provider_not_blank
                    CHECK (btrim(authentication_provider_id) <> ''),
                ADD CONSTRAINT users_password_reset_provider_not_blank
                    CHECK (btrim(password_reset_provider_id) <> '');

            UPDATE jellyfin.users
            SET policy = jsonb_set(
                    jsonb_set(
                        policy,
                        '{{AuthenticationProviderId}}',
                        to_jsonb(authentication_provider_id),
                        true
                    ),
                    '{{PasswordResetProviderId}}',
                    to_jsonb(password_reset_provider_id),
                    true
                )
            WHERE policy ->> 'AuthenticationProviderId'
                      IS DISTINCT FROM authentication_provider_id
               OR policy ->> 'PasswordResetProviderId'
                      IS DISTINCT FROM password_reset_provider_id;
            "
        );
        manager.get_connection().execute_unprepared(&sql).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.users \
                 DROP COLUMN IF EXISTS authentication_provider_id, \
                 DROP COLUMN IF EXISTS password_reset_provider_id;",
            )
            .await?;
        Ok(())
    }
}
