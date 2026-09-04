use std::path::Path;

use chrono::{DateTime, Utc};
use jellyfin_controller::{ItemImageError, ItemImageService};
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig, NewBaseItem,
    NewBaseItemImage,
};
use jellyfin_model::ImageType;
use tokio::fs;
use uuid::Uuid;

#[tokio::test]
async fn failed_remote_replacement_preserves_the_existing_image_and_file() {
    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let item_id = Uuid::new_v4();
    let mut new_item = NewBaseItem::new(item_id, "Movie");
    new_item.name = Some(format!("remote-image-replacement-{item_id}"));
    new_item.sort_name = new_item.name.clone();
    let items = BaseItemRepository::new(database.clone());
    items.create(new_item).await.expect("base item creation");

    let storage_root = std::env::temp_dir().join(format!("jellyfin-image-test-{item_id}"));
    let metadata_root = storage_root.join("metadata");
    let item_directory = item_metadata_directory(&metadata_root, item_id);
    fs::create_dir_all(&item_directory)
        .await
        .expect("item metadata directory creation");
    let old_path = item_directory.join("poster-old.jpg");
    fs::write(&old_path, b"existing-image")
        .await
        .expect("existing image write");
    let modified = fs::metadata(&old_path)
        .await
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .expect("existing image timestamp");

    let images = BaseItemImageRepository::new(database.clone());
    images
        .replace(
            item_id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: old_path.to_string_lossy().into_owned(),
                date_modified: modified,
                width: None,
                height: None,
                blurhash: None,
            }],
        )
        .await
        .expect("existing image persistence");

    let service = ItemImageService::with_storage_directories(
        database.clone(),
        storage_root.join("cache/images"),
        &metadata_root,
    );
    let result = service
        .replace_remote_image(
            item_id,
            ImageType::Primary,
            "http://127.0.0.1:1/unavailable.jpg",
        )
        .await;
    assert!(matches!(result, Err(ItemImageError::RemoteDownload(_))));

    let persisted = images
        .primary(item_id)
        .await
        .expect("primary image lookup")
        .expect("existing image must remain");
    assert_eq!(persisted.path, old_path.to_string_lossy());
    assert_eq!(
        fs::read(&old_path).await.expect("existing image read"),
        b"existing-image"
    );

    let local_media_directory = storage_root.join("media/movie");
    fs::create_dir_all(&local_media_directory)
        .await
        .expect("local media directory creation");
    let local_poster = local_media_directory.join("poster.jpg");
    fs::write(&local_poster, b"local-poster")
        .await
        .expect("local poster write");
    images
        .replace(
            item_id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: local_poster.to_string_lossy().into_owned(),
                date_modified: Utc::now(),
                width: None,
                height: None,
                blurhash: None,
            }],
        )
        .await
        .expect("local poster persistence");
    service
        .replace_remote_image(
            item_id,
            ImageType::Primary,
            "http://127.0.0.1:1/must-not-be-requested.jpg",
        )
        .await
        .expect("local media image must take precedence over a remote replacement");
    assert_eq!(
        images
            .primary(item_id)
            .await
            .expect("local primary lookup")
            .expect("local primary must remain")
            .path,
        local_poster.to_string_lossy()
    );
    assert_eq!(
        fs::read(&local_poster).await.expect("local poster read"),
        b"local-poster"
    );

    items.delete(item_id).await.expect("base item cleanup");
    fs::remove_dir_all(&storage_root)
        .await
        .expect("image storage cleanup");
    database.close().await.expect("database connection close");
}

fn item_metadata_directory(metadata_root: &Path, item_id: Uuid) -> std::path::PathBuf {
    let id = item_id.simple().to_string();
    metadata_root.join("library").join(&id[..2]).join(id)
}
