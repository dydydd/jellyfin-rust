use std::path::Path;

use jellyfin_model::ImageType;

use crate::manager::item_image_provider::{ImageItem, ImageItemKind, LocalImageInfo};

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tbn", "tif", "tiff",
];

/// Scans local folders and files to find artwork matching Jellyfin conventions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalImageProvider;

impl LocalImageProvider {
    /// Returns all local images found for the given item by querying `file_exists`.
    pub fn get_images<F>(item: &ImageItem, mut file_exists: F) -> Vec<LocalImageInfo>
    where
        F: FnMut(&str) -> bool,
    {
        let mut results = Vec::new();
        let containing_folder = item
            .containing_folder_path
            .as_deref()
            .or_else(|| item.path.as_deref().and_then(|p| Path::new(p).parent()?.to_str()))
            .unwrap_or("");

        let item_stem = item
            .path
            .as_deref()
            .and_then(|p| Path::new(p).file_stem()?.to_str());

        // 1. Primary images
        let primary_names: &[&str] = match item.kind {
            ImageItemKind::Video => &["poster", "folder", "cover", "default", "movie"],
            ImageItemKind::MusicArtist => &["folder", "poster", "cover", "default", "artist"],
            _ => &["poster", "folder", "cover", "default"],
        };

        Self::find_images(
            containing_folder,
            item_stem,
            primary_names,
            ImageType::Primary,
            &mut results,
            &mut file_exists,
        );

        // 2. Backdrops / Fanart
        let backdrop_names = &[
            "backdrop", "fanart", "background", "art",
            "backdrop1", "backdrop2", "backdrop3", "backdrop4", "backdrop5",
            "fanart1", "fanart2", "fanart3", "fanart4", "fanart5",
        ];
        Self::find_images(
            containing_folder,
            item_stem,
            backdrop_names,
            ImageType::Backdrop,
            &mut results,
            &mut file_exists,
        );

        // 3. Banner
        Self::find_images(
            containing_folder,
            item_stem,
            &["banner"],
            ImageType::Banner,
            &mut results,
            &mut file_exists,
        );

        // 4. Thumb / Landscape
        Self::find_images(
            containing_folder,
            item_stem,
            &["thumb", "landscape"],
            ImageType::Thumb,
            &mut results,
            &mut file_exists,
        );

        // 5. Logo
        Self::find_images(
            containing_folder,
            item_stem,
            &["logo", "clearlogo"],
            ImageType::Logo,
            &mut results,
            &mut file_exists,
        );

        // 6. Art
        Self::find_images(
            containing_folder,
            item_stem,
            &["clearart"],
            ImageType::Art,
            &mut results,
            &mut file_exists,
        );

        // 7. Disc
        Self::find_images(
            containing_folder,
            item_stem,
            &["disc", "cd", "discart"],
            ImageType::Disc,
            &mut results,
            &mut file_exists,
        );

        // 8. Box and BoxRear
        Self::find_images(
            containing_folder,
            item_stem,
            &["box", "box-front"],
            ImageType::Box,
            &mut results,
            &mut file_exists,
        );
        Self::find_images(
            containing_folder,
            item_stem,
            &["box-rear", "box-back"],
            ImageType::BoxRear,
            &mut results,
            &mut file_exists,
        );

        // 9. Menu
        Self::find_images(
            containing_folder,
            item_stem,
            &["menu"],
            ImageType::Menu,
            &mut results,
            &mut file_exists,
        );

        results
    }

    /// Finds season images stored in the parent series folder.
    pub fn get_season_images_from_series_folder<F>(
        series_folder: &str,
        season_number: i32,
        mut file_exists: F,
    ) -> Vec<LocalImageInfo>
    where
        F: FnMut(&str) -> bool,
    {
        let mut results = Vec::new();
        let prefix = if season_number == 0 {
            "season-specials".to_owned()
        } else {
            format!("season{season_number:02}")
        };

        for (suffix, img_type) in [
            ("-poster", ImageType::Primary),
            ("-fanart", ImageType::Backdrop),
            ("-banner", ImageType::Banner),
            ("-landscape", ImageType::Thumb),
        ] {
            let base = format!("{prefix}{suffix}");
            for ext in SUPPORTED_IMAGE_EXTENSIONS {
                let candidate = format!("{series_folder}/{base}.{ext}");
                if file_exists(&candidate) {
                    results.push(LocalImageInfo::new(candidate, img_type));
                    break;
                }
            }
        }

        results
    }

    fn find_images<F>(
        folder: &str,
        stem: Option<&str>,
        base_names: &[&str],
        image_type: ImageType,
        results: &mut Vec<LocalImageInfo>,
        file_exists: &mut F,
    ) where
        F: FnMut(&str) -> bool,
    {
        for name in base_names {
            // Check standalone name: e.g. folder/poster.jpg
            if !folder.is_empty() {
                for ext in SUPPORTED_IMAGE_EXTENSIONS {
                    let candidate = format!("{folder}/{name}.{ext}");
                    if file_exists(&candidate)
                        && !results.iter().any(|r| r.path == candidate)
                    {
                        results.push(LocalImageInfo::new(candidate, image_type));
                        break;
                    }
                }
            }

            // Check stem-prefixed name: e.g. folder/movie-poster.jpg
            if let Some(s) = stem
                && !folder.is_empty()
            {
                for ext in SUPPORTED_IMAGE_EXTENSIONS {
                    let candidate = format!("{folder}/{s}-{name}.{ext}");
                    if file_exists(&candidate)
                        && !results.iter().any(|r| r.path == candidate)
                    {
                        results.push(LocalImageInfo::new(candidate, image_type));
                        break;
                    }
                }
            }
        }

        // Also check standalone stem if checking primary: folder/movie.jpg
        if image_type == ImageType::Primary
            && let Some(s) = stem
            && !folder.is_empty()
        {
            for ext in SUPPORTED_IMAGE_EXTENSIONS {
                let candidate = format!("{folder}/{s}.{ext}");
                if file_exists(&candidate)
                    && !results.iter().any(|r| r.path == candidate)
                {
                    results.push(LocalImageInfo::new(candidate, image_type));
                    break;
                }
            }
        }
    }
}
