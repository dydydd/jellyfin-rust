use jellyfin_data::{
    BaseItemQuery, BaseItemRepository, DatabaseConfig, ItemValueRepository, NewBaseItem,
    entities::item_value,
};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::json;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_recursive_item_counts_";

#[tokio::test]
async fn recursive_item_counts_follow_links_merge_folders_and_filter_leaves() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary recursive-count database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_recursive_item_counts(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary recursive-count database cleanup must succeed");
    administrator.close().await.expect("admin pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("recursive-count test task was cancelled: {error}");
    }
}

#[allow(clippy::too_many_lines)]
async fn exercise_recursive_item_counts(database_name: &str) {
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

    let repository = BaseItemRepository::new(database.clone());
    let root = create_item(&repository, "Folder", None, true, None, None, None).await;
    let group_key = format!("merged-series-{}", Uuid::new_v4().simple());
    let series_a = create_item(
        &repository,
        "Series",
        Some(root.id),
        true,
        Some(group_key.clone()),
        None,
        None,
    )
    .await;
    let series_b = create_item(
        &repository,
        "Series",
        Some(root.id),
        true,
        Some(group_key),
        None,
        None,
    )
    .await;
    let empty = create_item(&repository, "Folder", Some(root.id), true, None, None, None).await;

    let season = create_item(
        &repository,
        "Season",
        Some(series_a.id),
        true,
        None,
        None,
        None,
    )
    .await;
    let primary = create_item(
        &repository,
        "Episode",
        Some(season.id),
        false,
        None,
        None,
        None,
    )
    .await;
    let _alternate = create_item(
        &repository,
        "Episode",
        Some(season.id),
        false,
        None,
        Some(primary.id),
        None,
    )
    .await;
    let mut virtual_item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    virtual_item.parent_id = Some(season.id);
    virtual_item.is_virtual_item = true;
    repository
        .create(virtual_item)
        .await
        .expect("virtual episode creation");
    let _owned = create_item(
        &repository,
        "Movie",
        Some(season.id),
        false,
        None,
        None,
        Some(json!({"OwnerId": primary.id.simple().to_string()})),
    )
    .await;

    let _second_leaf = create_item(
        &repository,
        "Episode",
        Some(series_b.id),
        false,
        None,
        None,
        None,
    )
    .await;
    let blocked_leaf = create_item(
        &repository,
        "Episode",
        Some(series_b.id),
        false,
        None,
        None,
        None,
    )
    .await;
    ItemValueRepository::new(database.clone())
        .link(blocked_leaf.id, item_value::ItemValueType::Tags, "blocked")
        .await
        .expect("blocked tag link");

    let linked_folder =
        create_item(&repository, "Folder", Some(root.id), true, None, None, None).await;
    let _linked_leaf = create_item(
        &repository,
        "Movie",
        Some(linked_folder.id),
        false,
        None,
        None,
        None,
    )
    .await;
    let nested_linked_folder =
        create_item(&repository, "Folder", Some(root.id), true, None, None, None).await;
    let _nested_linked_leaf = create_item(
        &repository,
        "Movie",
        Some(nested_linked_folder.id),
        false,
        None,
        None,
        None,
    )
    .await;
    for (parent_id, child_id) in [
        (series_a.id, linked_folder.id),
        (linked_folder.id, nested_linked_folder.id),
        (nested_linked_folder.id, series_a.id),
    ] {
        database
            .execute(Statement::from_sql_and_values(
                database.get_database_backend(),
                "INSERT INTO jellyfin.linked_children (parent_id, child_id, child_type) \
                 VALUES ($1, $2, 0)",
                [parent_id.into(), child_id.into()],
            ))
            .await
            .expect("linked child insertion");
    }

    let parent_ids = [series_a.id, series_b.id, empty.id];
    let unrestricted = repository
        .dto_recursive_item_counts(
            &parent_ids,
            &BaseItemQuery {
                is_virtual_item: Some(false),
                enable_all_folders: true,
                ..BaseItemQuery::default()
            },
        )
        .await
        .expect("unrestricted recursive counts");
    assert_eq!(unrestricted[&series_a.id], 5);
    assert_eq!(unrestricted[&series_b.id], 5);
    assert_eq!(unrestricted[&empty.id], 0);

    let policy_filtered = repository
        .dto_recursive_item_counts(
            &parent_ids,
            &BaseItemQuery {
                is_virtual_item: Some(false),
                blocked_tags: vec!["blocked".to_owned()],
                enable_all_folders: true,
                ..BaseItemQuery::default()
            },
        )
        .await
        .expect("policy-filtered recursive counts");
    assert_eq!(policy_filtered[&series_a.id], 4);
    assert_eq!(policy_filtered[&series_b.id], 4);
    assert_eq!(policy_filtered[&empty.id], 0);

    database.close().await.expect("task pool cleanup");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
    presentation_unique_key: Option<String>,
    primary_version_id: Option<Uuid>,
    data: Option<serde_json::Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item.presentation_unique_key = presentation_unique_key;
    item.primary_version_id = primary_version_id;
    item.data = data;
    repository.create(item).await.expect("base item creation")
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    );
}
