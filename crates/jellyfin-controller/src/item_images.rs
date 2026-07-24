use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageType,
    NewBaseItemImage, entities::base_item,
};
use jellyfin_drawing::inspect_dimensions;
use jellyfin_model::{ImageInfo, ImageType};
use md5::{Digest, Md5};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

const DOTNET_UNIX_EPOCH_TICKS: i128 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i128 = 10_000_000;
const MAX_REMOTE_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const REMOTE_IMAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Failure while loading an item's persisted image metadata.
#[derive(Debug, Error)]
pub enum ItemImageError {
    #[error("item image was not found")]
    NotFound,
    #[error("remote image URL is invalid")]
    InvalidRemoteUrl,
    #[error("remote image exceeded the download limit")]
    RemoteImageTooLarge,
    #[error("remote image download failed")]
    RemoteDownload(#[source] reqwest::Error),
    #[error("item image file operation failed")]
    Io(#[from] std::io::Error),
    #[error("the requested item image type cannot be uploaded")]
    UnsupportedImageType,
    #[error("the requested item image type does not support index changes")]
    UnsupportedIndexChange,
    #[error(transparent)]
    Store(#[from] BaseItemImageStoreError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemImageResource {
    pub path: PathBuf,
    pub date_modified: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Projects PostgreSQL-backed item images onto Jellyfin's public image DTO.
#[derive(Clone)]
pub struct ItemImageService {
    images: BaseItemImageRepository,
    http: reqwest::Client,
    cache_directory: PathBuf,
    internal_metadata_directory: PathBuf,
}

impl ItemImageService {
    #[must_use]
    pub fn new(database: sea_orm::DatabaseConnection) -> Self {
        Self::with_storage_directories(
            database,
            PathBuf::from("cache").join("images"),
            PathBuf::from("metadata"),
        )
    }

    #[must_use]
    pub fn with_cache_directory(
        database: sea_orm::DatabaseConnection,
        cache_directory: impl Into<PathBuf>,
    ) -> Self {
        Self::with_storage_directories(database, cache_directory, PathBuf::from("metadata"))
    }

    #[must_use]
    pub fn with_storage_directories(
        database: sea_orm::DatabaseConnection,
        cache_directory: impl Into<PathBuf>,
        internal_metadata_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            images: BaseItemImageRepository::new(database),
            http: reqwest::Client::builder()
                .timeout(REMOTE_IMAGE_TIMEOUT)
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "could not configure the item image HTTP client");
                    reqwest::Client::new()
                }),
            cache_directory: cache_directory.into(),
            internal_metadata_directory: internal_metadata_directory.into(),
        }
    }

    /// Lists one item's images in Jellyfin's single-image-then-multiple order.
    ///
    /// Local file metadata is best-effort, matching the official endpoint: a
    /// missing or inaccessible file still produces an image record with zero
    /// size and no dimensions or blur hash.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when image metadata cannot be loaded.
    pub async fn list(&self, item: &base_item::Model) -> Result<Vec<ImageInfo>, ItemImageError> {
        let images = self.images.list(item.id).await?;
        let (single_images, multiple_images): (Vec<_>, Vec<_>) = images
            .into_iter()
            .partition(|image| !allows_multiple_images(image.image_type));
        let mut backdrop_index = 0_i32;
        let mut chapter_index = 0_i32;
        let mut infos = Vec::with_capacity(single_images.len() + multiple_images.len());

        for image in single_images.into_iter().chain(multiple_images) {
            let image_index =
                public_image_index(image.image_type, &mut backdrop_index, &mut chapter_index);
            infos.push(project_image(item.path.as_deref(), image, image_index).await);
        }

        Ok(infos)
    }

    /// Resolves one image by its public zero-based ordinal.
    ///
    /// Remote image stubs are downloaded once, moved into the image cache, and
    /// conditionally persisted before the local resource is returned.
    ///
    /// # Errors
    ///
    /// Returns not-found, download, file-system, or persistence errors.
    pub async fn resource(
        &self,
        item_id: Uuid,
        image_type: ImageType,
        image_index: u32,
    ) -> Result<ItemImageResource, ItemImageError> {
        let stored_type = persisted_image_type(image_type);
        let image = self
            .images
            .at(item_id, stored_type, u64::from(image_index))
            .await?
            .ok_or(ItemImageError::NotFound)?;
        let image = if is_remote_path(&image.path) {
            self.materialize_remote(image).await?
        } else {
            image
        };
        Ok(ItemImageResource {
            path: PathBuf::from(image.path),
            date_modified: image.date_modified,
            width: image.width,
            height: image.height,
        })
    }

