use std::sync::Arc;

use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, DatabaseConfig, ItemValueRepository,
    LinkedChildRepository, LinkedChildType, NewBaseItem, entities::item_value,
};
use serde_json::json;
use tokio::sync::Barrier;
use uuid::Uuid;

#[tokio::test]
async fn media_source_versions_expand_the_group_with_the_requested_version_first() {
    let repository = repository().await;
    let group = create_group(&repository, "playback-order").await;

    let from_primary = repository
        .media_source_versions(group.primary)
        .await
        .expect("primary media-source versions");
    assert_eq!(from_primary.len(), 3);
    assert_eq!(from_primary[0].id, group.primary);
    assert!(
        from_primary
            .iter()
            .skip(1)
            .all(|item| item.primary_version_id == Some(group.primary))
    );

    let requested_alternate = group.alternates[1];
    let from_alternate = repository
        .media_source_versions(requested_alternate)
        .await
        .expect("alternate media-source versions");
    assert_eq!(from_alternate.len(), 3);
    assert_eq!(from_alternate[0].id, requested_alternate);
    assert_eq!(from_alternate[1].id, group.primary);
    assert_eq!(
        from_alternate
            .iter()
            .map(|item| item.id)
            .collect::<std::collections::HashSet<_>>(),
        group.ids().into_iter().collect()
    );

    let missing = Uuid::new_v4();
    let counts = repository
        .media_source_counts(&[group.primary, requested_alternate, missing])
        .await
        .expect("batched media-source counts");
    assert_eq!(counts.get(&group.primary), Some(&3));
    assert_eq!(counts.get(&requested_alternate), Some(&3));
    assert!(!counts.contains_key(&missing));

    assert!(
        repository
            .media_source_versions(missing)
            .await
            .expect("missing media-source versions")
            .is_empty()
    );
    cleanup(&repository, [&group]).await;
}

#[tokio::test]
async fn media_source_versions_load_multiple_groups_in_one_batch() {
    let repository = repository().await;
    let group_a = create_group(&repository, "batch-a").await;
    let group_b = create_group(&repository, "batch-b").await;

    let sources = repository
        .media_source_versions_for_items(&[group_a.alternates[0], group_b.primary, Uuid::new_v4()])
        .await
        .expect("batched media-source versions");
    assert_eq!(sources.len(), 6);
    let source_ids = sources
        .iter()
        .map(|item| item.id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        source_ids,
        group_a.ids().into_iter().chain(group_b.ids()).collect()
    );

    cleanup(&repository, [&group_a, &group_b]).await;
}

#[tokio::test]
async fn visible_item_ids_filter_alternate_sources_by_access_policy() {
    let repository = repository().await;
    let group = create_group(&repository, "visible-ids").await;
    let values = ItemValueRepository::new(
        jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available"),
    );
    values
        .link(
            group.alternates[0],
            item_value::ItemValueType::Tags,
            "PrivateVersion",
        )
        .await
        .expect("blocked alternate tag");

    let access_policy = BaseItemQuery {
        blocked_tags: vec!["privateversion".to_owned()],
        enable_all_folders: true,
        ..BaseItemQuery::default()
    };
    let visible = repository
        .visible_item_ids(&group.ids(), &access_policy)
        .await
        .expect("policy-filtered alternate identifiers");

    assert_eq!(
        visible,
        [group.primary, group.alternates[1]].into_iter().collect()
    );

    let visible_counts = repository
        .visible_media_source_counts(&group.ids(), &access_policy)
        .await
        .expect("policy-filtered media-source counts");
    assert_eq!(visible_counts.get(&group.primary), Some(&2));
    assert_eq!(visible_counts.get(&group.alternates[1]), Some(&2));
    assert_eq!(
        visible_counts.get(&group.alternates[0]),
        Some(&3),
        "the explicitly displayed source remains countable even when its tag is blocked"
    );
    cleanup(&repository, [&group]).await;
}

#[tokio::test]
async fn clearing_local_alternates_is_a_noop_and_preserves_rows() {
    let repository = repository().await;
    assert!(matches!(
        repository.clear_alternate_sources(Uuid::new_v4()).await,
        Err(BaseItemError::NotFound)
    ));

    let group_a = create_group(&repository, "a").await;
    let group_b = create_group(&repository, "b").await;
    let before_a = load_group(&repository, &group_a).await;
    let before_b = load_group(&repository, &group_b).await;

    repository
        .clear_alternate_sources(group_a.alternates[0])
        .await
        .expect("alternate entry point must clear its complete group");
    let after_a = load_group(&repository, &group_a).await;
    let after_b = load_group(&repository, &group_b).await;
    for (before, after) in before_a.iter().zip(&after_a) {
        assert_eq!(after.primary_version_id, before.primary_version_id);
        assert_eq!(after.row_version, before.row_version);
        assert_eq!(after.path, before.path);
        assert_eq!(after.data, before.data);
        assert!(
            after
                .path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file()),
            "clearing alternate sources must not remove media files"
        );
    }
    assert_eq!(
        after_b, before_b,
        "an unrelated version group must not change"
    );

    repository
        .clear_alternate_sources(group_b.primary)
        .await
        .expect("primary entry point must preserve its local group");
    assert_eq!(load_group(&repository, &group_b).await, before_b);

    cleanup(&repository, [&group_a, &group_b]).await;
}

