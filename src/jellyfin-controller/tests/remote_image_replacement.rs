use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

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
async fn concurrent_remote_image_downloads_share_one_upstream_request() {
    const DOWNLOAD_COUNT: usize = 8;

    let database = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let items = BaseItemRepository::new(database.clone());
    let mut item_ids = Vec::with_capacity(DOWNLOAD_COUNT);
    for index in 0..DOWNLOAD_COUNT {
        let item_id = Uuid::new_v4();
        let mut item = NewBaseItem::new(item_id, "Movie");
        item.name = Some(format!("remote-image-singleflight-{index}-{item_id}"));
        item.sort_name.clone_from(&item.name);
        items.create(item).await.expect("base item creation");
        item_ids.push(item_id);
    }

    let storage_root =
        std::env::temp_dir().join(format!("jellyfin-image-singleflight-{}", Uuid::new_v4()));
    let image_bytes = b"shared-remote-image".to_vec();
    let upstream = CountingImageServer::start(image_bytes.clone());
    let service = ItemImageService::with_storage_directories(
        database.clone(),
        storage_root.join("cache/images"),
        storage_root.join("metadata"),
    );
    let start = Arc::new(tokio::sync::Barrier::new(DOWNLOAD_COUNT + 1));
    let mut downloads = Vec::with_capacity(DOWNLOAD_COUNT);
    for item_id in &item_ids {
        let service = service.clone();
        let start = Arc::clone(&start);
        let url = upstream.url.clone();
        let item_id = *item_id;
        downloads.push(tokio::spawn(async move {
            start.wait().await;
            service
                .download_remote_image(item_id, ImageType::Primary, &url)
                .await
        }));
    }
    start.wait().await;
    for download in downloads {
        download
            .await
            .expect("remote image download task")
            .expect("remote image download");
    }

    assert_eq!(upstream.stop(), 1, "same URL must only be fetched once");
    let images = BaseItemImageRepository::new(database.clone());
    for item_id in &item_ids {
        let image = images
            .primary(*item_id)
            .await
            .expect("primary image lookup")
            .expect("downloaded primary image");
        assert_eq!(
            fs::read(image.path).await.expect("downloaded image read"),
            image_bytes
        );
    }

    for item_id in item_ids {
        items.delete(item_id).await.expect("base item cleanup");
    }
    fs::remove_dir_all(&storage_root)
        .await
        .expect("image storage cleanup");
    database.close().await.expect("database connection close");
}

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

struct CountingImageServer {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

impl CountingImageServer {
    fn start(bytes: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock image server bind");
        listener
            .set_nonblocking(true)
            .expect("mock image server nonblocking mode");
        let address = listener.local_addr().expect("mock image server address");
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let server_requests = Arc::clone(&requests);
        let server_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        server_requests.fetch_add(1, Ordering::AcqRel);
                        let mut request = [0_u8; 2048];
                        let _ = stream.read(&mut request);
                        thread::sleep(Duration::from_millis(150));
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        )
                        .expect("mock image response headers");
                        stream.write_all(&bytes).expect("mock image response body");
                        stream.flush().expect("mock image response flush");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock image server accept failed: {error}"),
                }
            }
        });
        Self {
            url: format!("http://{address}/shared.jpg"),
            requests,
            stop,
            thread,
        }
    }

    fn stop(self) -> usize {
        self.stop.store(true, Ordering::Release);
        self.thread.join().expect("mock image server thread");
        self.requests.load(Ordering::Acquire)
    }
}