    /// Deletes one image by public ordinal and removes a local backing file.
    ///
    /// Missing images are idempotent. Remote paths only lose their persisted
    /// metadata because they are not local file-system targets.
    ///
    /// # Errors
    ///
    /// Returns missing-item or persistence errors. File cleanup failures are
    /// logged after the durable metadata delete and do not change the result.
    pub async fn delete(
        &self,
        item_id: Uuid,
        image_type: ImageType,
        image_index: u32,
    ) -> Result<(), ItemImageError> {
        let image = self
            .images
            .delete_at(
                item_id,
                persisted_image_type(image_type),
                u64::from(image_index),
            )
            .await
            .map_err(|error| match error {
                BaseItemImageStoreError::BaseItemNotFound { .. } => ItemImageError::NotFound,
                error => ItemImageError::Store(error),
            })?;
        let Some(image) = image else {
            return Ok(());
        };
        if !is_remote_path(&image.path) {
            match fs::remove_file(&image.path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(
                    path = %image.path,
                    %error,
                    "image metadata was deleted but its local file could not be removed"
                ),
            }
        }
        Ok(())
    }

    /// Persists a Base64-decoded item image in the item's internal metadata directory.
    ///
    /// Single-image types replace index zero while backdrops append. Dimension
    /// inspection is best-effort: unsupported or malformed image bytes remain
    /// stored to match Jellyfin's upload behavior.
    ///
    /// # Errors
    ///
    /// Returns unsupported-type, missing-item, file-system, or persistence errors.
    pub async fn upload(
        &self,
        item_id: Uuid,
        image_type: ImageType,
        extension: &str,
        bytes: &[u8],
    ) -> Result<(), ItemImageError> {
        let image_type = persisted_image_type(image_type);
        if image_type == BaseItemImageType::Chapter {
            return Err(ItemImageError::UnsupportedImageType);
        }
        let directory = self.item_metadata_directory(item_id);
        fs::create_dir_all(&directory).await?;
        let stem = upload_file_stem(image_type);
        let unique = Uuid::new_v4().simple();
        let target = directory.join(format!("{stem}-{unique}{extension}"));
        let temporary = directory.join(format!(".{stem}-{unique}.tmp"));
        let write_result = async {
            let mut file = fs::File::create(&temporary).await?;
            file.write_all(bytes).await?;
            file.sync_all().await?;
            drop(file);
            fs::rename(&temporary, &target).await
        }
        .await;
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary).await;
        }
        write_result?;

        let modified = match fs::metadata(&target)
            .await
            .and_then(|metadata| metadata.modified())
        {
            Ok(modified) => DateTime::<Utc>::from(modified),
            Err(error) => {
                let _ = fs::remove_file(&target).await;
                return Err(ItemImageError::Io(error));
            }
        };
        let dimensions = inspect_dimensions(&target).await.ok();
        let image = NewBaseItemImage {
            image_type,
            image_index: 0,
            path: target.to_string_lossy().into_owned(),
            date_modified: modified,
            width: dimensions.map(|(width, _)| width),
            height: dimensions.map(|(_, height)| height),
            blurhash: None,
        };
        let mutation = match self.images.set_or_append(item_id, image).await {
            Ok(mutation) => mutation,
            Err(error) => {
                let _ = fs::remove_file(&target).await;
                return Err(match error {
                    BaseItemImageStoreError::BaseItemNotFound { .. } => ItemImageError::NotFound,
                    BaseItemImageStoreError::UnsupportedUploadImageType { .. } => {
                        ItemImageError::UnsupportedImageType
                    }
                    error => ItemImageError::Store(error),
                });
            }
        };
        if let Some(previous) = mutation.replaced {
            let previous_path = Path::new(&previous.path);
            if previous_path != target && previous_path.parent() == Some(directory.as_path()) {
                match fs::remove_file(previous_path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => tracing::warn!(
                        path = %previous.path,
                        %error,
                        "item image was replaced but its previous managed file could not be removed"
                    ),
                }
            }
        }
        Ok(())
    }

    /// Swaps the file contents addressed by two public image ordinals.
    ///
    /// Missing images and remote paths are official-compatible no-ops. Local
    /// replacements are staged beside each destination, so the operation also
    /// works when the two images reside on different file systems.
    ///
    /// # Errors
    ///
    /// Returns unsupported-type, missing-item, file-system, or persistence errors.
    pub async fn swap(
        &self,
        item_id: Uuid,
        image_type: ImageType,
        first_ordinal: i64,
        second_ordinal: i64,
    ) -> Result<(), ItemImageError> {
        let swap = self
            .images
            .begin_swap(
                item_id,
                persisted_image_type(image_type),
                first_ordinal,
                second_ordinal,
            )
            .await
            .map_err(|error| match error {
                BaseItemImageStoreError::BaseItemNotFound { .. } => ItemImageError::NotFound,
                BaseItemImageStoreError::UnsupportedSwapImageType { .. } => {
                    ItemImageError::UnsupportedIndexChange
                }
                error => ItemImageError::Store(error),
            })?;
        let Some(swap) = swap else {
            return Ok(());
        };
        if is_remote_path(&swap.first.path) || is_remote_path(&swap.second.path) {
            return Ok(());
        }
        let first_path = PathBuf::from(&swap.first.path);
        let second_path = PathBuf::from(&swap.second.path);
        let first = first_path.as_path();
        let second = second_path.as_path();
        if first == second {
            return Err(ItemImageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cannot swap an image file with itself",
            )));
        }
        let token = Uuid::new_v4().simple();
        let first_stage = sibling_temporary(first, "swap-in", token)?;
        let second_stage = sibling_temporary(second, "swap-in", token)?;
        let first_backup = sibling_temporary(first, "swap-backup", token)?;
        let second_backup = sibling_temporary(second, "swap-backup", token)?;
        let staged = async {
            copy_and_sync(second, &first_stage).await?;
            copy_and_sync(first, &second_stage).await?;
            copy_and_sync(first, &first_backup).await?;
            copy_and_sync(second, &second_backup).await
        }
        .await;
        if let Err(error) = staged {
            cleanup_files([&first_stage, &second_stage, &first_backup, &second_backup]).await;
            return Err(ItemImageError::Io(error));
        }

        if let Err(error) = copy_and_sync(&first_stage, first).await {
            cleanup_files([&first_stage, &second_stage, &first_backup, &second_backup]).await;
            return Err(ItemImageError::Io(error));
        }
        if let Err(error) = copy_and_sync(&second_stage, second).await {
            let _ = copy_and_sync(&first_backup, first).await;
            cleanup_files([&first_stage, &second_stage, &first_backup, &second_backup]).await;
            return Err(ItemImageError::Io(error));
        }
        cleanup_files([&first_stage, &second_stage]).await;

        let metadata = async {
            let first_modified = fs::metadata(first).await?.modified()?;
            let second_modified = fs::metadata(second).await?.modified()?;
            swap.commit(first_modified.into(), second_modified.into())
                .await
                .map_err(ItemImageError::Store)
        }
        .await;
        if let Err(error) = metadata {
            let _ = copy_and_sync(&first_backup, first).await;
            let _ = copy_and_sync(&second_backup, second).await;
            cleanup_files([&first_backup, &second_backup]).await;
            return Err(error);
        }
        cleanup_files([&first_backup, &second_backup]).await;
        Ok(())
    }

    fn item_metadata_directory(&self, item_id: Uuid) -> PathBuf {
        let id = item_id.simple().to_string();
        self.internal_metadata_directory
            .join("library")
            .join(&id[..2])
            .join(id)
    }

    async fn materialize_remote(
        &self,
        image: BaseItemImage,
    ) -> Result<BaseItemImage, ItemImageError> {
        let url = reqwest::Url::parse(&image.path).map_err(|_| ItemImageError::InvalidRemoteUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ItemImageError::InvalidRemoteUrl);
        }
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(ItemImageError::RemoteDownload)?
            .error_for_status()
            .map_err(ItemImageError::RemoteDownload)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_REMOTE_IMAGE_BYTES)
        {
            return Err(ItemImageError::RemoteImageTooLarge);
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or_default(),
        );
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(ItemImageError::RemoteDownload)?
        {
            let next_length = bytes.len().saturating_add(chunk.len());
            if u64::try_from(next_length).unwrap_or(u64::MAX) > MAX_REMOTE_IMAGE_BYTES {
                return Err(ItemImageError::RemoteImageTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }

        let extension = jellyfin_model::MimeTypes::try_get_image_extension(content_type.as_deref())
            .or_else(|| {
                Path::new(image.path.split('?').next().unwrap_or_default())
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| format!(".{extension}"))
            })
            .unwrap_or_else(|| ".img".to_owned());
        let target_directory = self.cache_directory.join("remote");
        fs::create_dir_all(&target_directory).await?;
        let source_key = format!("{:x}", Md5::digest(image.path.as_bytes()));
        let file_name = format!(
            "{}-{}-{}-{source_key}{}",
            image.item_id.simple(),
            image.image_type.as_i16(),
            image.image_index,
            extension
        );
        let target = target_directory.join(file_name);
        let temporary = target.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
        let write_result = async {
            let mut file = fs::File::create(&temporary).await?;
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            drop(file);
            fs::rename(&temporary, &target).await
        }
        .await;
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary).await;
        }
        write_result?;
        let modified = fs::metadata(&target)
            .await?
            .modified()
            .map(DateTime::<Utc>::from)?;
        let target = target.to_string_lossy().into_owned();
        if let Some(relocated) = self
            .images
            .relocate_if_path_matches(&image, &target, modified)
            .await?
        {
            return Ok(relocated);
        }
        self.images
            .get(image.item_id, image.image_type, image.image_index)
            .await?
            .ok_or(ItemImageError::NotFound)
    }
}

