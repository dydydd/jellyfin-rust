use std::collections::BTreeMap;

use jellyfin_model::{ImageFormat, ImageType, MediaProtocol, RemoteImageInfo};
use jellyfin_providers::{
    manager::item_image_provider::{
        ImageItem, ImageItemKind, ImageLibraryOptions, ImageProvider, ImageRefreshOptions,
        ImageTypeOptions, ItemImage, ItemImageProvider, ItemImageProviderCapability,
        LocalImageInfo, RemoteImageResponse,
    },
    media_info::EmbeddedImageResponse,
};

#[derive(Default)]
struct FixtureCapability {
    local_images: Vec<LocalImageInfo>,
    dynamic_response: Option<EmbeddedImageResponse>,
    remote_images: Vec<RemoteImageInfo>,
    remote_content: Vec<u8>,
    last_write_time: i64,
    deleted: Vec<String>,
    dynamic_calls: usize,
    remote_response_calls: usize,
    stream_saves: usize,
    path_saves: usize,
    remote_saves: usize,
    file_lengths: BTreeMap<String, u64>,
}

impl ItemImageProviderCapability for FixtureCapability {
    type Error = String;

    fn local_images(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
    ) -> Result<Vec<LocalImageInfo>, Self::Error> {
        Ok(self.local_images.clone())
    }

    fn dynamic_image(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
        _image_type: ImageType,
    ) -> Result<EmbeddedImageResponse, Self::Error> {
        self.dynamic_calls += 1;
        Ok(self
            .dynamic_response
            .clone()
            .unwrap_or_else(no_dynamic_image))
    }

    fn remote_images(
        &mut self,
        _provider_name: &str,
        _item: &ImageItem,
    ) -> Result<Vec<RemoteImageInfo>, Self::Error> {
        Ok(self.remote_images.clone())
    }

    fn remote_response(
        &mut self,
        _provider_name: &str,
        url: &str,
    ) -> Result<RemoteImageResponse, Self::Error> {
        self.remote_response_calls += 1;
        let mut response = RemoteImageResponse::ok(url, self.remote_content.clone());
        response.mime_type = Some("image/jpeg".to_owned());
        Ok(response)
    }

    fn path_exists(&self, path: &str) -> bool {
        path.starts_with("valid") || path.starts_with("saved")
    }

    fn last_write_time(&self, _path: &str) -> i64 {
        self.last_write_time
    }

    fn file_length(&self, path: &str) -> Option<u64> {
        self.file_lengths.get(path).copied()
    }

    fn delete_file(&mut self, path: &str) -> Result<(), Self::Error> {
        self.deleted.push(path.to_owned());
        Ok(())
    }

    fn save_image_stream(
        &mut self,
        _provider_name: &str,
        image_type: ImageType,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        self.stream_saves += 1;
        Ok(ItemImage::new("saved stream", image_type))
    }

    fn save_image_path(
        &mut self,
        _provider_name: &str,
        _path: &str,
        image_type: ImageType,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        self.path_saves += 1;
        Ok(ItemImage::new("saved path", image_type))
    }

    fn save_remote_image(
        &mut self,
        _provider_name: &str,
        image_type: ImageType,
        response: &RemoteImageResponse,
        _mime_type: &str,
    ) -> Result<ItemImage, Self::Error> {
        self.remote_saves += 1;
        let mut image = ItemImage::new(format!("saved remote {}", self.remote_saves), image_type);
        image.file_length = u64::try_from(response.content.len()).ok();
        Ok(image)
    }
}

fn no_dynamic_image() -> EmbeddedImageResponse {
    EmbeddedImageResponse {
        path: None,
        protocol: MediaProtocol::File,
        format: None,
        has_image: false,
        cache_key: None,
    }
}

