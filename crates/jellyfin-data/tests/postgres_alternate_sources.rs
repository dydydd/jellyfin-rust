use std::sync::Arc;

use jellyfin_data::{BaseItemError, BaseItemRepository, DatabaseConfig, NewBaseItem};
use serde_json::json;
use tokio::sync::Barrier;
use uuid::Uuid;

#[tokio::test]
async fn clear_from_primary_or_alternate_is_atomic_and_preserves_rows() {
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
        assert_eq!(after.primary_version_id, None);
        assert_eq!(after.row_version, before.row_version + 1);
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
        .expect("primary entry point must clear its complete group");
    assert!(
        load_group(&repository, &group_b)
            .await
            .iter()
            .all(|item| item.primary_version_id.is_none())
    );

    cleanup(&repository, [&group_a, &group_b]).await;
}

#[tokio::test]
async fn concurrent_clears_are_idempotent_and_never_leave_a_partial_group() {
    let repository = repository().await;
    let group = create_group(&repository, "concurrent").await;
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
    assert!(
        after_concurrent_clear
            .iter()
            .all(|item| item.primary_version_id.is_none())
    );
    for id in group.ids() {
        repository
            .clear_alternate_sources(id)
            .await
            .expect("repeated clear must remain successful");
    }
    assert!(
        load_group(&repository, &group)
            .await
            .iter()
            .all(|item| item.primary_version_id.is_none())
    );

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