fn sibling_temporary(
    path: &Path,
    purpose: &str,
    token: impl std::fmt::Display,
) -> Result<PathBuf, ItemImageError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ItemImageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "image path has no UTF-8 file name",
            ))
        })?;
    Ok(path.with_file_name(format!(".{name}-{purpose}-{token}.tmp")))
}

async fn copy_and_sync(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::copy(source, target).await?;
    fs::OpenOptions::new()
        .write(true)
        .open(target)
        .await?
        .sync_all()
        .await
}

async fn cleanup_files<const N: usize>(paths: [&PathBuf; N]) {
    for path in paths {
        let _ = fs::remove_file(path).await;
    }
}

fn is_remote_path(path: &str) -> bool {
    path.get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http"))
}

const fn allows_multiple_images(image_type: BaseItemImageType) -> bool {
    matches!(
        image_type,
        BaseItemImageType::Backdrop | BaseItemImageType::Chapter
    )
}

const fn upload_file_stem(image_type: BaseItemImageType) -> &'static str {
    match image_type {
        BaseItemImageType::Primary => "poster",
        BaseItemImageType::Art => "clearart",
        BaseItemImageType::BoxRear => "back",
        BaseItemImageType::Thumb => "landscape",
        BaseItemImageType::Disc => "disc",
        BaseItemImageType::Backdrop => "backdrop",
        BaseItemImageType::Banner => "banner",
        BaseItemImageType::Logo => "logo",
        BaseItemImageType::Box => "box",
        BaseItemImageType::Screenshot => "screenshot",
        BaseItemImageType::Menu => "menu",
        BaseItemImageType::Profile => "profile",
        BaseItemImageType::Chapter => "chapter",
    }
}