fn dynamic_image(path: Option<&str>, protocol: MediaProtocol) -> EmbeddedImageResponse {
    EmbeddedImageResponse {
        path: path.map(str::to_owned),
        protocol,
        format: Some(ImageFormat::Jpg),
        has_image: true,
        cache_key: None,
    }
}

fn item_with_images(image_type: ImageType, count: usize, valid: bool) -> ImageItem {
    let mut item = ImageItem::default();
    for index in 0..count {
        item.images.push(ItemImage::new(
            format!("{} path {index}", if valid { "valid" } else { "invalid" }),
            image_type,
        ));
    }
    item
}

fn local_images(image_type: ImageType, count: usize, valid: bool) -> Vec<LocalImageInfo> {
    (0..count)
        .map(|index| {
            LocalImageInfo::new(
                format!("{} path {index}", if valid { "valid" } else { "invalid" }),
                image_type,
            )
        })
        .collect()
}

fn remote_images(image_type: ImageType, count: usize) -> Vec<RemoteImageInfo> {
    (0..count)
        .map(|index| RemoteImageInfo {
            image_type,
            url: Some(format!("image url {index}")),
            ..RemoteImageInfo::default()
        })
        .collect()
}

fn library_options(image_type: ImageType, count: usize) -> ImageLibraryOptions {
    ImageLibraryOptions {
        image_options: vec![ImageTypeOptions::new(image_type, count)],
    }
}

fn assert_type_count(item: &ImageItem, image_type: ImageType, count: usize) {
    assert_eq!(item.image_count(image_type), count);
}

#[test]
fn validate_images_photo_empty_providers_no_change() {
    let mut item = ImageItem {
        kind: ImageItemKind::Photo,
        ..ImageItem::default()
    };
    let mut capability = FixtureCapability::default();
    let changed = ItemImageProvider
        .validate_images(&mut item, &[], None, &mut capability)
        .unwrap();
    assert!(!changed);
}

#[test]
fn validate_images_empty_item_empty_providers_no_change() {
    let mut item = ImageItem::default();
    let mut capability = FixtureCapability::default();
    let changed = ItemImageProvider
        .validate_images(&mut item, &[], None, &mut capability)
        .unwrap();
    assert!(!changed);
    assert!(item.images.is_empty());
}

#[test]
fn validate_images_empty_item_and_populated_providers_adds_images() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = ImageItem::default();
        let mut capability = FixtureCapability {
            local_images: local_images(image_type, count, true),
            ..FixtureCapability::default()
        };
        let changed = ItemImageProvider
            .validate_images(
                &mut item,
                &[ImageProvider::local("fixture")],
                None,
                &mut capability,
            )
            .unwrap();
        assert!(changed);
        assert_type_count(&item, image_type, count);
    }
}

#[test]
fn validate_images_populated_item_with_good_paths_and_empty_providers_no_change() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = item_with_images(image_type, count, true);
        let mut capability = FixtureCapability::default();
        let changed = ItemImageProvider
            .validate_images(&mut item, &[], None, &mut capability)
            .unwrap();
        assert!(!changed);
        assert_type_count(&item, image_type, count);
    }
}

#[test]
fn validate_images_populated_item_with_bad_paths_and_empty_providers_removes_image() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = item_with_images(image_type, count, false);
        let mut capability = FixtureCapability::default();
        let changed = ItemImageProvider
            .validate_images(&mut item, &[], None, &mut capability)
            .unwrap();
        assert!(changed);
        assert_type_count(&item, image_type, 0);
    }
}

#[test]
fn merge_images_empty_item_new_images_empty_no_change() {
    let mut item = ImageItem::default();
    let capability = FixtureCapability::default();
    assert!(!ItemImageProvider.merge_images(&mut item, &[], None, &capability));
}