#[tokio::test]
async fn concurrent_linked_clears_restore_the_local_group_atomically() {
    let repository = repository().await;
    let group = create_group(&repository, "concurrent").await;
    let (standalone, standalone_directory) =
        create_standalone_video(&repository, "concurrent-standalone").await;
    repository
        .merge_linked_alternate_versions(&[group.primary, standalone])
        .await
        .expect("linked version merge");
    let barrier = Arc::new(Barrier::new(3));
    let primary = spawn_clear(repository.clone(), Arc::clone(&barrier), group.primary);
    let alternate = spawn_clear(
        repository.clone(),
        Arc::clone(&barrier),
        group.alternates[1],
    );
    barrier.wait().await;
    primary
        .await
        .expect("primary clear task must join")
        .expect("primary clear must succeed");
    alternate
        .await
        .expect("alternate clear task must join")
        .expect("alternate clear must succeed");

    let after_concurrent_clear = load_group(&repository, &group).await;
    assert_eq!(after_concurrent_clear.len(), 3);
    assert_eq!(after_concurrent_clear[0].primary_version_id, None);
    assert_eq!(
        after_concurrent_clear[1].primary_version_id,
        Some(group.primary)
    );
    assert_eq!(
        after_concurrent_clear[2].primary_version_id,
        Some(group.primary)
    );
    assert_eq!(
        repository
            .get(standalone)
            .await
            .expect("standalone lookup")
            .expect("standalone row")
            .primary_version_id,
        None
    );
    for id in group.ids() {
        repository
            .clear_alternate_sources(id)
            .await
            .expect("repeated clear must remain successful");
    }
    assert_eq!(
        load_group(&repository, &group).await,
        after_concurrent_clear
    );

    repository
        .delete(standalone)
        .await
        .expect("standalone cleanup");
    std::fs::remove_dir_all(standalone_directory).expect("standalone media cleanup");
    cleanup(&repository, [&group]).await;
}

#[tokio::test]
async fn linked_merge_and_clear_restore_each_local_version_group() {
    let repository = repository().await;
    let links = linked_repository().await;
    let group_a = create_group(&repository, "linked-a").await;
    let group_b = create_group(&repository, "linked-b").await;
    let expected_primary = group_a.primary.min(group_b.primary);
    let linked_primary = if expected_primary == group_a.primary {
        group_b.primary
    } else {
        group_a.primary
    };

    assert_eq!(
        repository
            .merge_linked_alternate_versions(&[group_a.primary, group_b.primary])
            .await
            .expect("linked version merge"),
        expected_primary
    );
    let linked = links.list(expected_primary).await.expect("merged links");
    assert!(linked.iter().any(|link| {
        link.child_id == linked_primary
            && link.child_type == LinkedChildType::LinkedAlternateVersion
    }));
    assert!(linked.iter().any(|link| {
        link.child_type == LinkedChildType::LocalAlternateVersion
            && (link.child_id == group_a.alternates[0] || link.child_id == group_b.alternates[0])
    }));

    repository
        .clear_alternate_sources(linked_primary)
        .await
        .expect("clear linked sources through alternate");
    for group in [&group_a, &group_b] {
        let restored = load_group(&repository, group).await;
        assert_eq!(restored[0].primary_version_id, None);
        assert_eq!(restored[1].primary_version_id, Some(group.primary));
        assert_eq!(restored[2].primary_version_id, Some(group.primary));
        assert!(
            links
                .list(group.primary)
                .await
                .expect("restored local links")
                .iter()
                .all(|link| link.child_type == LinkedChildType::LocalAlternateVersion)
        );
    }

    cleanup(&repository, [&group_a, &group_b]).await;
}

