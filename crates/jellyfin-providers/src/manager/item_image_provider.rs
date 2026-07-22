use std::{fmt::Display, path::Path};

use jellyfin_model::{ImageFormat, ImageType, MediaProtocol, MimeTypes, RemoteImageInfo};

use crate::media_info::EmbeddedImageResponse;

/// Image types for which an item can store only one image.
pub const SINGULAR_IMAGE_TYPES: [ImageType; 9] = [
    ImageType::Primary,
    ImageType::Art,
    ImageType::Banner,
    ImageType::Box,
    ImageType::BoxRear,
    ImageType::Disc,
    ImageType::Logo,
    ImageType::Menu,
    ImageType::Thumb,
];

/// Item categories that affect local scanning and remote image stubs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageItemKind {
    #[default]
    Video,
    Photo,
    LiveTvProgram,
    ItemByName,
    MusicArtist,
}

/// An image currently attached to an item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemImage {
    pub path: String,
    pub image_type: ImageType,
    pub date_modified: i64,
    pub width: i32,
    pub height: i32,
    pub is_local_file: bool,
    pub file_length: Option<u64>,
}

impl ItemImage {
    #[must_use]
    pub fn new(path: impl Into<String>, image_type: ImageType) -> Self {
        Self {
            path: path.into(),
            image_type,
            date_modified: 0,
            width: 0,
            height: 0,
            is_local_file: true,
            file_length: None,
        }
    }
}

/// The item state consumed and updated by image refreshes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageItem {
    pub kind: ImageItemKind,
    pub path: Option<String>,
    pub containing_folder_path: Option<String>,
    pub internal_metadata_path: String,
    pub protocol: MediaProtocol,
    pub save_local_metadata: bool,
    pub is_virtual: bool,
    pub supports_remote_images: bool,
    pub images: Vec<ItemImage>,
}

impl Default for ImageItem {
    fn default() -> Self {
        Self {
            kind: ImageItemKind::Video,
            path: None,
            containing_folder_path: None,
            internal_metadata_path: String::new(),
            protocol: MediaProtocol::File,
            save_local_metadata: false,
            is_virtual: false,
            supports_remote_images: true,
            images: Vec::new(),
        }
    }
}

impl ImageItem {
    #[must_use]
    pub fn images_of(&self, image_type: ImageType) -> Vec<&ItemImage> {
        self.images
            .iter()
            .filter(|image| image.image_type == image_type)
            .collect()
    }

    #[must_use]
    pub fn image_count(&self, image_type: ImageType) -> usize {
        self.images
            .iter()
            .filter(|image| image.image_type == image_type)
            .count()
    }

    #[must_use]
    pub fn has_image(&self, image_type: ImageType) -> bool {
        self.images
            .iter()
            .any(|image| image.image_type == image_type)
    }

    fn set_image(&mut self, image: ItemImage) {
        if allows_multiple_images(image.image_type) {
            self.images.push(image);
        } else if let Some(current) = self
            .images
            .iter_mut()
            .find(|current| current.image_type == image.image_type)
        {
            *current = image;
        } else {
            self.images.push(image);
        }
    }
}

/// A local provider's discovered image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalImageInfo {
    pub image_type: ImageType,
    pub path: String,
}

impl LocalImageInfo {
    #[must_use]
    pub fn new(path: impl Into<String>, image_type: ImageType) -> Self {
        Self {
            image_type,
            path: path.into(),
        }
    }
}

/// Per-image-type library settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageTypeOptions {
    pub image_type: ImageType,
    pub limit: usize,
    pub min_width: i32,
    pub enabled: bool,
}

impl ImageTypeOptions {
    #[must_use]
    pub const fn new(image_type: ImageType, limit: usize) -> Self {
        Self {
            image_type,
            limit,
            min_width: 0,
            enabled: true,
        }
    }
}

/// Image settings for one library item type.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImageLibraryOptions {
    pub image_options: Vec<ImageTypeOptions>,
}

impl ImageLibraryOptions {
    fn option(&self, image_type: ImageType) -> Option<&ImageTypeOptions> {
        self.image_options
            .iter()
            .find(|option| option.image_type == image_type)
    }