#[test]
fn merge_images_populated_item_with_good_paths_and_populated_new_images_adds_updates_images() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = item_with_images(image_type, count, true);
        let capability = FixtureCapability::default();
        let incoming = local_images(image_type, count, false);
        let changed = ItemImageProvider.merge_images(&mut item, &incoming, None, &capability);
        assert!(changed);
        let expected = if image_type == ImageType::Backdrop {
            count * 2
        } else {
            1
        };
        assert_type_count(&item, image_type, expected);
        if image_type == ImageType::Primary {
            assert_eq!(item.images_of(image_type)[0].path, incoming[0].path);
        }
    }
}

#[test]
fn merge_images_populated_item_with_good_paths_and_same_new_images_reset_if_time_changes() {
    for (image_type, count, update_time) in [
        (ImageType::Primary, 1, false),
        (ImageType::Backdrop, 2, false),
        (ImageType::Primary, 1, true),
        (ImageType::Backdrop, 2, true),
    ] {
        let mut item = item_with_images(image_type, count, true);
        for image in &mut item.images {
            image.date_modified = 1;
            image.width = 1;
            image.height = 1;
        }
        let capability = FixtureCapability {
            last_write_time: if update_time { 2 } else { 1 },
            ..FixtureCapability::default()
        };
        let incoming = local_images(image_type, count, true);
        let changed = ItemImageProvider.merge_images(&mut item, &incoming, None, &capability);
        assert_eq!(changed, update_time);
        if update_time {
            for image in item.images_of(image_type) {
                assert_eq!(image.date_modified, 2);
                assert_eq!((image.width, image.height), (0, 0));
            }
        }
    }
}

#[test]
fn remove_images_deletes_images_when_found() {
    for (image_type, count) in [
        (ImageType::Primary, 0),
        (ImageType::Primary, 1),
        (ImageType::Backdrop, 2),
    ] {
        let mut item = item_with_images(image_type, count, false);
        let mut capability = FixtureCapability::default();
        let removed = ItemImageProvider
            .remove_images(&mut item, false, &mut capability)
            .unwrap();
        assert_eq!(removed, count != 0);
        assert_type_count(&item, image_type, 0);
        assert_eq!(capability.deleted.len(), count);
    }
}

#[test]
fn refresh_images_populated_item_populated_provider_dynamic_updates_images_if_forced() {
    for (image_type, count, force) in [
        (ImageType::Primary, 1, false),
        (ImageType::Backdrop, 2, false),
        (ImageType::Primary, 1, true),
        (ImageType::Backdrop, 2, true),
    ] {
        let mut item = item_with_images(image_type, count, false);
        let mut capability = FixtureCapability {
            dynamic_response: Some(dynamic_image(Some("url path"), MediaProtocol::Http)),
            ..FixtureCapability::default()
        };
        let options = ImageRefreshOptions {
            full_refresh: force,
            replace_all_images: force,
            ..ImageRefreshOptions::default()
        };
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, count),
            &[ImageProvider::dynamic("dynamic", vec![image_type])],
            &options,
            &mut capability,
        );
        assert_eq!(result.image_updated, force);
        assert_eq!(capability.dynamic_calls, usize::from(force));
        assert_type_count(&item, image_type, if force { 1 } else { count });
    }
}

#[test]
fn refresh_images_empty_item_populated_provider_dynamic_adds_images() {
    for (image_type, count, has_path, protocol) in [
        (ImageType::Primary, 1, true, MediaProtocol::Http),
        (ImageType::Backdrop, 2, true, MediaProtocol::Http),
        (ImageType::Primary, 1, true, MediaProtocol::File),
        (ImageType::Backdrop, 2, true, MediaProtocol::File),
        (ImageType::Primary, 1, false, MediaProtocol::File),
        (ImageType::Backdrop, 2, false, MediaProtocol::File),
    ] {
        let response_path = has_path.then_some("valid path 0");
        let mut item = ImageItem::default();
        let mut capability = FixtureCapability {
            dynamic_response: Some(dynamic_image(response_path, protocol)),
            ..FixtureCapability::default()
        };
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, count),
            &[ImageProvider::dynamic("dynamic", vec![image_type])],
            &ImageRefreshOptions::default(),
            &mut capability,
        );
        assert!(result.image_updated);
        assert_type_count(&item, image_type, 1);
        match (has_path, protocol) {
            (true, MediaProtocol::Http) => {
                assert_eq!(item.images_of(image_type)[0].path, "valid path 0");
                assert_eq!((capability.path_saves, capability.stream_saves), (0, 0));
            }
            (true, _) => assert_eq!(capability.path_saves, 1),
            (false, _) => assert_eq!(capability.stream_saves, 1),
        }
    }
}

