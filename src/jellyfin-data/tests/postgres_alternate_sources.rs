use std::sync::Arc;

use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, DatabaseConfig, ItemValueRepository,
    LinkedChildRepository, LinkedChildType, NewBaseItem,
    entities::{item_value, linked_child},
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
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
    assert_eq!(
        sources.iter().map(|item| item.id).collect::<Vec<_>>(),
        [
            group_a.alternates[0],
            group_a.primary,
            group_a.alternates[1],
            group_b.primary,
            group_b.alternates[0],
            group_b.alternates[1],
        ],
        "each group must retain its requested and relationship ordering in one batch"
    );
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
#[allow(clippy::too_many_lines)] // One fixture makes the root/local ordering relationships explicit.
async fn media_source_versions_follow_official_linked_and_local_relationship_order() {
    let (repository, database) = repository_and_database().await;
    let primary = create_version_item(&repository, "primary", "Primary", None).await;
    let linked_zulu =
        create_version_item(&repository, "linked-zulu", "Zulu", Some(primary.id)).await;
    let linked_alpha_later =
        create_version_item(&repository, "linked-alpha-later", "Alpha", Some(primary.id)).await;
    let linked_alpha_first =
        create_version_item(&repository, "linked-alpha-first", "Alpha", Some(primary.id)).await;
    let linked_unnamed_ids = [Uuid::new_v4(), Uuid::new_v4()];
    let linked_unnamed_later =
        create_unnamed_version_item(&repository, linked_unnamed_ids[0], Some(primary.id)).await;
    let linked_unnamed_first =
        create_unnamed_version_item(&repository, linked_unnamed_ids[1], Some(primary.id)).await;
    let primary_local_ids = [Uuid::new_v4(), Uuid::new_v4()];
    let primary_local_first_id = primary_local_ids[0].max(primary_local_ids[1]);
    let primary_local_second_id = primary_local_ids[0].min(primary_local_ids[1]);
    let primary_local_first = create_version_item_with_id(
        &repository,
        primary_local_first_id,
        "primary-local-first",
        "Primary local first",
        Some(primary.id),
    )
    .await;
    let primary_local_second = create_version_item_with_id(
        &repository,
        primary_local_second_id,
        "primary-local-second",
        "Primary local second",
        Some(primary.id),
    )
    .await;
    let zulu_local =
        create_version_item(&repository, "zulu-local", "Zulu local", Some(primary.id)).await;
    let alpha_later_local = create_version_item(
        &repository,
        "alpha-later-local",
        "Alpha later local",
        Some(primary.id),
    )
    .await;
    let alpha_first_local = create_version_item(
        &repository,
        "alpha-first-local",
        "Alpha first local",
        Some(primary.id),
    )
    .await;

    // Official Jellyfin first preserves the linked-child order and then applies a stable
    // SortName ordering to linked roots. Null names compare first, while the two unnamed and two
    // Alpha roots each retain their respective link order.
    for (child_id, sort_order) in [
        (linked_zulu.id, 0),
        (linked_alpha_later.id, 2),
        (linked_alpha_first.id, 1),
        (linked_unnamed_later.id, 4),
        (linked_unnamed_first.id, 3),
    ] {
        insert_version_link(
            &database,
            primary.id,
            child_id,
            LinkedChildType::LinkedAlternateVersion,
            sort_order,
        )
        .await;
    }
    for (parent_id, child_id, sort_order) in [
        (primary.id, primary_local_first.id, 0),
        (primary.id, primary_local_second.id, 1),
        (linked_zulu.id, zulu_local.id, 0),
        (linked_alpha_later.id, alpha_later_local.id, 0),
        (linked_alpha_first.id, alpha_first_local.id, 0),
    ] {
        insert_version_link(
            &database,
            parent_id,
            child_id,
            LinkedChildType::LocalAlternateVersion,
            sort_order,
        )
        .await;
    }

    let from_primary = version_ids(
        repository
            .media_source_versions(primary.id)
            .await
            .expect("relationship-ordered sources from primary"),
    );
    assert_eq!(
        from_primary,
        [
            primary.id,
            linked_unnamed_first.id,
            linked_unnamed_later.id,
            linked_alpha_first.id,
            linked_alpha_later.id,
            linked_zulu.id,
            primary_local_first.id,
            primary_local_second.id,
            alpha_first_local.id,
            alpha_later_local.id,
            zulu_local.id,
        ]
    );
    assert!(
        primary_local_first.id > primary_local_second.id,
        "the fixture must prove local sort_order wins over UUID order"
    );

    let from_linked_root = version_ids(
        repository
            .media_source_versions(linked_zulu.id)
            .await
            .expect("relationship-ordered sources from linked root"),
    );
    assert_eq!(
        from_linked_root,
        [
            linked_zulu.id,
            primary.id,
            linked_unnamed_first.id,
            linked_unnamed_later.id,
            linked_alpha_first.id,
            linked_alpha_later.id,
            zulu_local.id,
            primary_local_first.id,
            primary_local_second.id,
            alpha_first_local.id,
            alpha_later_local.id,
        ]
    );

    let from_local_child = version_ids(
        repository
            .media_source_versions(zulu_local.id)
            .await
            .expect("relationship-ordered sources from local child"),
    );
    assert_eq!(
        from_local_child,
        [
            zulu_local.id,
            primary.id,
            linked_unnamed_first.id,
            linked_unnamed_later.id,
            linked_alpha_first.id,
            linked_alpha_later.id,
            linked_zulu.id,
            primary_local_first.id,
            primary_local_second.id,
            alpha_first_local.id,
            alpha_later_local.id,
        ]
    );

    delete_ordering_items(
        &repository,
        &[
            primary.id,
            linked_zulu.id,
            linked_alpha_later.id,
            linked_alpha_first.id,
            linked_unnamed_later.id,
            linked_unnamed_first.id,
            primary_local_first.id,
            primary_local_second.id,
            zulu_local.id,
            alpha_later_local.id,
            alpha_first_local.id,
        ],
    )
    .await;
}