    fn is_enabled(&self, image_type: ImageType) -> bool {
        self.option(image_type).is_none_or(|option| option.enabled)
    }

    fn limit(&self, image_type: ImageType) -> usize {
        self.option(image_type).map_or(1, |option| option.limit)
    }

    fn min_width(&self, image_type: ImageType) -> i32 {
        self.option(image_type).map_or(0, |option| option.min_width)
    }
}

/// Controls which existing images may be replaced during a refresh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImageRefreshOptions {
    pub full_refresh: bool,
    pub replace_all_images: bool,
    pub replace_images: Vec<ImageType>,
}

impl ImageRefreshOptions {
    #[must_use]
    pub fn is_replacing(&self, image_type: ImageType) -> bool {
        self.replace_all_images || self.replace_images.contains(&image_type)
    }
}

/// Provider descriptor. Provider work itself is supplied by a capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageProvider {
    Local {
        name: String,
    },
    Dynamic {
        name: String,
        supported_images: Vec<ImageType>,
    },
    Remote {
        name: String,
        supported_images: Vec<ImageType>,
    },
}

impl ImageProvider {
    #[must_use]
    pub fn local(name: impl Into<String>) -> Self {
        Self::Local { name: name.into() }
    }

    #[must_use]
    pub fn dynamic(name: impl Into<String>, supported_images: Vec<ImageType>) -> Self {
        Self::Dynamic {
            name: name.into(),
            supported_images,
        }
    }

    #[must_use]
    pub fn remote(name: impl Into<String>, supported_images: Vec<ImageType>) -> Self {
        Self::Remote {
            name: name.into(),
            supported_images,
        }
    }
}

/// HTTP-like status returned by a fixture-backed remote provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteImageStatus {
    Ok,
    NotFound,
    Forbidden,
    Error,
}

/// A remote image response without tying provider logic to an HTTP client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteImageResponse {
    pub status: RemoteImageStatus,
    pub content: Vec<u8>,
    pub mime_type: Option<String>,
    pub request_url: String,
}

impl RemoteImageResponse {
    #[must_use]
    pub fn ok(request_url: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            status: RemoteImageStatus::Ok,
            content: content.into(),
            mime_type: None,
            request_url: request_url.into(),
        }
    }
}

/// Result flags and non-fatal provider error from an image refresh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImageRefreshResult {
    pub image_updated: bool,
    pub error_message: Option<String>,
}

/// Fixture-friendly boundary for provider data and image persistence.
///
/// Default methods make focused capabilities small. Production adapters can
/// replace them with filesystem, provider-manager, and HTTP implementations.
pub trait ItemImageProviderCapability {
    type Error: Display;

    /// Lists images found by one local provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when local image enumeration fails.
    fn local_images(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
    ) -> Result<Vec<LocalImageInfo>, Self::Error> {
        Ok(Vec::new())
    }

    /// Gets a generated or extracted image from one dynamic provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when dynamic extraction fails.
    fn dynamic_image(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
        _image_type: ImageType,
    ) -> Result<EmbeddedImageResponse, Self::Error> {
        Ok(EmbeddedImageResponse {
            path: None,
            protocol: MediaProtocol::File,
            format: None,
            has_image: false,
            cache_key: None,
        })
    }

    /// Lists the preferred remote image candidates for one provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when remote enumeration fails.
    fn remote_images(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
    ) -> Result<Vec<RemoteImageInfo>, Self::Error> {
        Ok(Vec::new())
    }

    /// Retrieves one remote image candidate.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when the request cannot be completed.
    fn remote_response(
        &mut self,
        _provider_name: &str,
        url: &str,
    ) -> Result<RemoteImageResponse, Self::Error> {
        Ok(RemoteImageResponse {
            status: RemoteImageStatus::Error,
            content: Vec::new(),
            mime_type: None,
            request_url: url.to_owned(),
        })
    }

    fn path_exists(&self, _path: &str) -> bool {
        true
    }

    fn last_write_time(&self, _path: &str) -> i64 {
        0
    }

    fn file_length(&self, _path: &str) -> Option<u64> {
        None
    }