#[test]
fn refresh_images_populated_item_populated_provider_remote_updates_images_if_forced() {
    for (image_type, count, force) in [
        (ImageType::Primary, 1, false),
        (ImageType::Backdrop, 1, false),
        (ImageType::Backdrop, 2, false),
        (ImageType::Primary, 1, true),
        (ImageType::Backdrop, 1, true),
        (ImageType::Backdrop, 2, true),
    ] {
        let mut item = item_with_images(image_type, count, false);
        let mut capability = FixtureCapability {
            remote_images: remote_images(image_type, count),
            ..FixtureCapability::default()
        };
        let options = ImageRefreshOptions {
            full_refresh: force,
            replace_all_images: force,
            ..ImageRefreshOptions::default()
        };
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, count),
            &[ImageProvider::remote("remote", vec![image_type])],
            &options,
            &mut capability,
        );
        assert_eq!(result.image_updated, force);
        assert_type_count(&item, image_type, count);
        assert_eq!(
            item.images_of(image_type)
                .iter()
                .all(|image| image.path.starts_with("image url")),
            force
        );
    }
}

#[test]
fn refresh_images_non_stub_item_populated_provider_remote_downloads_if_necessary() {
    for (image_type, initial_count, full_refresh) in [
        (ImageType::Primary, 0, false),
        (ImageType::Backdrop, 0, false),
        (ImageType::Backdrop, 1, false),
        (ImageType::Backdrop, 1, true),
    ] {
        let content = b"Content".to_vec();
        let mut item = item_with_images(image_type, initial_count, false);
        item.path = Some("non-empty path".to_owned());
        for image in &mut item.images {
            image.file_length = u64::try_from(content.len()).ok();
        }
        let mut capability = FixtureCapability {
            remote_images: remote_images(image_type, 1),
            remote_content: content,
            ..FixtureCapability::default()
        };
        let options = ImageRefreshOptions {
            full_refresh,
            replace_all_images: full_refresh,
            ..ImageRefreshOptions::default()
        };
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, 2),
            &[ImageProvider::remote("remote", vec![image_type])],
            &options,
            &mut capability,
        );
        let should_download = initial_count == 0 || full_refresh;
        assert_eq!(result.image_updated, should_download);
        assert_type_count(&item, image_type, 1);
        assert_eq!(capability.remote_saves, usize::from(should_download));
    }
}

#[test]
fn refresh_images_empty_item_populated_provider_remote_extras_limits_images() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = ImageItem::default();
        let mut capability = FixtureCapability {
            remote_images: remote_images(image_type, count * 2),
            ..FixtureCapability::default()
        };
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, count),
            &[ImageProvider::remote("remote", vec![image_type])],
            &ImageRefreshOptions::default(),
            &mut capability,
        );
        assert!(result.image_updated);
        assert_type_count(&item, image_type, count);
        for (index, image) in item.images_of(image_type).iter().enumerate() {
            assert_eq!(image.path, format!("image url {index}"));
        }
    }
}

#[test]
fn refresh_images_populated_item_empty_provider_remote_full_refresh_doesnt_clear_images() {
    for (image_type, count) in [(ImageType::Primary, 1), (ImageType::Backdrop, 2)] {
        let mut item = item_with_images(image_type, count, false);
        let mut capability = FixtureCapability::default();
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &library_options(image_type, count),
            &[ImageProvider::remote("remote", vec![image_type])],
            &ImageRefreshOptions {
                full_refresh: true,
                replace_all_images: true,
                ..ImageRefreshOptions::default()
            },
            &mut capability,
        );
        assert!(!result.image_updated);
        assert_type_count(&item, image_type, count);
    }
}