#[tokio::test]
async fn media_source_versions_use_uuid_only_for_legacy_unlinked_members() {
    let repository = repository().await;
    let primary = create_version_item(&repository, "legacy-primary", "Legacy primary", None).await;
    let legacy_ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    for (index, id) in legacy_ids.iter().copied().enumerate() {
        create_version_item_with_id(
            &repository,
            id,
            &format!("legacy-{index}"),
            &format!("Legacy {index}"),
            Some(primary.id),
        )
        .await;
    }
    let requested_id = legacy_ids[1];
    let mut remaining_ids = legacy_ids
        .into_iter()
        .filter(|id| *id != requested_id)
        .collect::<Vec<_>>();
    remaining_ids.sort_unstable();

    let sources = version_ids(
        repository
            .media_source_versions(requested_id)
            .await
            .expect("legacy media-source versions"),
    );
    assert_eq!(sources[0], requested_id);
    assert_eq!(sources[1], primary.id);
    assert_eq!(&sources[2..], remaining_ids);

    let mut cleanup_ids = vec![primary.id];
    cleanup_ids.extend(legacy_ids);
    delete_ordering_items(&repository, &cleanup_ids).await;
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
async fn local_alternate_assignments_number_each_parent_in_input_order() {
    let repository = repository().await;
    let links = linked_repository().await;
    let parent_a = create_ordering_item(&repository, "ordered-parent-a").await;
    let parent_b = create_ordering_item(&repository, "ordered-parent-b").await;
    let children_a = [
        create_ordering_item(&repository, "ordered-child-a1").await,
        create_ordering_item(&repository, "ordered-child-a2").await,
    ];
    let children_b = [
        create_ordering_item(&repository, "ordered-child-b1").await,
        create_ordering_item(&repository, "ordered-child-b2").await,
    ];

    repository
        .assign_local_alternate_versions(&[
            (children_a[0].id, parent_a.id),
            (children_b[0].id, parent_b.id),
            (children_a[1].id, parent_a.id),
            (children_b[1].id, parent_b.id),
        ])
        .await
        .expect("ordered local alternate assignments");

    assert_eq!(
        local_link_order(&links, parent_a.id).await,
        vec![(children_a[0].id, Some(0)), (children_a[1].id, Some(1))]
    );
    assert_eq!(
        local_link_order(&links, parent_b.id).await,
        vec![(children_b[0].id, Some(0)), (children_b[1].id, Some(1))]
    );
    for (child_id, parent_id) in [
        (children_a[0].id, parent_a.id),
        (children_a[1].id, parent_a.id),
        (children_b[0].id, parent_b.id),
        (children_b[1].id, parent_b.id),
    ] {
        assert_eq!(
            repository
                .get(child_id)
                .await
                .expect("assigned child lookup")
                .expect("assigned child")
                .primary_version_id,
            Some(parent_id)
        );
    }

    delete_ordering_items(
        &repository,
        &[
            parent_a.id,
            parent_b.id,
            children_a[0].id,
            children_a[1].id,
            children_b[0].id,
            children_b[1].id,
        ],
    )
    .await;
}

#[tokio::test]
async fn local_alternate_assignments_keep_the_first_valid_parent_per_child() {
    let repository = repository().await;
    let links = linked_repository().await;
    let parent_a = create_ordering_item(&repository, "first-parent-a").await;
    let parent_b = create_ordering_item(&repository, "first-parent-b").await;
    let child = create_ordering_item(&repository, "first-child").await;

    repository
        .assign_local_alternate_versions(&[
            (child.id, Uuid::new_v4()),
            (child.id, child.id),
            (child.id, parent_a.id),
            (child.id, parent_b.id),
        ])
        .await
        .expect("first valid local alternate assignment");

    assert_eq!(
        local_link_order(&links, parent_a.id).await,
        vec![(child.id, Some(0))]
    );
    assert!(local_link_order(&links, parent_b.id).await.is_empty());
    assert_eq!(
        repository
            .get(child.id)
            .await
            .expect("first-wins child lookup")
            .expect("first-wins child")
            .primary_version_id,
        Some(parent_a.id)
    );

    delete_ordering_items(&repository, &[parent_a.id, parent_b.id, child.id]).await;
}

#[tokio::test]
async fn local_alternate_reassignment_rewrites_order_and_compacts_old_parent() {
    let repository = repository().await;
    let links = linked_repository().await;
    let parent_a = create_ordering_item(&repository, "reorder-parent-a").await;
    let parent_b = create_ordering_item(&repository, "reorder-parent-b").await;
    let child_a = create_ordering_item(&repository, "reorder-child-a").await;
    let child_b = create_ordering_item(&repository, "reorder-child-b").await;
    let child_c = create_ordering_item(&repository, "reorder-child-c").await;

    repository
        .assign_local_alternate_versions(&[
            (child_a.id, parent_a.id),
            (child_b.id, parent_a.id),
            (child_c.id, parent_b.id),
        ])
        .await
        .expect("initial local alternate order");
    assert_eq!(
        repository
            .assign_local_alternate_versions(&[
                (child_b.id, parent_a.id),
                (child_a.id, parent_a.id),
            ])
            .await
            .expect("link-only local alternate reorder"),
        vec![parent_a.id],
        "a link-only reorder must report the parent whose DTO-visible order changed"
    );
    assert_eq!(
        local_link_order(&links, parent_a.id).await,
        vec![(child_b.id, Some(0)), (child_a.id, Some(1))]
    );
    let changed = repository
        .assign_local_alternate_versions(&[(child_c.id, parent_b.id), (child_b.id, parent_b.id)])
        .await
        .expect("reordered local alternates");
    assert_eq!(
        changed
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        [parent_a.id, parent_b.id, child_b.id].into_iter().collect()
    );

    assert_eq!(
        local_link_order(&links, parent_a.id).await,
        vec![(child_a.id, Some(0))]
    );
    assert_eq!(
        local_link_order(&links, parent_b.id).await,
        vec![(child_c.id, Some(0)), (child_b.id, Some(1))]
    );
    assert_eq!(
        repository
            .get(child_b.id)
            .await
            .expect("reassigned child lookup")
            .expect("reassigned child")
            .primary_version_id,
        Some(parent_b.id)
    );

    delete_ordering_items(
        &repository,
        &[parent_a.id, parent_b.id, child_a.id, child_b.id, child_c.id],
    )
    .await;
}

#[tokio::test]
async fn repeated_local_alternate_assignments_are_idempotent() {
    let repository = repository().await;
    let links = linked_repository().await;
    let parent = create_ordering_item(&repository, "repeat-parent").await;
    let child_a = create_ordering_item(&repository, "repeat-child-a").await;
    let child_b = create_ordering_item(&repository, "repeat-child-b").await;
    let assignments = [(child_a.id, parent.id), (child_b.id, parent.id)];

    repository
        .assign_local_alternate_versions(&assignments)
        .await
        .expect("initial repeated assignments");
    let links_before = links.list(parent.id).await.expect("links before repeat");
    let children_before = [
        repository
            .get(child_a.id)
            .await
            .expect("first child before repeat")
            .expect("first child"),
        repository
            .get(child_b.id)
            .await
            .expect("second child before repeat")
            .expect("second child"),
    ];

    assert!(
        repository
            .assign_local_alternate_versions(&assignments)
            .await
            .expect("idempotent repeated assignments")
            .is_empty()
    );
    assert_eq!(
        links.list(parent.id).await.expect("links after repeat"),
        links_before
    );
    assert!(
        repository
            .assign_local_alternate_versions(&[(child_a.id, parent.id)])
            .await
            .expect("idempotent partial repeated assignment")
            .is_empty()
    );
    assert_eq!(
        links
            .list(parent.id)
            .await
            .expect("links after partial repeat"),
        links_before,
        "a partial scan batch must not move an existing child to the end"
    );
    assert_eq!(
        [
            repository
                .get(child_a.id)
                .await
                .expect("first child after repeat")
                .expect("first child"),
            repository
                .get(child_b.id)
                .await
                .expect("second child after repeat")
                .expect("second child"),
        ],
        children_before
    );

    delete_ordering_items(&repository, &[parent.id, child_a.id, child_b.id]).await;
}

#[tokio::test]
async fn opposing_local_alternate_reassignments_complete_without_deadlock() {
    let repository = repository().await;
    let links = linked_repository().await;
    let parent_a = create_ordering_item(&repository, "opposing-parent-a").await;
    let parent_b = create_ordering_item(&repository, "opposing-parent-b").await;
    let child_a = create_ordering_item(&repository, "opposing-child-a").await;
    let child_b = create_ordering_item(&repository, "opposing-child-b").await;
    repository
        .assign_local_alternate_versions(&[(child_a.id, parent_a.id), (child_b.id, parent_b.id)])
        .await
        .expect("initial opposing assignments");

    let barrier = Arc::new(Barrier::new(3));
    let move_a = spawn_assignment(
        repository.clone(),
        Arc::clone(&barrier),
        child_a.id,
        parent_b.id,
    );
    let move_b = spawn_assignment(
        repository.clone(),
        Arc::clone(&barrier),
        child_b.id,
        parent_a.id,
    );
    barrier.wait().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        move_a
            .await
            .expect("first opposing assignment task")
            .expect("first opposing assignment");
        move_b
            .await
            .expect("second opposing assignment task")
            .expect("second opposing assignment");
    })
    .await
    .expect("opposing assignments must not deadlock");

    assert_eq!(
        local_link_order(&links, parent_a.id).await,
        vec![(child_b.id, Some(0))]
    );
    assert_eq!(
        local_link_order(&links, parent_b.id).await,
        vec![(child_a.id, Some(0))]
    );
    for (child_id, parent_id) in [(child_a.id, parent_b.id), (child_b.id, parent_a.id)] {
        assert_eq!(
            repository
                .get(child_id)
                .await
                .expect("opposing child lookup")
                .expect("opposing child")
                .primary_version_id,
            Some(parent_id)
        );
    }

    delete_ordering_items(
        &repository,
        &[parent_a.id, parent_b.id, child_a.id, child_b.id],
    )
    .await;
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
    assert_eq!(
        linked
            .iter()
            .map(|link| link.sort_order)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)],
        "official alternate relationships share one non-null parent order"
    );
    let links_before_repeat = linked.clone();
    assert_eq!(
        repository
            .merge_linked_alternate_versions(&[group_a.primary, group_b.primary])
            .await
            .expect("repeated linked version merge"),
        expected_primary
    );
    assert_eq!(
        links
            .list(expected_primary)
            .await
            .expect("links after repeated merge"),
        links_before_repeat,
        "repeating a merge must preserve relationship order"
    );

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
    assert_eq!(
        links.iter().map(|link| link.sort_order).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)],
        "scan-created relationships must be continuously ordered"
    );

    repository
        .delete(standalone)
        .await
        .expect("standalone cleanup");
    std::fs::remove_dir_all(&standalone_directory).expect("standalone media cleanup");
    cleanup(&repository, [&group]).await;
}

