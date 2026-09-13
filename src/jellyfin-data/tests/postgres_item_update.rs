use std::collections::BTreeMap;

use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, ItemMetadataPatch, ItemUpdateRepository,
    ItemUpdateStoreError, ItemValueRepository, NewBaseItem,
    entities::{base_item, item_value},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

#[tokio::test]
async fn postgres_item_update_matches_official_collection_rows() {
    let database = setup_database().await;
    let item = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("movie fixture creation");
    let updates = ItemUpdateRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());

    let tags_only = updates
        .update(
            item.id,
            ItemMetadataPatch {
                tags: Some(vec!["new-tag-1".to_owned(), "new-tag-2".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .expect("tags-only update");
    assert_eq!(
        metadata_strings(&tags_only, "Tags"),
        ["new-tag-1", "new-tag-2"]
    );
    assert!(metadata_value(&tags_only, "Genres").is_none());
    assert!(metadata_value(&tags_only, "ProviderIds").is_none());
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Tags).await,
        ["new-tag-1", "new-tag-2"]
    );

    let seeded = updates
        .update(
            item.id,
            ItemMetadataPatch {
                tags: Some(vec!["old-tag".to_owned()]),
                genres: Some(vec!["Action".to_owned()]),
                provider_ids: Some(BTreeMap::from([(
                    "Imdb".to_owned(),
                    "tt1234567".to_owned(),
                )])),
                ..Default::default()
            },
        )
        .await
        .expect("existing metadata setup");
    let cleared_tags = ItemUpdateRepository::new(database.clone())
        .update(
            item.id,
            ItemMetadataPatch {
                tags: Some(Vec::new()),
                ..Default::default()
            },
        )
        .await
        .expect("explicit empty tags update");
    assert!(cleared_tags.row_version > seeded.row_version);
    assert!(metadata_strings(&cleared_tags, "Tags").is_empty());
    assert_eq!(metadata_strings(&cleared_tags, "Genres"), ["Action"]);
    assert_eq!(
        metadata_value(&cleared_tags, "ProviderIds"),
        Some(&serde_json::json!({ "Imdb": "tt1234567" }))
    );
    assert!(
        value_names(&values, item.id, item_value::ItemValueType::Tags)
            .await
            .is_empty()
    );
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Genre).await,
        ["Action"]
    );

    cleanup(&database, item.id).await;
}

#[tokio::test]
async fn postgres_item_update_serializes_partial_writers_and_rolls_back() {
    let database = setup_database().await;
    let item = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("movie fixture creation");
    let first = ItemUpdateRepository::new(database.clone());
    let second = ItemUpdateRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let seeded = first
        .update(
            item.id,
            ItemMetadataPatch {
                tags: Some(vec!["stable-tag".to_owned()]),
                genres: Some(vec!["Action".to_owned()]),
                provider_ids: Some(BTreeMap::from([(
                    "Imdb".to_owned(),
                    "tt7654321".to_owned(),
                )])),
                ..Default::default()
            },
        )
        .await
        .expect("metadata setup");

    let error = first
        .update(
            item.id,
            ItemMetadataPatch {
                tags: Some(vec!["must-roll-back".to_owned()]),
                genres: Some(vec!["---".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .expect_err("unsearchable genre must fail");
    assert!(matches!(error, ItemUpdateStoreError::InvalidValue));
    let after_failure = BaseItemRepository::new(database.clone())
        .get(item.id)
        .await
        .expect("post-rollback item lookup")
        .expect("post-rollback item");
    assert_eq!(after_failure.row_version, seeded.row_version);
    assert_eq!(metadata_strings(&after_failure, "Tags"), ["stable-tag"]);
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Tags).await,
        ["stable-tag"]
    );

    let (tags_result, providers_result) = tokio::join!(
        first.update(
            item.id,
            ItemMetadataPatch {
                tags: Some(vec!["parallel-tag".to_owned()]),
                ..Default::default()
            }
        ),
        second.update(
            item.id,
            ItemMetadataPatch {
                provider_ids: Some(BTreeMap::from([("Tmdb".to_owned(), "12345".to_owned(),)])),
                ..Default::default()
            }
        )
    );
    tags_result.expect("concurrent tags update");
    providers_result.expect("concurrent provider update");

    let persisted = BaseItemRepository::new(database.clone())
        .get(item.id)
        .await
        .expect("cross-instance item lookup")
        .expect("cross-instance item");
    assert_eq!(metadata_strings(&persisted, "Tags"), ["parallel-tag"]);
    assert_eq!(metadata_strings(&persisted, "Genres"), ["Action"]);
    assert_eq!(
        metadata_value(&persisted, "ProviderIds"),
        Some(&serde_json::json!({ "Tmdb": "12345" }))
    );
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Tags).await,
        ["parallel-tag"]
    );
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Genre).await,
        ["Action"]
    );
    assert!(matches!(
        first
            .update(Uuid::new_v4(), ItemMetadataPatch::default())
            .await,
        Err(ItemUpdateStoreError::NotFound)
    ));

    cleanup(&database, item.id).await;
}