    /// Deletes a stored local image.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when deletion fails.
    fn delete_file(&mut self, _path: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Persists a stream returned by a dynamic provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when the image cannot be persisted.
    fn save_image_stream(
        &mut self,
        _provider_name: &str,
        image_type: ImageType,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        Ok(ItemImage::new("saved-stream-image", image_type))
    }

    /// Persists a file path returned by a dynamic provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when the image cannot be persisted.
    fn save_image_path(
        &mut self,
        _provider_name: &str,
        path: &str,
        image_type: ImageType,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        Ok(ItemImage::new(path, image_type))
    }

    /// Persists the bytes returned by a remote provider.
    ///
    /// # Errors
    ///
    /// Returns an adapter-specific error when the image cannot be persisted.
    fn save_remote_image(
        &mut self,
        _provider_name: &str,
        image_type: ImageType,
        response: &RemoteImageResponse,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        let mut image = ItemImage::new(&response.request_url, image_type);
        image.file_length = u64::try_from(response.content.len()).ok();
        Ok(image)
    }
}

/// Jellyfin-compatible orchestration for local, dynamic, and remote images.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ItemImageProvider;

impl ItemImageProvider {
    /// Validates stored paths and merges local-provider results.
    ///
    /// # Errors
    ///
    /// Returns a capability error raised while enumerating local images.
    pub fn validate_images<C: ItemImageProviderCapability + ?Sized>(
        &self,
        item: &mut ImageItem,
        providers: &[ImageProvider],
        refresh_options: Option<&mut ImageRefreshOptions>,
        capability: &mut C,
    ) -> Result<bool, C::Error> {
        if item.kind == ImageItemKind::Photo {
            return Ok(false);
        }

        let mut local_images = Vec::new();
        for provider in providers {
            if let ImageProvider::Local { name } = provider {
                local_images.extend(capability.local_images(name, item)?);
            }
        }
        Ok(self.merge_images(item, &local_images, refresh_options, capability))
    }