async fn project_image(
    item_path: Option<&str>,
    image: BaseItemImage,
    image_index: Option<i32>,
) -> ImageInfo {
    let (size, width, height, blur_hash) = local_image_metadata(&image).await;
    ImageInfo {
        image_type: model_image_type(image.image_type),
        image_index,
        image_tag: image_cache_tag(item_path.unwrap_or_default(), image.date_modified),
        path: image.path,
        blur_hash,
        height,
        width,
        size,
    }
}

async fn local_image_metadata(
    image: &BaseItemImage,
) -> (i64, Option<i32>, Option<i32>, Option<String>) {
    if image
        .path
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http"))
    {
        return (0, None, None, None);
    }

    let Ok(metadata) = tokio::fs::metadata(&image.path).await else {
        return (0, None, None, None);
    };
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    let width = image.width.and_then(|value| i32::try_from(value).ok());
    let height = image.height.and_then(|value| i32::try_from(value).ok());
    (size, width, height, image.blurhash.clone())
}

fn public_image_index(
    image_type: BaseItemImageType,
    backdrop_index: &mut i32,
    chapter_index: &mut i32,
) -> Option<i32> {
    match image_type {
        BaseItemImageType::Backdrop => Some(take_index(backdrop_index)),
        BaseItemImageType::Chapter => Some(take_index(chapter_index)),
        BaseItemImageType::Primary
        | BaseItemImageType::Art
        | BaseItemImageType::Banner
        | BaseItemImageType::Logo
        | BaseItemImageType::Thumb
        | BaseItemImageType::Disc
        | BaseItemImageType::Box
        | BaseItemImageType::Screenshot
        | BaseItemImageType::Menu
        | BaseItemImageType::BoxRear
        | BaseItemImageType::Profile => None,
    }
}