#[tokio::test]
async fn postgres_metadata_settings_reset_is_atomic_and_case_insensitive() {
    let database = setup_database().await;
    let repository = BaseItemRepository::new(database.clone());
    let mut first = NewBaseItem::new(Uuid::new_v4(), "Movie");
    first.data = Some(serde_json::json!({
        "isLocked": true,
        "LOCKEDFIELDS": ["Name", "Overview"],
        "ProviderIds": { "Tmdb": "123", "Custom": "opaque-id" },
        "Custom": { "nested": true }
    }));
    let first = repository.create(first).await.expect("first reset fixture");
    let mut second = NewBaseItem::new(Uuid::new_v4(), "Series");
    second.data = Some(serde_json::json!({
        "IsLocked": true,
        "lockedFields": "Genres|Studios",
        "ProviderIds": { "Tvdb": "456" }
    }));
    let second = repository
        .create(second)
        .await
        .expect("second reset fixture");
    let updates = ItemUpdateRepository::new(database.clone());

    let missing = Uuid::new_v4();
    assert!(matches!(
        updates.reset_metadata_settings(&[first.id, missing]).await,
        Err(ItemUpdateStoreError::NotFound)
    ));
    let unchanged = repository
        .get(first.id)
        .await
        .expect("post-failure lookup")
        .expect("first item still exists");
    assert_eq!(unchanged.row_version, first.row_version);
    assert_eq!(unchanged.data, first.data);

    assert_eq!(
        updates
            .reset_metadata_settings(&[first.id, second.id, first.id])
            .await
            .expect("atomic metadata settings reset"),
        2
    );
    let reset = repository
        .get_many(&[first.id, second.id])
        .await
        .expect("reset item lookup");
    assert_eq!(reset.len(), 2);
    for item in &reset {
        let object = item
            .data
            .as_ref()
            .and_then(Value::as_object)
            .expect("reset metadata object");
        assert_eq!(object.get("IsLocked"), Some(&serde_json::json!(false)));
        assert_eq!(object.get("LockedFields"), Some(&serde_json::json!([])));
        assert_eq!(
            object
                .keys()
                .filter(|key| key.eq_ignore_ascii_case("IsLocked"))
                .count(),
            1
        );
        assert_eq!(
            object
                .keys()
                .filter(|key| key.eq_ignore_ascii_case("LockedFields"))
                .count(),
            1
        );
    }
    let reset_first = reset
        .iter()
        .find(|item| item.id == first.id)
        .expect("reset first item");
    assert_eq!(
        metadata_value(reset_first, "ProviderIds"),
        Some(&serde_json::json!({ "Tmdb": "123", "Custom": "opaque-id" }))
    );
    assert_eq!(
        metadata_value(reset_first, "Custom"),
        Some(&serde_json::json!({ "nested": true }))
    );
    assert!(reset_first.row_version > first.row_version);

    updates
        .update(
            first.id,
            ItemMetadataPatch {
                provider_ids: Some(BTreeMap::from([(
                    "TMDB".to_owned(),
                    "fresh-provider-id".to_owned(),
                )])),
                ..ItemMetadataPatch::default()
            },
        )
        .await
        .expect("simulate provider-owned replacement patch");
    let merged = updates
        .merge_provider_ids_if_missing(
            first.id,
            &BTreeMap::from([
                ("Tmdb".to_owned(), "stale-provider-id".to_owned()),
                ("Custom".to_owned(), "opaque-id".to_owned()),
            ]),
        )
        .await
        .expect("restore provider ids omitted by refresh patch");
    let provider_ids = metadata_value(&merged, "ProviderIds")
        .and_then(Value::as_object)
        .expect("merged provider ids");
    assert_eq!(
        provider_ids.get("TMDB"),
        Some(&serde_json::json!("fresh-provider-id"))
    );
    assert_eq!(
        provider_ids.get("Custom"),
        Some(&serde_json::json!("opaque-id"))
    );
    assert_eq!(
        provider_ids
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("Tmdb"))
            .count(),
        1,
        "the fresh provider-owned value wins case-insensitively",
    );

    cleanup(&database, first.id).await;
    cleanup(&database, second.id).await;
}

