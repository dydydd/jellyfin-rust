use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError, BaseItemImageType,
    entities::base_item,
};
use jellyfin_model::{ImageInfo, ImageType};
use md5::{Digest, Md5};
use thiserror::Error;
use uuid::Uuid;

const DOTNET_UNIX_EPOCH_TICKS: i128 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i128 = 10_000_000;

/// Failure while loading an item's persisted image metadata.
#[derive(Debug, Error)]
pub enum ItemImageError {
    #[error(transparent)]
    Store(#[from] BaseItemImageStoreError),
}

/// Projects PostgreSQL-backed item images onto Jellyfin's public image DTO.
#[derive(Clone)]
pub struct ItemImageService {
    images: BaseItemImageRepository,
}

impl ItemImageService {
    #[must_use]
    pub fn new(database: sea_orm::DatabaseConnection) -> Self {
        Self {
            images: BaseItemImageRepository::new(database),
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
}

const fn allows_multiple_images(image_type: BaseItemImageType) -> bool {
    matches!(
        image_type,
        BaseItemImageType::Backdrop | BaseItemImageType::Chapter
    )
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