fn take_index(index: &mut i32) -> i32 {
    let current = *index;
    *index = index.saturating_add(1);
    current
}

fn image_cache_tag(item_path: &str, date_modified: DateTime<Utc>) -> String {
    let subsecond_ticks = i128::from(date_modified.timestamp_subsec_nanos() / 100);
    let ticks = DOTNET_UNIX_EPOCH_TICKS
        + i128::from(date_modified.timestamp()) * TICKS_PER_SECOND
        + subsecond_ticks;
    let source = format!("{item_path}{ticks}");
    let mut hasher = Md5::new();
    for unit in source.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    Uuid::from_bytes_le(hasher.finalize().into())
        .simple()
        .to_string()
}

const fn model_image_type(image_type: BaseItemImageType) -> ImageType {
    match image_type {
        BaseItemImageType::Primary => ImageType::Primary,
        BaseItemImageType::Art => ImageType::Art,
        BaseItemImageType::Backdrop => ImageType::Backdrop,
        BaseItemImageType::Banner => ImageType::Banner,
        BaseItemImageType::Logo => ImageType::Logo,
        BaseItemImageType::Thumb => ImageType::Thumb,
        BaseItemImageType::Disc => ImageType::Disc,
        BaseItemImageType::Box => ImageType::Box,
        BaseItemImageType::Screenshot => ImageType::Screenshot,
        BaseItemImageType::Menu => ImageType::Menu,
        BaseItemImageType::Chapter => ImageType::Chapter,
        BaseItemImageType::BoxRear => ImageType::BoxRear,
        BaseItemImageType::Profile => ImageType::Profile,
    }
}

const fn persisted_image_type(image_type: ImageType) -> BaseItemImageType {
    match image_type {
        ImageType::Primary => BaseItemImageType::Primary,
        ImageType::Art => BaseItemImageType::Art,
        ImageType::Backdrop => BaseItemImageType::Backdrop,
        ImageType::Banner => BaseItemImageType::Banner,
        ImageType::Logo => BaseItemImageType::Logo,
        ImageType::Thumb => BaseItemImageType::Thumb,
        ImageType::Disc => BaseItemImageType::Disc,
        ImageType::Box => BaseItemImageType::Box,
        ImageType::Screenshot => BaseItemImageType::Screenshot,
        ImageType::Menu => BaseItemImageType::Menu,
        ImageType::Chapter => BaseItemImageType::Chapter,
        ImageType::BoxRear => BaseItemImageType::BoxRear,
        ImageType::Profile => BaseItemImageType::Profile,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::image_cache_tag;

    #[test]
    fn cache_tag_matches_jellyfin_utf16_md5_guid_contract() {
        let modified = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).single().unwrap();

        assert_eq!(
            image_cache_tag("/media/image-info-test.mkv", modified),
            "fdcbd27b24b37e862315a492f0300d8c"
        );
    }
}