#[tokio::test]
async fn postgres_metadata_settings_reset_batches_validation_before_writes() {
    let database = setup_database().await;
    let repository = BaseItemRepository::new(database.clone());
    let updates = ItemUpdateRepository::new(database.clone());
    let mut item_ids = Vec::with_capacity(129);
    let mut first_version = None;
    for index in 0..129 {
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some(format!("batched reset item {index}"));
        item.data = Some(serde_json::json!({ "IsLocked": true }));
        let item = repository.create(item).await.expect("batch fixture item");
        first_version.get_or_insert(item.row_version);
        item_ids.push(item_id);
    }

    let mut with_missing_in_second_batch = item_ids.clone();
    with_missing_in_second_batch.push(Uuid::new_v4());
    assert!(matches!(
        updates
            .reset_metadata_settings(&with_missing_in_second_batch)
            .await,
        Err(ItemUpdateStoreError::NotFound)
    ));
    let unchanged = repository
        .get(item_ids[0])
        .await
        .expect("unchanged batch lookup")
        .expect("unchanged batch item");
    assert_eq!(unchanged.row_version, first_version.expect("first version"));
    assert_eq!(
        metadata_value(&unchanged, "IsLocked"),
        Some(&serde_json::json!(true)),
        "a missing id after the 128-id boundary must roll back before writes",
    );

    assert_eq!(
        updates
            .reset_metadata_settings(&item_ids)
            .await
            .expect("two-batch metadata reset"),
        129
    );
    let reset = repository
        .get(item_ids[128])
        .await
        .expect("second batch lookup")
        .expect("second batch item");
    assert_eq!(
        metadata_value(&reset, "IsLocked"),
        Some(&serde_json::json!(false))
    );

    base_item::Entity::delete_many()
        .filter(base_item::Column::Id.is_in(item_ids))
        .exec(&database)
        .await
        .expect("batch fixture cleanup");
}

#[tokio::test]
async fn postgres_metadata_settings_reset_orders_overlapping_batch_locks() {
    let database = setup_database().await;
    let repository = BaseItemRepository::new(database.clone());
    let mut item_ids = Vec::with_capacity(130);
    for index in 0..130 {
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some(format!("overlapping lock item {index}"));
        item.data = Some(serde_json::json!({ "IsLocked": true }));
        repository
            .create(item)
            .await
            .expect("overlapping lock fixture");
        item_ids.push(item_id);
    }
    let first_ids = item_ids[..129].to_vec();
    let mut second_ids = item_ids[1..].to_vec();
    second_ids.reverse();
    let first = ItemUpdateRepository::new(database.clone());
    let second = ItemUpdateRepository::new(database.clone());
    let (first_result, second_result) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            tokio::join!(
                first.reset_metadata_settings(&first_ids),
                second.reset_metadata_settings(&second_ids),
            )
        })
        .await
        .expect("canonically ordered overlapping row locks must not deadlock");
    assert_eq!(first_result.expect("first overlapping reset"), 129);
    assert_eq!(second_result.expect("second overlapping reset"), 129);

    base_item::Entity::delete_many()
        .filter(base_item::Column::Id.is_in(item_ids))
        .exec(&database)
        .await
        .expect("overlapping lock fixture cleanup");
}

