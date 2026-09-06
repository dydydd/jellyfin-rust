use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig, NewBaseItem,
    NewBaseItemImage,
};
use jellyfin_server_implementations::{
    DtoImage, DtoImageItem, DtoImageOptions, ImageCacheTagProvider,
    PersistedDtoImageProjectionService,
};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_dto_images_";

#[tokio::test]
async fn postgres_dto_images_match_official_inheritance_scenarios() {
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
        exercise_persisted_dto_images(&task_database_name).await;
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

async fn exercise_persisted_dto_images(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let items = BaseItemRepository::new(database.clone());
    let images = BaseItemImageRepository::new(database.clone());
    let service =
        PersistedDtoImageProjectionService::new(items.clone(), images.clone(), PathCacheTags);

    assert_episode_uses_season(&items, &images, &service).await;
    assert_episode_falls_back_to_series(&items, &images, &service).await;
    assert_episode_keeps_own_without_parent_images(&items, &images, &service).await;
    assert_season_inherits_series_images_from_relations(&items, &images, &service).await;
    assert_playlist_uses_display_parent(&items, &images, &service).await;
    assert_playlist_keeps_own_without_parent_image(&items, &images, &service).await;
    assert_persisted_blur_hashes(&items, &images, &service).await;
    assert_eq!(
        service
            .project(Uuid::new_v4(), DtoImageOptions::default())
            .await
            .expect("missing item lookup must succeed"),
        None
    );

    drop(service);
    drop(images);
    drop(items);
    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_persisted_blur_hashes(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let item = create_item(items, "Movie", None, None, None).await;
    images
        .replace(
            item,
            &[
                NewBaseItemImage {
                    image_type: BaseItemImageType::Primary,
                    image_index: 0,
                    path: "blurhash-primary.jpg".to_owned(),
                    date_modified: timestamp(),
                    width: None,
                    height: None,
                    blurhash: Some("primary-hash".to_owned()),
                },
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 0,
                    path: "blurhash-backdrop.jpg".to_owned(),
                    date_modified: timestamp(),
                    width: None,
                    height: None,
                    blurhash: Some("backdrop-hash".to_owned()),
                },
            ],
        )
        .await
        .expect("BlurHash images must persist");

    let projection = service
        .project(item, DtoImageOptions::default())
        .await
        .expect("BlurHash projection must succeed")
        .expect("BlurHash item must exist");
    assert_eq!(
        projection.image_blur_hashes[&jellyfin_model::ImageType::Primary],
        std::collections::HashMap::from([(
            "tag:blurhash-primary.jpg".to_owned(),
            "primary-hash".to_owned()
        )])
    );
    assert_eq!(
        projection.image_blur_hashes[&jellyfin_model::ImageType::Backdrop],
        std::collections::HashMap::from([(
            "tag:blurhash-backdrop.jpg".to_owned(),
            "backdrop-hash".to_owned()
        )])
    );

    let metadata = service
        .primary_image_metadata(&[item])
        .await
        .expect("batched primary-image metadata must load");
    assert_eq!(metadata[&item].tag, "tag:blurhash-primary.jpg");
    assert_eq!(metadata[&item].blur_hash.as_deref(), Some("primary-hash"));
}

async fn assert_episode_uses_season(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let series = create_item(items, "Series", Some(ratio(2.0 / 3.0)), None, None).await;
    let season = create_item(items, "Season", Some(ratio(2.0 / 3.0)), None, Some(series)).await;
    let episode = create_item(
        items,
        "Episode",
        Some(ratio(16.0 / 9.0)),
        Some(season),
        Some(series),
    )
    .await;
    set_primary(images, series, "series.jpg").await;
    set_primary(images, season, "season.jpg").await;
    set_primary(images, episode, "episode.jpg").await;

    let projection = service
        .project(
            episode,
            DtoImageOptions {
                include_primary_image_aspect_ratio: true,
                ..DtoImageOptions::default()
            },
        )
        .await
        .expect("persisted episode projection must succeed")
        .expect("episode must exist");

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode.jpg")
    );
    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, Some(season));
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:season.jpg")
    );
    assert_eq!(projection.primary_image_aspect_ratio, Some(16.0 / 9.0));
}

async fn assert_episode_falls_back_to_series(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let series = create_item(items, "Series", Some(ratio(2.0 / 3.0)), None, None).await;
    let season = create_item(items, "Season", Some(ratio(2.0 / 3.0)), None, Some(series)).await;
    let episode = create_item(
        items,
        "Episode",
        Some(ratio(16.0 / 9.0)),
        Some(season),
        Some(series),
    )
    .await;
    set_primary(images, series, "series-fallback.jpg").await;
    set_primary(images, episode, "episode-fallback.jpg").await;

    let projection = service
        .project(episode, DtoImageOptions::default())
        .await
        .expect("persisted episode projection must succeed")
        .expect("episode must exist");

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode-fallback.jpg")
    );
    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series-fallback.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, Some(series));
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:series-fallback.jpg")
    );
}

async fn assert_episode_keeps_own_without_parent_images(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let series = create_item(items, "Series", Some(ratio(2.0 / 3.0)), None, None).await;
    let season = create_item(items, "Season", Some(ratio(2.0 / 3.0)), None, Some(series)).await;
    let episode = create_item(
        items,
        "Episode",
        Some(ratio(16.0 / 9.0)),
        Some(season),
        Some(series),
    )
    .await;
    set_primary(images, episode, "episode-only.jpg").await;

    let projection = service
        .project(episode, DtoImageOptions::default())
        .await
        .expect("persisted episode projection must succeed")
        .expect("episode must exist");

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode-only.jpg")
    );
    assert_eq!(projection.series_primary_image_tag, None);
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.parent_primary_image_tag, None);
}