#[test]
fn refresh_images_provider_remote_filters_by_width() {
    for (width, expected_update) in [(Some(9), false), (Some(10), true), (None, true)] {
        let mut item = ImageItem::default();
        let mut image = remote_images(ImageType::Primary, 1).remove(0);
        image.width = width;
        let mut capability = FixtureCapability {
            remote_images: vec![image],
            ..FixtureCapability::default()
        };
        let mut options = ImageTypeOptions::new(ImageType::Primary, 1);
        options.min_width = 10;
        let result = ItemImageProvider.refresh_images(
            &mut item,
            &ImageLibraryOptions {
                image_options: vec![options],
            },
            &[ImageProvider::remote("remote", vec![ImageType::Primary])],
            &ImageRefreshOptions::default(),
            &mut capability,
        );
        assert_eq!(result.image_updated, expected_update);
    }
}

#[test]
fn refresh_images_local_provider_attaches_discovered_artwork() {
    let mut item = ImageItem::default();
    let mut capability = FixtureCapability {
        local_images: vec![
            LocalImageInfo::new("/media/movies/The Matrix/poster.jpg", ImageType::Primary),
            LocalImageInfo::new("/media/movies/The Matrix/fanart.png", ImageType::Backdrop),
        ],
        ..FixtureCapability::default()
    };

    let result = ItemImageProvider.refresh_images(
        &mut item,
        &ImageLibraryOptions::default(),
        &[ImageProvider::local("local")],
        &ImageRefreshOptions::default(),
        &mut capability,
    );

    assert!(result.image_updated);
    assert_eq!(item.image_count(ImageType::Primary), 1);
    assert_eq!(item.image_count(ImageType::Backdrop), 1);
    // Local artwork keeps its own path instead of being copied into metadata.
    let primary = item.images_of(ImageType::Primary).remove(0);
    assert_eq!(primary.path, "/media/movies/The Matrix/poster.jpg");
    assert!(primary.is_local_file);
}

#[test]
fn refresh_images_local_provider_does_not_overwrite_existing_images() {
    let mut item = item_with_images(ImageType::Primary, 1, false);
    let existing = item.images_of(ImageType::Primary).remove(0).path.clone();
    let mut capability = FixtureCapability {
        local_images: vec![LocalImageInfo::new("/media/local/poster.jpg", ImageType::Primary)],
        ..FixtureCapability::default()
    };

    let result = ItemImageProvider.refresh_images(
        &mut item,
        &ImageLibraryOptions::default(),
        &[ImageProvider::local("local")],
        &ImageRefreshOptions::default(),
        &mut capability,
    );

    assert!(!result.image_updated);
    assert_eq!(item.image_count(ImageType::Primary), 1);
    assert_eq!(item.images_of(ImageType::Primary).remove(0).path, existing);
}

#[test]
fn refresh_images_local_provider_skips_disabled_image_types() {
    let mut item = ImageItem::default();
    let mut capability = FixtureCapability {
        local_images: vec![LocalImageInfo::new("/media/local/logo.png", ImageType::Logo)],
        ..FixtureCapability::default()
    };

    let mut options = ImageTypeOptions::new(ImageType::Logo, 1);
    options.enabled = false;

    let result = ItemImageProvider.refresh_images(
        &mut item,
        &ImageLibraryOptions {
            image_options: vec![options],
        },
        &[ImageProvider::local("local")],
        &ImageRefreshOptions::default(),
        &mut capability,
    );

    assert!(!result.image_updated);
    assert_eq!(item.image_count(ImageType::Logo), 0);
}