#[tokio::test]
async fn merge_versions_expands_existing_groups_and_preserves_rows() {
    let repository = repository().await;
    let group = create_group(&repository, "merge-group").await;
    let (standalone, standalone_directory) =
        create_standalone_video(&repository, "merge-standalone").await;
    let mut expected_ids = group.ids().to_vec();
    expected_ids.push(standalone);
    expected_ids.sort_unstable();
    let expected_primary = expected_ids[0];

    let primary = repository
        .merge_alternate_versions(&[group.alternates[0], standalone])
        .await
        .expect("version merge");
    assert_eq!(primary, expected_primary);

    let mut merged = load_group(&repository, &group).await;
    merged.push(
        repository
            .get(standalone)
            .await
            .expect("standalone lookup")
            .expect("standalone row must remain present"),
    );
    assert_eq!(merged.len(), 4);
    for item in merged {
        if item.id == expected_primary {
            assert_eq!(item.primary_version_id, None);
        } else {
            assert_eq!(item.primary_version_id, Some(expected_primary));
        }
        assert!(
            item.path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file()),
            "merging versions must not remove media files"
        );
    }

    let before_repeat = repository
        .media_source_versions(expected_primary)
        .await
        .expect("merged versions before repeat");
    assert_eq!(
        repository
            .merge_alternate_versions(&[group.alternates[0], standalone])
            .await
            .expect("repeated version merge"),
        expected_primary
    );
    let after_repeat = repository
        .media_source_versions(expected_primary)
        .await
        .expect("merged versions after repeat");
    assert_eq!(
        after_repeat, before_repeat,
        "an idempotent merge must not change row versions"
    );
    let links = linked_repository()
        .await
        .list(expected_primary)
        .await
        .expect("local version links");
    assert_eq!(links.len(), 3);
    assert!(
        links
            .iter()
            .all(|link| link.child_type == LinkedChildType::LocalAlternateVersion)
    );

    repository
        .delete(standalone)
        .await
        .expect("standalone cleanup");
    std::fs::remove_dir_all(&standalone_directory).expect("standalone media cleanup");
    cleanup(&repository, [&group]).await;
}

fn spawn_clear(
    repository: BaseItemRepository,
    barrier: Arc<Barrier>,
    item_id: Uuid,
) -> tokio::task::JoinHandle<Result<(), BaseItemError>> {
    tokio::spawn(async move {
        barrier.wait().await;
        repository.clear_alternate_sources(item_id).await
    })
}

async fn repository() -> BaseItemRepository {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    BaseItemRepository::new(database)
}

async fn linked_repository() -> LinkedChildRepository {
    LinkedChildRepository::new(
        jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available"),
    )
}

struct VersionGroup {
    primary: Uuid,
    alternates: [Uuid; 2],
    media_directory: std::path::PathBuf,
}

impl VersionGroup {
    fn ids(&self) -> [Uuid; 3] {
        [self.primary, self.alternates[0], self.alternates[1]]
    }
}

async fn create_group(repository: &BaseItemRepository, label: &str) -> VersionGroup {
    let primary = Uuid::new_v4();
    let alternates = [Uuid::new_v4(), Uuid::new_v4()];
    let media_directory = std::env::temp_dir().join(format!("jellyfin-alt-{label}-{primary}"));
    std::fs::create_dir(&media_directory).expect("version media directory creation");
    create_item(repository, primary, label, "Movie", None, &media_directory).await;
    create_item(
        repository,
        alternates[0],
        label,
        "Video",
        Some(primary),
        &media_directory,
    )
    .await;
    repository
        .assign_local_alternate_versions(&[(alternates[0], primary), (alternates[1], primary)])
        .await
        .expect("local alternate links");
    create_item(
        repository,
        alternates[1],
        label,
        "Movie",
        Some(primary),
        &media_directory,
    )
    .await;
    VersionGroup {
        primary,
        alternates,
        media_directory,
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    id: Uuid,
    label: &str,
    item_type: &str,
    primary_version_id: Option<Uuid>,
    media_directory: &std::path::Path,
) {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(format!("{label}-{id}"));
    let media_path = media_directory.join(format!("{id}.mkv"));
    std::fs::write(&media_path, b"alternate-source fixture").expect("version media file creation");
    item.path = Some(media_path.to_string_lossy().into_owned());
    item.data = Some(json!({ "group": label, "id": id }));
    item.media_type = Some("Video".to_owned());
    item.presentation_unique_key = Some(format!("alternate-source-{label}"));
    item.primary_version_id = primary_version_id;
    repository
        .create(item)
        .await
        .expect("version item creation");
}

async fn create_standalone_video(
    repository: &BaseItemRepository,
    label: &str,
) -> (Uuid, std::path::PathBuf) {
    let id = Uuid::new_v4();
    let media_directory = std::env::temp_dir().join(format!("jellyfin-alt-{label}-{id}"));
    std::fs::create_dir(&media_directory).expect("standalone media directory creation");
    create_item(repository, id, label, "Movie", None, &media_directory).await;
    (id, media_directory)
}

async fn load_group(
    repository: &BaseItemRepository,
    group: &VersionGroup,
) -> Vec<jellyfin_data::entities::base_item::Model> {
    let mut items = Vec::new();
    for id in group.ids() {
        items.push(
            repository
                .get(id)
                .await
                .expect("version lookup")
                .expect("version row must remain present"),
        );
    }
    items
}

async fn cleanup<'a>(
    repository: &BaseItemRepository,
    groups: impl IntoIterator<Item = &'a VersionGroup>,
) {
    let groups = groups.into_iter().collect::<Vec<_>>();
    let ids = groups
        .iter()
        .copied()
        .flat_map(VersionGroup::ids)
        .collect::<Vec<_>>();
    repository
        .delete_many(&ids)
        .await
        .expect("version fixtures must clean up");
    for group in groups {
        std::fs::remove_dir_all(&group.media_directory)
            .expect("version media fixtures must clean up");
    }
}
