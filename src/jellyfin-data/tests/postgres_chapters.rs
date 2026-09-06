use std::collections::HashMap;

use jellyfin_data::{
    BaseItemRepository, ChapterRecord, ChapterRepository, DatabaseConfig, NewBaseItem, NewChapter,
    entities::base_item,
};
use jellyfin_migration::OptimizeChapterQueriesMigration;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement, TransactionTrait,
    TryGetable,
};
use sea_orm_migration::{MigrationTrait, SchemaManager};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_chapters_";

#[tokio::test]
async fn postgres_chapters_batch_in_official_dto_order() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_chapters(&task_database_name).await;
    })
    .await;

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

async fn exercise_chapters(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 4,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let schema = SchemaManager::new(&database);
    OptimizeChapterQueriesMigration
        .up(&schema)
        .await
        .expect("reapplying chapter query index must be idempotent");

    let items = BaseItemRepository::new(database.clone());
    let chapters = ChapterRepository::new(database.clone());
    let first = create_item(&items, "first").await;
    let second = create_item(&items, "second").await;
    let without_chapters = create_item(&items, "empty").await;

    chapters
        .replace(
            first.id,
            vec![
                chapter(8, 300, "Credits"),
                chapter(5, 100, "Part two"),
                chapter(2, 100, "Part one"),
            ],
        )
        .await
        .expect("first item chapters must persist");
    chapters
        .replace(second.id, vec![chapter(7, 200, "Only")])
        .await
        .expect("second item chapters must persist");

    assert_indexes_then_start_positions(
        &chapters.list_for_item(first.id).await.unwrap(),
        &[2, 5, 8],
        &[100, 100, 300],
    );

    let grouped = chapters
        .list_many(&[second.id, first.id, first.id, without_chapters.id])
        .await
        .expect("batched chapter query must succeed");
    assert_eq!(grouped.len(), 3, "duplicate item ids must be deduplicated");
    assert_indexes_then_start_positions(&grouped[&first.id], &[2, 5, 8], &[100, 100, 300]);
    assert_indexes_then_start_positions(&grouped[&second.id], &[7], &[200]);
    assert_eq!(grouped[&without_chapters.id], Vec::<ChapterRecord>::new());
    assert_eq!(
        chapters.list_many(&[]).await.unwrap(),
        HashMap::new(),
        "an empty page must not require a database query"
    );

    assert_chapter_query_index(&database, first.id).await;
    database
        .close()
        .await
        .expect("temporary database connection must close");
}

fn assert_indexes_then_start_positions(
    records: &[ChapterRecord],
    expected_indexes: &[i32],
    expected_starts: &[i64],
) {
    assert_eq!(
        records
            .iter()
            .map(|chapter| chapter.index_number)
            .collect::<Vec<_>>(),
        expected_indexes
    );
    assert_eq!(
        records
            .iter()
            .map(|chapter| chapter.start_position_ticks)
            .collect::<Vec<_>>(),
        expected_starts
    );
}

async fn assert_chapter_query_index(database: &DatabaseConnection, item_id: Uuid) {
    let transaction = database.begin().await.expect("EXPLAIN transaction");
    transaction
        .execute_unprepared("SET LOCAL enable_seqscan = off")
        .await
        .expect("disable sequential scans");
    let plan = transaction
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.chapters \
             WHERE item_id = $1 \
             ORDER BY start_position_ticks, index_number, id",
            [item_id.into()],
        ))
        .await
        .expect("chapter query EXPLAIN")
        .iter()
        .map(explain_line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("chapters_item_start_idx"),
        "expected chapter start-position index plan:\n{plan}"
    );
    transaction.rollback().await.expect("EXPLAIN rollback");
}

fn explain_line(row: &QueryResult) -> String {
    String::try_get(row, "", "QUERY PLAN").expect("EXPLAIN line must be text")
}

async fn create_item(items: &BaseItemRepository, label: &str) -> base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Video");
    item.name = Some(label.to_owned());
    item.sort_name = Some(label.to_owned());
    items.create(item).await.expect("base-item creation")
}

fn chapter(index_number: i32, start_position_ticks: i64, name: &str) -> NewChapter {
    NewChapter {
        index_number,
        start_position_ticks,
        end_position_ticks: start_position_ticks + 99,
        name: Some(name.to_owned()),
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(
        name.starts_with(DATABASE_PREFIX)
            && name.len() > DATABASE_PREFIX.len()
            && name[DATABASE_PREFIX.len()..]
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
        "refusing unsafe temporary database name: {name}"
    );
}