#[tokio::test]
async fn linked_merge_orders_inferred_legacy_local_relationships() {
    let repository = repository().await;
    let links = linked_repository().await;
    let legacy_root = create_ordering_item(&repository, "legacy-linked-root").await;
    let legacy_child = create_version_item(
        &repository,
        "legacy-linked-child",
        "Legacy linked child",
        Some(legacy_root.id),
    )
    .await;
    let standalone = create_ordering_item(&repository, "legacy-linked-standalone").await;

    repository
        .merge_linked_alternate_versions(&[legacy_root.id, standalone.id])
        .await
        .expect("merge group containing a legacy back-reference");

    assert_eq!(
        local_link_order(&links, legacy_root.id).await,
        vec![(legacy_child.id, Some(0))],
        "a recovered local subgroup must receive an official non-null order"
    );

    delete_ordering_items(
        &repository,
        &[legacy_root.id, legacy_child.id, standalone.id],
    )
    .await;
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

fn spawn_assignment(
    repository: BaseItemRepository,
    barrier: Arc<Barrier>,
    child_id: Uuid,
    parent_id: Uuid,
) -> tokio::task::JoinHandle<Result<Vec<Uuid>, BaseItemError>> {
    tokio::spawn(async move {
        barrier.wait().await;
        repository
            .assign_local_alternate_versions(&[(child_id, parent_id)])
            .await
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

async fn repository_and_database() -> (BaseItemRepository, DatabaseConnection) {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    (BaseItemRepository::new(database.clone()), database)
}

async fn linked_repository() -> LinkedChildRepository {
    LinkedChildRepository::new(
        jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available"),
    )
}

async fn create_ordering_item(
    repository: &BaseItemRepository,
    label: &str,
) -> jellyfin_data::entities::base_item::Model {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(format!("{label}-{id}"));
    item.sort_name = item.name.clone();
    item.media_type = Some("Video".to_owned());
    repository
        .create(item)
        .await
        .expect("ordering fixture item")
}

async fn create_version_item(
    repository: &BaseItemRepository,
    label: &str,
    sort_name: &str,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    create_version_item_with_id(
        repository,
        Uuid::new_v4(),
        label,
        sort_name,
        primary_version_id,
    )
    .await
}

async fn create_version_item_with_id(
    repository: &BaseItemRepository,
    id: Uuid,
    label: &str,
    sort_name: &str,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, "Movie");
    item.name = Some(label.to_owned());
    item.sort_name = Some(sort_name.to_owned());
    item.media_type = Some("Video".to_owned());
    item.primary_version_id = primary_version_id;
    repository
        .create(item)
        .await
        .expect("version ordering fixture item")
}

async fn create_unnamed_version_item(
    repository: &BaseItemRepository,
    id: Uuid,
    primary_version_id: Option<Uuid>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(id, "Movie");
    item.media_type = Some("Video".to_owned());
    item.primary_version_id = primary_version_id;
    repository
        .create(item)
        .await
        .expect("unnamed version ordering fixture item")
}

async fn insert_version_link(
    database: &DatabaseConnection,
    parent_id: Uuid,
    child_id: Uuid,
    child_type: LinkedChildType,
    sort_order: i32,
) {
    linked_child::ActiveModel {
        parent_id: Set(parent_id),
        child_id: Set(child_id),
        child_type: Set(child_type as i16),
        sort_order: Set(Some(sort_order)),
    }
    .insert(database)
    .await
    .expect("version relationship fixture");
}

fn version_ids(items: Vec<jellyfin_data::entities::base_item::Model>) -> Vec<Uuid> {
    items.into_iter().map(|item| item.id).collect()
}

async fn local_link_order(
    links: &LinkedChildRepository,
    parent_id: Uuid,
) -> Vec<(Uuid, Option<i32>)> {
    links
        .list(parent_id)
        .await
        .expect("ordered local links")
        .into_iter()
        .filter(|link| link.child_type == LinkedChildType::LocalAlternateVersion)
        .map(|link| (link.child_id, link.sort_order))
        .collect()
}

async fn delete_ordering_items(repository: &BaseItemRepository, ids: &[Uuid]) {
    repository
        .delete_many(ids)
        .await
        .expect("ordering fixture cleanup");
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
