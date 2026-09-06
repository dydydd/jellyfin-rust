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
                WITH parent_maximum AS MATERIALIZED (
                    SELECT parent_id, COALESCE(MAX(sort_order), -1) AS maximum
                    FROM jellyfin.linked_children
                    GROUP BY parent_id
                ), ordered AS (
                    SELECT link.parent_id,
                           link.child_id,
                           CAST(parent_maximum.maximum
                               + ROW_NUMBER() OVER (
                                   PARTITION BY link.parent_id
                                   ORDER BY link.child_type, link.child_id
                               ) AS integer) AS repaired_sort_order
                    FROM jellyfin.linked_children AS link
                    INNER JOIN parent_maximum
                        ON parent_maximum.parent_id = link.parent_id
                    WHERE link.child_type IN (2, 3)
                      AND link.sort_order IS NULL
                )
                UPDATE jellyfin.linked_children AS link
                SET sort_order = ordered.repaired_sort_order
                FROM ordered
                WHERE link.parent_id = ordered.parent_id
                  AND link.child_id = ordered.child_id
                  AND link.sort_order IS NULL;

                ALTER TABLE jellyfin.linked_children
                    DROP CONSTRAINT IF EXISTS linked_children_alternate_sort_order_required;
                ALTER TABLE jellyfin.linked_children
                    ADD CONSTRAINT linked_children_alternate_sort_order_required
                    CHECK (child_type NOT IN (2, 3) OR sort_order IS NOT NULL);
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE jellyfin.linked_children \
                 DROP CONSTRAINT IF EXISTS linked_children_alternate_sort_order_required;",
            )
            .await?;
        Ok(())
    }
}