async fn assert_season_inherits_series_images_from_relations(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let series = create_item(items, "Series", Some(ratio(2.0 / 3.0)), None, None).await;
    // Keep the Season JSON empty to cover rows written before hierarchy image metadata was
    // redundantly persisted. The relational SeriesId remains authoritative.
    let season = create_item(items, "Season", None, None, Some(series)).await;
    set_images(
        images,
        series,
        &[
            (BaseItemImageType::Primary, "series-season-primary.jpg"),
            (BaseItemImageType::Logo, "series-season-logo.png"),
            (BaseItemImageType::Thumb, "series-season-thumb.jpg"),
            (BaseItemImageType::Backdrop, "series-season-backdrop.jpg"),
        ],
    )
    .await;

    let projection = service
        .project(
            season,
            DtoImageOptions {
                include_primary_image_aspect_ratio: true,
                ..DtoImageOptions::default()
            },
        )
        .await
        .expect("persisted season projection must succeed")
        .expect("season must exist");

    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series-season-primary.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.primary_image_aspect_ratio, Some(2.0 / 3.0));
    assert_eq!(projection.parent_logo_item_id, Some(series));
    assert_eq!(
        projection.parent_logo_image_tag.as_deref(),
        Some("tag:series-season-logo.png")
    );
    assert_eq!(projection.parent_thumb_item_id, Some(series));
    assert_eq!(
        projection.parent_thumb_image_tag.as_deref(),
        Some("tag:series-season-thumb.jpg")
    );
    assert_eq!(projection.parent_backdrop_image_item_id, Some(series));
    assert_eq!(
        projection.parent_backdrop_image_tags,
        ["tag:series-season-backdrop.jpg"]
    );
}

async fn assert_playlist_uses_display_parent(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let parent = create_item(items, "PlaylistsFolder", None, None, None).await;
    let user_view = create_item(
        items,
        "UserView",
        Some(json!({
            "ViewType": "playlists",
            "DisplayParentId": parent,
            "DefaultPrimaryImageAspectRatio": 1.25
        })),
        None,
        None,
    )
    .await;
    set_primary(images, parent, "/images/playlists-custom.jpg").await;
    set_primary(images, user_view, "/images/generated.png").await;

    let projection = service
        .project(
            user_view,
            DtoImageOptions {
                include_primary_image_aspect_ratio: true,
                ..DtoImageOptions::default()
            },
        )
        .await
        .expect("persisted playlist projection must succeed")
        .expect("user view must exist");

    assert_eq!(projection.primary_image_tag, None);
    assert_eq!(projection.parent_primary_image_item_id, Some(parent));
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:/images/playlists-custom.jpg")
    );
    assert_eq!(projection.primary_image_aspect_ratio, Some(1.25));
}

async fn assert_playlist_keeps_own_without_parent_image(
    items: &BaseItemRepository,
    images: &BaseItemImageRepository,
    service: &PersistedDtoImageProjectionService<PathCacheTags>,
) {
    let parent = create_item(items, "PlaylistsFolder", None, None, None).await;
    let user_view = create_item(
        items,
        "UserView",
        Some(json!({
            "ViewType": "playlists",
            "DisplayParentId": parent
        })),
        None,
        None,
    )
    .await;
    set_primary(images, user_view, "/images/generated-own.png").await;

    let projection = service
        .project(user_view, DtoImageOptions::default())
        .await
        .expect("persisted playlist projection must succeed")
        .expect("user view must exist");

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:/images/generated-own.png")
    );
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.parent_primary_image_tag, None);
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    data: Option<Value>,
    season_id: Option<Uuid>,
    series_id: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, item_type);
    item.data = data;
    item.season_id = season_id;
    item.series_id = series_id;
    repository
        .create(item)
        .await
        .expect("DTO image test item must persist");
    id
}

async fn set_primary(repository: &BaseItemImageRepository, item_id: Uuid, path: &str) {
    set_images(repository, item_id, &[(BaseItemImageType::Primary, path)]).await;
}

async fn set_images(
    repository: &BaseItemImageRepository,
    item_id: Uuid,
    images: &[(BaseItemImageType, &str)],
) {
    repository
        .replace(
            item_id,
            &images
                .iter()
                .map(|(image_type, path)| NewBaseItemImage {
                    image_type: *image_type,
                    image_index: 0,
                    path: (*path).to_owned(),
                    date_modified: timestamp(),
                    width: None,
                    height: None,
                    blurhash: None,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .expect("images must persist");
}

fn ratio(value: f64) -> Value {
    json!({ "DefaultPrimaryImageAspectRatio": value })
}

fn timestamp() -> DateTime<Utc> {
    DateTime::from_timestamp(1_768_478_400, 0).expect("test timestamp must be valid")
}

#[derive(Clone, Copy)]
struct PathCacheTags;

impl ImageCacheTagProvider for PathCacheTags {
    fn get_image_cache_tag(&self, _item: &DtoImageItem, image: &DtoImage) -> Option<String> {
        Some(format!("tag:{}", image.path))
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