    /// Removes eligible singular images and backdrops from an item.
    ///
    /// # Errors
    ///
    /// Returns the first capability error encountered while deleting a file.
    pub fn remove_images<C: ItemImageProviderCapability + ?Sized>(
        &self,
        item: &mut ImageItem,
        can_delete_local: bool,
        capability: &mut C,
    ) -> Result<bool, C::Error> {
        let removable = item
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| {
                (is_singular(image.image_type) || image.image_type == ImageType::Backdrop)
                    && (image
                        .path
                        .to_ascii_lowercase()
                        .starts_with(&item.internal_metadata_path.to_ascii_lowercase())
                        || can_delete_local
                        || item.save_local_metadata)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        for &index in &removable {
            let image = &item.images[index];
            if image.is_local_file {
                capability.delete_file(&image.path)?;
            }
        }
        for index in removable.iter().rev() {
            item.images.remove(*index);
        }
        Ok(!removable.is_empty())
    }

    /// Merges local images after pruning invalid stored paths.
    pub fn merge_images<C: ItemImageProviderCapability + ?Sized>(
        &self,
        item: &mut ImageItem,
        images: &[LocalImageInfo],
        refresh_options: Option<&mut ImageRefreshOptions>,
        capability: &C,
    ) -> bool {
        let mut changed = validate_stored_images(item, capability);
        let mut found_image_types = Vec::new();

        for image_type in SINGULAR_IMAGE_TYPES {
            let Some(image) = images.iter().find(|image| image.image_type == image_type) else {
                continue;
            };
            if is_stored_with_media(item, &image.path) {
                found_image_types.push(image_type);
            }

            if let Some(current) = item
                .images
                .iter_mut()
                .find(|current| current.image_type == image_type)
            {
                if current.path.eq_ignore_ascii_case(&image.path) {
                    let modified = capability.last_write_time(&image.path);
                    if current.date_modified != modified
                        && (current.width > 0 || current.height > 0)
                    {
                        current.width = 0;
                        current.height = 0;
                        changed = true;
                    }
                    current.date_modified = modified;
                } else {
                    *current = local_item_image(image, capability);
                    changed = true;
                }
            } else {
                item.images.push(local_item_image(image, capability));
                changed = true;
            }
        }

        let backdrops = images
            .iter()
            .filter(|image| image.image_type == ImageType::Backdrop)
            .collect::<Vec<_>>();
        if !backdrops.is_empty() {
            if backdrops
                .iter()
                .any(|image| is_stored_with_media(item, &image.path))
            {
                found_image_types.push(ImageType::Backdrop);
            }
            for image in backdrops {
                if let Some(current) = item.images.iter_mut().find(|current| {
                    current.image_type == ImageType::Backdrop
                        && current.path.eq_ignore_ascii_case(&image.path)
                }) {
                    let modified = capability.last_write_time(&image.path);
                    if current.date_modified != modified
                        && (current.width > 0 || current.height > 0)
                    {
                        current.width = 0;
                        current.height = 0;
                        changed = true;
                    }
                    current.date_modified = modified;
                } else {
                    item.images.push(local_item_image(image, capability));
                    changed = true;
                }
            }
        }

        if let Some(options) = refresh_options {
            update_replace_images(options, &found_image_types);
        }
        changed
    }

    /// Refreshes dynamic and remote providers in the supplied preference order.
    pub fn refresh_images<C: ItemImageProviderCapability + ?Sized>(
        &self,
        item: &mut ImageItem,
        library_options: &ImageLibraryOptions,
        providers: &[ImageProvider],
        refresh_options: &ImageRefreshOptions,
        capability: &mut C,
    ) -> ImageRefreshResult {
        let old_backdrops = if refresh_options.is_replacing(ImageType::Backdrop) {
            item.images
                .iter()
                .filter(|image| image.image_type == ImageType::Backdrop)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let backdrop_limit = library_options
            .limit(ImageType::Backdrop)
            .saturating_add(old_backdrops.len());
        let mut result = ImageRefreshResult::default();
        let mut downloaded_images = Vec::new();

        for provider in providers {
            let provider_result = match provider {
                ImageProvider::Local { .. } => Ok(()),
                ImageProvider::Dynamic {
                    name,
                    supported_images,
                } => refresh_dynamic(
                    item,
                    name,
                    supported_images,
                    library_options,
                    refresh_options,
                    &mut downloaded_images,
                    &mut result,
                    capability,
                ),
                ImageProvider::Remote {
                    name,
                    supported_images,
                } => refresh_remote(
                    item,
                    name,
                    supported_images,
                    library_options,
                    refresh_options,
                    backdrop_limit,
                    &mut downloaded_images,
                    &mut result,
                    capability,
                ),
            };
            if let Err(error) = provider_result {
                result.error_message = Some(error.to_string());
            }
        }

        if !old_backdrops.is_empty() && old_backdrops.len() < item.image_count(ImageType::Backdrop)
        {
            prune_images(item, &old_backdrops, capability);
        }
        result
    }
}

fn validate_stored_images<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    capability: &C,
) -> bool {
    let old_len = item.images.len();
    item.images.retain(|image| {
        !image.is_local_file
            || image.path.starts_with("http")
            || capability.path_exists(&image.path)
    });
    item.images.len() != old_len
}

fn local_item_image<C: ItemImageProviderCapability + ?Sized>(
    image: &LocalImageInfo,
    capability: &C,
) -> ItemImage {
    let mut stored = ItemImage::new(&image.path, image.image_type);
    stored.date_modified = capability.last_write_time(&image.path);
    stored
}

fn is_stored_with_media(item: &ImageItem, path: &str) -> bool {
    let Some(containing_folder) = item.containing_folder_path.as_deref() else {
        return false;
    };
    Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .is_some_and(|parent| {
            containing_folder
                .to_ascii_lowercase()
                .contains(&parent.to_ascii_lowercase())
        })
}

fn update_replace_images(options: &mut ImageRefreshOptions, protected: &[ImageType]) {
    if options.replace_all_images {
        options.replace_all_images = false;
        options.replace_images = all_image_types().to_vec();
    }
    options
        .replace_images
        .retain(|image_type| !protected.contains(image_type));
}

#[allow(clippy::too_many_arguments)]
fn refresh_dynamic<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    provider_name: &str,
    supported_images: &[ImageType],
    library_options: &ImageLibraryOptions,
    refresh_options: &ImageRefreshOptions,
    downloaded_images: &mut Vec<ImageType>,
    result: &mut ImageRefreshResult,
    capability: &mut C,
) -> Result<(), C::Error> {
    for &image_type in supported_images {
        if !library_options.is_enabled(image_type)
            || (item.has_image(image_type)
                && (!refresh_options.is_replacing(image_type)
                    || downloaded_images.contains(&image_type)))
        {
            continue;
        }

        let response = capability.dynamic_image(provider_name, item, image_type)?;
        if !response.has_image {
            continue;
        }
        let image = match response.path.as_deref() {
            None => capability.save_image_stream(
                provider_name,
                image_type,
                response
                    .format
                    .map_or("application/octet-stream", ImageFormat::mime_type),
            )?,
            Some(path) if response.protocol == MediaProtocol::Http => {
                let mut image = ItemImage::new(path, image_type);
                image.is_local_file = false;
                image
            }
            Some(path) => {
                let mime_type = mime_type_for_path(path);
                capability.save_image_path(provider_name, path, image_type, &mime_type)?
            }
        };
        item.set_image(image);
        downloaded_images.push(image_type);
        result.image_updated = true;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn refresh_remote<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    provider_name: &str,
    supported_images: &[ImageType],
    library_options: &ImageLibraryOptions,
    refresh_options: &ImageRefreshOptions,
    backdrop_limit: usize,
    downloaded_images: &mut Vec<ImageType>,
    result: &mut ImageRefreshResult,
    capability: &mut C,
) -> Result<(), C::Error> {
    if !item.supports_remote_images {
        return Ok(());
    }
    if !refresh_options.replace_all_images
        && refresh_options.replace_images.is_empty()
        && contains_images(item, supported_images, library_options, backdrop_limit)
    {
        return Ok(());
    }

    let images = capability.remote_images(provider_name, item)?;
    for &image_type in &SINGULAR_IMAGE_TYPES {
        if !supported_images.contains(&image_type)
            || !library_options.is_enabled(image_type)
            || (item.has_image(image_type)
                && (!refresh_options.is_replacing(image_type)
                    || downloaded_images.contains(&image_type)))
        {
            continue;
        }
        if download_singular(
            item,
            provider_name,
            image_type,
            library_options.min_width(image_type),
            &images,
            result,
            capability,
        )? {
            downloaded_images.push(image_type);
        }
    }

    if supported_images.contains(&ImageType::Backdrop)
        && library_options.is_enabled(ImageType::Backdrop)
    {
        let mut backdrops = images
            .iter()
            .filter(|image| image.image_type == ImageType::Backdrop)
            .collect::<Vec<_>>();
        backdrops.sort_by_key(|image| {
            image
                .language
                .as_ref()
                .is_some_and(|value| !value.is_empty())
        });
        download_backdrops(
            item,
            provider_name,
            refresh_options,
            backdrop_limit,
            library_options.min_width(ImageType::Backdrop),
            &backdrops,
            result,
            capability,
        )?;
    }
    Ok(())
}

fn contains_images(
    item: &ImageItem,
    supported_images: &[ImageType],
    options: &ImageLibraryOptions,
    backdrop_limit: usize,
) -> bool {
    for image_type in SINGULAR_IMAGE_TYPES {
        if supported_images.contains(&image_type)
            && !item.has_image(image_type)
            && options.limit(image_type) > 0
        {
            return false;
        }
    }
    !supported_images.contains(&ImageType::Backdrop)
        || item.image_count(ImageType::Backdrop) >= backdrop_limit
}

#[allow(clippy::too_many_arguments)]
fn download_singular<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    provider_name: &str,
    image_type: ImageType,
    min_width: i32,
    images: &[RemoteImageInfo],
    result: &mut ImageRefreshResult,
    capability: &mut C,
) -> Result<bool, C::Error> {
    let eligible = images.iter().filter(|image| {
        image.image_type == image_type && image.width.is_none_or(|width| width >= min_width)
    });
    if enable_image_stub(item) {
        if let Some(url) = eligible.clone().find_map(|image| image.url.as_deref()) {
            save_image_stub(item, image_type, url);
            result.image_updated = true;
            return Ok(true);
        }
        return Ok(false);
    }

    for image in eligible {
        let Some(url) = image.url.as_deref() else {
            continue;
        };
        let response = capability.remote_response(provider_name, url)?;
        match response.status {
            RemoteImageStatus::NotFound | RemoteImageStatus::Forbidden => {}
            RemoteImageStatus::Error => break,
            RemoteImageStatus::Ok => {
                let mime_type = response_mime_type(&response);
                let saved = capability.save_remote_image(
                    provider_name,
                    image_type,
                    &response,
                    &mime_type,
                )?;
                item.set_image(saved);
                result.image_updated = true;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn download_backdrops<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    provider_name: &str,
    refresh_options: &ImageRefreshOptions,
    limit: usize,
    min_width: i32,
    images: &[&RemoteImageInfo],
    result: &mut ImageRefreshResult,
    capability: &mut C,
) -> Result<(), C::Error> {
    for image in images {
        if item.image_count(ImageType::Backdrop) >= limit {
            break;
        }
        if image.width.is_some_and(|width| width < min_width) {
            continue;
        }
        let Some(url) = image.url.as_deref() else {
            continue;
        };
        if enable_image_stub(item) {
            save_image_stub(item, ImageType::Backdrop, url);
            result.image_updated = true;
            continue;
        }

        let response = capability.remote_response(provider_name, url)?;
        match response.status {
            RemoteImageStatus::NotFound | RemoteImageStatus::Forbidden => {}
            RemoteImageStatus::Error => break,
            RemoteImageStatus::Ok => {
                let response_length = u64::try_from(response.content.len()).ok();
                if !refresh_options.is_replacing(ImageType::Backdrop)
                    && response_length.is_some_and(|length| {
                        item.images_of(ImageType::Backdrop).iter().any(|stored| {
                            stored
                                .file_length
                                .or_else(|| capability.file_length(&stored.path))
                                == Some(length)
                        })
                    })
                {
                    continue;
                }
                let mime_type = response_mime_type(&response);
                let saved = capability.save_remote_image(
                    provider_name,
                    ImageType::Backdrop,
                    &response,
                    &mime_type,
                )?;
                item.set_image(saved);
                result.image_updated = true;
            }
        }
    }
    Ok(())
}

fn enable_image_stub(item: &ImageItem) -> bool {
    item.kind == ImageItemKind::LiveTvProgram
        || item.path.as_deref().is_none_or(str::is_empty)
        || item.protocol != MediaProtocol::File
        || item.kind == ImageItemKind::ItemByName
}

fn save_image_stub(item: &mut ImageItem, image_type: ImageType, url: &str) {
    let mut image = ItemImage::new(url, image_type);
    image.is_local_file = false;
    item.set_image(image);
}

fn prune_images<C: ItemImageProviderCapability + ?Sized>(
    item: &mut ImageItem,
    images: &[ItemImage],
    capability: &mut C,
) {
    for image in images {
        if image.is_local_file {
            let _ = capability.delete_file(&image.path);
        }
    }
    for image in images {
        if let Some(index) = item.images.iter().position(|candidate| candidate == image) {
            item.images.remove(index);
        }
    }
}

fn response_mime_type(response: &RemoteImageResponse) -> String {
    response
        .mime_type
        .as_deref()
        .filter(|mime_type| !mime_type.eq_ignore_ascii_case("application/octet-stream"))
        .map_or_else(|| mime_type_for_path(&response.request_url), str::to_owned)
}

fn mime_type_for_path(path: &str) -> String {
    MimeTypes::get_mime_type(path).unwrap_or_else(|_| "application/octet-stream".to_owned())
}

const fn allows_multiple_images(image_type: ImageType) -> bool {
    !is_singular(image_type)
}

const fn is_singular(image_type: ImageType) -> bool {
    matches!(
        image_type,
        ImageType::Primary
            | ImageType::Art
            | ImageType::Banner
            | ImageType::Box
            | ImageType::BoxRear
            | ImageType::Disc
            | ImageType::Logo
            | ImageType::Menu
            | ImageType::Thumb
    )
}

const fn all_image_types() -> &'static [ImageType] {
    &[
        ImageType::Primary,
        ImageType::Art,
        ImageType::Backdrop,
        ImageType::Banner,
        ImageType::Logo,
        ImageType::Thumb,
        ImageType::Disc,
        ImageType::Box,
        ImageType::Screenshot,
        ImageType::Menu,
        ImageType::Chapter,
        ImageType::BoxRear,
        ImageType::Profile,
    ]
}
