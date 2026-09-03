use jellyfin_data::{
    DatabaseConfig, NewMediaPath, NewVirtualFolder, VirtualFolderError, VirtualFolderRepository,
    entities::{media_path, virtual_folder},
};
use jellyfin_migration::CreateVirtualFoldersMigration;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    Statement, TransactionTrait,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn postgres_virtual_folders_vertical_slice() {
    let database = prepare_database().await;
    let repository = VirtualFolderRepository::new(database.clone());
    let suffix = Uuid::new_v4().simple().to_string();

    assert_unicode_concurrent_name_uniqueness(&repository, &suffix).await;
    let fixture = assert_crud_path_overlap_and_cascade(&database, &repository, &suffix).await;
    assert_postgres_catalog(&database).await;
    assert_containment_query_plan(&database, &fixture.child_path).await;
    cleanup(&database, &suffix).await;
}

struct PathFixture {
    child_path: String,
}

async fn prepare_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let schema = SchemaManager::new(&database);
    CreateVirtualFoldersMigration
        .up(&schema)
        .await
        .expect("reapplying virtual-folder DDL must succeed");
    CreateVirtualFoldersMigration
        .up(&schema)
        .await
        .expect("virtual-folder DDL must remain idempotent");
    database
}

async fn assert_unicode_concurrent_name_uniqueness(
    repository: &VirtualFolderRepository,
    suffix: &str,
) {
    let first = new_folder(format!("Cinéma 東京 {suffix}"));
    let equivalent = new_folder(format!("CINEMA---東京---{suffix}"));
    let (one, two) = tokio::join!(
        repository.create(first, Vec::new()),
        repository.create(equivalent, Vec::new())
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let error = one.err().or_else(|| two.err()).expect("one conflict");
    assert!(matches!(error, VirtualFolderError::DuplicateName));
}

async fn assert_crud_path_overlap_and_cascade(
    database: &DatabaseConnection,
    repository: &VirtualFolderRepository,
    suffix: &str,
) -> PathFixture {
    let library_name = format!("Path library {suffix}");
    let root_path = format!("/tmp/jellyfin-rust-vf-{suffix}/media");
    let child_path = format!("{root_path}/movies");
    let sibling_path = format!("/tmp/jellyfin-rust-vf-{suffix}/music");
    let created = repository
        .create(new_folder(library_name.clone()), vec![new_path(&root_path)])
        .await
        .expect("folder and path creation");
    assert_eq!(created.paths.len(), 1);
    assert_eq!(created.folder.library_options["Enabled"], false);

    assert!(matches!(
        repository
            .add_path(&library_name, new_path(&child_path), false)
            .await,
        Err(VirtualFolderError::PathOverlap)
    ));
    repository
        .add_path(&library_name, new_path(&sibling_path), true)
        .await
        .expect("non-overlapping sibling path");

    let second_library = format!("Second path library {suffix}");
    repository
        .create(new_folder(second_library.clone()), Vec::new())
        .await
        .expect("second library");
    let parent_path = format!("/tmp/jellyfin-rust-vf-{suffix}");
    assert!(matches!(
        repository
            .add_path(&second_library, new_path(&parent_path), false)
            .await,
        Err(VirtualFolderError::PathOverlap)
    ));

    repository
        .update_path(
            &library_name,
            &root_path,
            json!({ "Path": root_path, "NetworkPath": "smb://media" }),
        )
        .await
        .expect("path metadata update");
    let loaded = repository
        .get_by_name(&library_name)
        .await
        .expect("folder lookup")
        .expect("folder exists");
    assert!(loaded.folder.refresh_requested);
    assert_eq!(loaded.paths[0].path_info["NetworkPath"], "smb://media");

    repository
        .remove_path(&library_name, &sibling_path, false)
        .await
        .expect("path removal");
    assert!(matches!(
        repository
            .remove_path(&library_name, &sibling_path, false)
            .await,
        Err(VirtualFolderError::PathNotFound)
    ));

    let folder_id = loaded.folder.id;
    repository
        .delete(&library_name, true)
        .await
        .expect("folder deletion");
    assert_eq!(
        media_path::Entity::find()
            .filter(media_path::Column::VirtualFolderId.eq(folder_id))
            .count(database)
            .await
            .expect("cascade count"),
        0
    );
    assert!(matches!(
        repository.delete(&library_name, false).await,
        Err(VirtualFolderError::NotFound)
    ));
    PathFixture { child_path }
}

async fn assert_postgres_catalog(database: &DatabaseConnection) {
    let constraints = names(
        database,
        "SELECT conname AS name FROM pg_constraint WHERE conrelid IN \
         ('jellyfin.virtual_folders'::regclass, 'jellyfin.media_paths'::regclass)",
    )
    .await;
    for expected in [
        "virtual_folders_normalized_name_key",
        "virtual_folders_options_object",
        "media_paths_virtual_folder_fkey",
        "media_paths_normalized_path_key",
        "media_paths_ancestors_array",
    ] {
        assert!(
            constraints.iter().any(|name| name == expected),
            "{expected}"
        );
    }
    let indexes = names(
        database,
        "SELECT indexname AS name FROM pg_indexes WHERE schemaname = 'jellyfin' \
         AND tablename IN ('virtual_folders', 'media_paths')",
    )
    .await;
    for expected in [
        "virtual_folders_name_trgm_idx",
        "media_paths_folder_path_idx",
        "media_paths_ancestors_gin_idx",
    ] {
        assert!(indexes.iter().any(|name| name == expected), "{expected}");
    }
}

async fn assert_containment_query_plan(database: &DatabaseConnection, child_path: &str) {
    let transaction = database.begin().await.expect("plan transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scan");
    let plan = transaction
        .query_all(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "EXPLAIN SELECT 1 FROM jellyfin.media_paths WHERE path_ancestors ? $1",
            [child_path.into()],
        ))
        .await
        .expect("containment explain")
        .into_iter()
        .map(|row| row.try_get::<String>("", "QUERY PLAN").expect("plan row"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plan.contains("media_paths_ancestors_gin_idx"), "{plan}");
    transaction.rollback().await.expect("plan rollback");
}

async fn names(database: &DatabaseConnection, sql: &str) -> Vec<String> {
    database
        .query_all(Statement::from_string(
            sea_orm::DbBackend::Postgres,
            sql.to_owned(),
        ))
        .await
        .expect("catalog query")
        .into_iter()
        .map(|row| row.try_get("", "name").expect("catalog name"))
        .collect()
}

fn new_folder(name: String) -> NewVirtualFolder {
    NewVirtualFolder {
        name,
        collection_type: Some("movies".to_owned()),
        library_options: json!({ "Enabled": false, "PathInfos": [] }),
        refresh_requested: false,
    }
}

fn new_path(path: &str) -> NewMediaPath {
    let ancestors = std::path::Path::new(path)
        .ancestors()
        .map(|ancestor| ancestor.to_string_lossy().into_owned())
        .collect();
    NewMediaPath {
        path: path.to_owned(),
        normalized_path: path.to_owned(),
        ancestors,
        path_info: json!({ "Path": path }),
    }
}

async fn cleanup(database: &DatabaseConnection, suffix: &str) {
    virtual_folder::Entity::delete_many()
        .filter(virtual_folder::Column::Name.contains(suffix))
        .exec(database)
        .await
        .expect("fixture cleanup");
}
