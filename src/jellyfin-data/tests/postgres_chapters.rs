use std::{collections::HashMap, time::Duration};

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
    let indexed = chapters
        .get(first.id, 5)
        .await
        .expect("indexed chapter lookup must succeed")
        .expect("persisted chapter index must resolve");
    assert_eq!(indexed.name.as_deref(), Some("Part two"));
    assert_eq!(indexed.start_position_ticks, 100);
    assert_eq!(chapters.get(first.id, 6).await.unwrap(), None);
    assert_eq!(chapters.get(without_chapters.id, 5).await.unwrap(), None);

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

    assert_chapter_query_indexes(&database, first.id).await;
    assert_atomic_batched_replacement(&chapters, first.id).await;
    assert_concurrent_replacements(&database, &chapters, without_chapters.id).await;
    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_atomic_batched_replacement(chapters: &ChapterRepository, item_id: Uuid) {
    let input = (0..257)
        .rev()
        .map(|index| chapter(index, i64::from(index) * 100, "Batched"))
        .collect();
    let saved = chapters
        .replace(item_id, input)
        .await
        .expect("chapter replacement crossing two batch boundaries must succeed");
    assert_eq!(saved.len(), 257);
    assert_eq!(saved.first().unwrap().index_number, 256);
    assert_eq!(saved.last().unwrap().index_number, 0);
    let mut expected = saved;
    expected.reverse();
    assert_eq!(chapters.list_for_item(item_id).await.unwrap(), expected);

    // A constraint violation in the second INSERT must roll back the first batch and the
    // preceding DELETE, including the original chapter identities and thumbnail metadata.
    chapters
        .set_image_data(expected[0].id, "/metadata/chapter.jpg", chrono::Utc::now())
        .await
        .unwrap();
    let original = chapters.list_for_item(item_id).await.unwrap();
    let mut invalid = (0..128)
        .map(|index| chapter(index, i64::from(index) * 100, "Must roll back"))
        .collect::<Vec<_>>();
    invalid.push(chapter(0, 99_999, "Duplicate index in a later batch"));
    assert!(chapters.replace(item_id, invalid).await.is_err());
    assert_eq!(chapters.list_for_item(item_id).await.unwrap(), original);

    assert!(
        chapters
            .replace(item_id, Vec::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(chapters.list_for_item(item_id).await.unwrap().is_empty());
}

async fn assert_concurrent_replacements(
    database: &DatabaseConnection,
    chapters: &ChapterRepository,
    item_id: Uuid,
) {
    let holder = database.begin().await.unwrap();
    holder
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM jellyfin.base_items WHERE id = $1 FOR UPDATE",
            [item_id.into()],
        ))
        .await
        .unwrap();

    let first_repository = chapters.clone();
    let first = tokio::spawn(async move {
        first_repository
            .replace(
                item_id,
                (0..129)
                    .map(|index| chapter(index, i64::from(index) * 100, "First replacement"))
                    .collect(),
            )
            .await
    });
    let second_repository = chapters.clone();
    let second = tokio::spawn(async move {
        second_repository
            .replace(
                item_id,
                (200..330)
                    .map(|index| chapter(index, i64::from(index) * 100, "Second replacement"))
                    .collect(),
            )
            .await
    });

    // Both writers must reach a database lock before the holder releases the empty item.
    // Without owner serialization, both DELETEs finish first and their disjoint inserts merge.
    let waiting = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = database
                .query_one(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT COUNT(*)::bigint AS waiting FROM pg_stat_activity \
                     WHERE datname = current_database() AND wait_event_type = 'Lock' \
                     AND pid <> pg_backend_pid()",
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "waiting")
                .unwrap();
            if count >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    holder.commit().await.unwrap();
    waiting.expect("both chapter writers must reach their database lock");
    let first = first.await.unwrap().expect("first concurrent replacement");
    let second = second
        .await
        .unwrap()
        .expect("second concurrent replacement");
    let saved = chapters.list_for_item(item_id).await.unwrap();
    assert!(
        saved == first || saved == second,
        "concurrent replacements must leave one complete chapter set, never a union"
    );
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

async fn assert_chapter_query_indexes(database: &DatabaseConnection, item_id: Uuid) {
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

    let plan = transaction
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "EXPLAIN (FORMAT TEXT) SELECT * FROM jellyfin.chapters \
             WHERE item_id = $1 AND index_number = $2",
            [item_id.into(), 5_i32.into()],
        ))
        .await
        .expect("chapter index lookup EXPLAIN")
        .iter()
        .map(explain_line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("Index Scan")
            && (plan.contains("chapters_item_index_unique")
                || plan.contains("chapters_item_start_idx")),
        "expected indexed chapter lookup plan:\n{plan}"
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
