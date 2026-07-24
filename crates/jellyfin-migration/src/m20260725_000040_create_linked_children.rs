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
                CREATE TABLE IF NOT EXISTS jellyfin.linked_children (
                    parent_id uuid NOT NULL,
                    child_id uuid NOT NULL,
                    child_type smallint NOT NULL,
                    sort_order integer,
                    CONSTRAINT linked_children_pkey PRIMARY KEY (parent_id, child_id),
                    CONSTRAINT linked_children_parent_id_fkey
                        FOREIGN KEY (parent_id) REFERENCES jellyfin.base_items (id),
                    CONSTRAINT linked_children_child_id_fkey
                        FOREIGN KEY (child_id) REFERENCES jellyfin.base_items (id),
                    CONSTRAINT linked_children_type_valid CHECK (child_type BETWEEN 0 AND 3),
                    CONSTRAINT linked_children_sort_order_nonnegative
                        CHECK (sort_order IS NULL OR sort_order >= 0),
                    CONSTRAINT linked_children_not_self CHECK (parent_id <> child_id)
                );

                CREATE INDEX IF NOT EXISTS linked_children_parent_order_idx
                    ON jellyfin.linked_children
                        (parent_id, sort_order NULLS LAST, child_id)
                    INCLUDE (child_type);

                CREATE INDEX IF NOT EXISTS linked_children_manual_parent_lookup_idx
                    ON jellyfin.linked_children (child_id, parent_id)
                    WHERE child_type = 0;

                COMMENT ON TABLE jellyfin.linked_children IS
                    'Ordered non-hierarchical item relationships shared by collections, '
                    'playlists, shortcuts, and alternate media versions.';
                ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS jellyfin.linked_children CASCADE;")
            .await?;
        Ok(())
    }
}