#[tokio::test]
async fn postgres_genre_patch_keeps_json_and_relations_in_lockstep() {
    let database = setup_database().await;
    let item = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Series"))
        .await
        .expect("series fixture creation");
    let updates = ItemUpdateRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());

    let seeded = updates
        .update(
            item.id,
            ItemMetadataPatch {
                genres: Some(vec!["Action".to_owned(), "Drama".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .expect("genre setup");
    assert_eq!(metadata_strings(&seeded, "Genres"), ["Action", "Drama"]);
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Genre).await,
        ["Action", "Drama"]
    );

    let preserved = updates
        .update(
            item.id,
            ItemMetadataPatch {
                provider_ids: Some(BTreeMap::from([("Tmdb".to_owned(), "12345".to_owned())])),
                genres: None,
                ..Default::default()
            },
        )
        .await
        .expect("omitted genre patch");
    assert_eq!(metadata_strings(&preserved, "Genres"), ["Action", "Drama"]);
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Genre).await,
        ["Action", "Drama"]
    );

    let replaced = updates
        .update(
            item.id,
            ItemMetadataPatch {
                genres: Some(vec!["Comedy".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .expect("replacement genre patch");
    assert_eq!(metadata_strings(&replaced, "Genres"), ["Comedy"]);
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Genre).await,
        ["Comedy"]
    );

    cleanup(&database, item.id).await;
}

#[tokio::test]
async fn postgres_studio_patch_keeps_nullable_json_and_relations_atomic() {
    let database = setup_database().await;
    let item = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("movie fixture creation");
    let updates = ItemUpdateRepository::new(database.clone());
    let values = ItemValueRepository::new(database.clone());
    let seeded = updates
        .update(
            item.id,
            ItemMetadataPatch {
                studios: Some(vec![
                    Some(" Alpha Studio ".to_owned()),
                    None,
                    Some(String::new()),
                    Some("  ".to_owned()),
                    Some("---".to_owned()),
                    Some("Beta Studio".to_owned()),
                ]),
                ..Default::default()
            },
        )
        .await
        .expect("studio setup");
    assert_eq!(
        metadata_value(&seeded, "Studios"),
        Some(&serde_json::json!([
            " Alpha Studio ",
            null,
            "",
            "  ",
            "---",
            "Beta Studio"
        ]))
    );
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Studios).await,
        ["Alpha Studio", "Beta Studio"]
    );

    let preserved = updates
        .update(
            item.id,
            ItemMetadataPatch {
                genres: Some(vec!["Drama".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .expect("unrelated metadata edit");
    assert_eq!(
        metadata_value(&preserved, "Studios"),
        metadata_value(&seeded, "Studios")
    );
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Studios).await,
        ["Alpha Studio", "Beta Studio"]
    );

    assert_studio_patch_rollback(&database, &updates, &values, &preserved).await;

    let cleared = updates
        .update(
            item.id,
            ItemMetadataPatch {
                studios: Some(Vec::new()),
                ..Default::default()
            },
        )
        .await
        .expect("explicit empty studio edit");
    assert_eq!(
        metadata_value(&cleared, "Studios"),
        Some(&serde_json::json!([]))
    );
    assert!(
        value_names(&values, item.id, item_value::ItemValueType::Studios)
            .await
            .is_empty()
    );
    assert_eq!(metadata_strings(&cleared, "Genres"), ["Drama"]);

    let mut batch_names = (0..129)
        .map(|index| Some(format!("Batch Studio {index:03}")))
        .collect::<Vec<_>>();
    // A second input spelling with the same searchable key must not make one
    // PostgreSQL ON CONFLICT statement affect the same row twice.
    batch_names.push(Some(" batch studio 000 ".to_owned()));
    let batched = updates
        .update(
            item.id,
            ItemMetadataPatch {
                studios: Some(batch_names),
                ..Default::default()
            },
        )
        .await
        .expect("studio edit across bounded batch boundary");
    assert_eq!(metadata_strings(&batched, "Studios").len(), 130);
    assert_eq!(
        value_names(&values, item.id, item_value::ItemValueType::Studios)
            .await
            .len(),
        129
    );
    cleanup(&database, item.id).await;
}

async fn assert_studio_patch_rollback(
    database: &sea_orm::DatabaseConnection,
    updates: &ItemUpdateRepository,
    values: &ItemValueRepository,
    preserved: &base_item::Model,
) {
    let failure = updates
        .update(
            preserved.id,
            ItemMetadataPatch {
                tags: Some(vec!["Must Roll Back".to_owned()]),
                genres: Some(vec!["---".to_owned()]),
                studios: Some(vec![Some("Must Not Replace Studios".to_owned())]),
                ..Default::default()
            },
        )
        .await
        .expect_err("invalid genre must fail the whole transaction");
    assert!(matches!(failure, ItemUpdateStoreError::InvalidValue));
    let after_failure = BaseItemRepository::new(database.clone())
        .get(preserved.id)
        .await
        .expect("rollback item lookup")
        .expect("rollback item");
    assert_eq!(after_failure.row_version, preserved.row_version);
    assert_eq!(
        metadata_value(&after_failure, "Studios"),
        metadata_value(preserved, "Studios")
    );
    assert_eq!(
        value_names(values, preserved.id, item_value::ItemValueType::Genre).await,
        ["Drama"]
    );
    assert_eq!(
        value_names(values, preserved.id, item_value::ItemValueType::Studios).await,
        ["Alpha Studio", "Beta Studio"]
    );
}

async fn setup_database() -> sea_orm::DatabaseConnection {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}

async fn value_names(
    repository: &ItemValueRepository,
    item_id: Uuid,
    value_type: item_value::ItemValueType,
) -> Vec<String> {
    repository
        .values_for_item(item_id, value_type)
        .await
        .expect("item metadata value lookup")
        .into_iter()
        .map(|value| value.value)
        .collect()
}

fn metadata_value<'a>(item: &'a base_item::Model, key: &str) -> Option<&'a Value> {
    item.data.as_ref()?.as_object()?.get(key)
}

fn metadata_strings<'a>(item: &'a base_item::Model, key: &str) -> Vec<&'a str> {
    metadata_value(item, key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

async fn cleanup(database: &sea_orm::DatabaseConnection, item_id: Uuid) {
    base_item::Entity::delete_many()
        .filter(base_item::Column::Id.eq(item_id))
        .exec(database)
        .await
        .expect("item-update fixture cleanup");
}
