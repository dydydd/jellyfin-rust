use chrono::{DateTime, Utc};
use jellyfin_model::{CollectionType, ImageType};
use uuid::Uuid;

/// Image metadata required to build an item DTO image tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtoImage {
    pub image_type: ImageType,
    pub path: String,
    pub date_modified: DateTime<Utc>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Item kinds with distinct primary-image inheritance behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtoImageItemKind {
    UserView {
        view_type: CollectionType,
        display_parent_id: Option<Uuid>,
    },
    Episode {
        season_id: Option<Uuid>,
        series_id: Option<Uuid>,
    },
    Other,
}

/// Library item projection consumed by [`DtoImageProjectionService`].
#[derive(Debug, Clone, PartialEq)]
pub struct DtoImageItem {
    pub id: Uuid,
    pub kind: DtoImageItemKind,
    pub images: Vec<DtoImage>,
    pub default_primary_image_aspect_ratio: Option<f64>,
}

impl DtoImageItem {
    fn primary_image(&self) -> Option<&DtoImage> {
        self.images
            .iter()
            .find(|image| image.image_type == ImageType::Primary)
    }
}

/// Image-related DTO options used by the projection service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtoImageOptions {
    pub enable_images: bool,
    pub primary_image_limit: usize,
    pub include_primary_image_aspect_ratio: bool,
}

impl Default for DtoImageOptions {
    fn default() -> Self {
        Self {
            enable_images: true,
            primary_image_limit: usize::MAX,
            include_primary_image_aspect_ratio: false,
        }
    }
}

impl DtoImageOptions {
    const fn includes_primary_images(self) -> bool {
        self.enable_images && self.primary_image_limit > 0
    }
}

/// Primary-image fields projected onto a base item DTO.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DtoImageProjection {
    /// Corresponds to the `Primary` entry in Jellyfin's `ImageTags` map.
    pub primary_image_tag: Option<String>,
    pub series_primary_image_tag: Option<String>,
    pub parent_primary_image_item_id: Option<Uuid>,
    pub parent_primary_image_tag: Option<String>,
    pub primary_image_aspect_ratio: Option<f64>,
}

/// Item lookup boundary used when resolving display-parent, season, and series images.
pub trait DtoImageLibrary {
    fn get_item_by_id(&self, item_id: Uuid) -> Option<DtoImageItem>;
}

/// Cache-tag boundary for images exposed through a DTO.
pub trait ImageCacheTagProvider {
    /// Returns `None` when a stable cache tag cannot be produced.
    fn get_image_cache_tag(&self, item: &DtoImageItem, image: &DtoImage) -> Option<String>;
}

/// Projects primary-image DTO fields while preserving Jellyfin inheritance rules.
#[derive(Debug, Clone)]
pub struct DtoImageProjectionService<L, C> {
    library: L,
    cache_tags: C,
}

impl<L, C> DtoImageProjectionService<L, C> {
    pub const fn new(library: L, cache_tags: C) -> Self {
        Self {
            library,
            cache_tags,
        }
    }

    pub const fn library(&self) -> &L {
        &self.library
    }

    pub const fn cache_tags(&self) -> &C {
        &self.cache_tags
    }
}

impl<L: DtoImageLibrary, C: ImageCacheTagProvider> DtoImageProjectionService<L, C> {
    /// Projects image tags and the optional primary-image aspect ratio for one item.
    pub fn project(&self, item: &DtoImageItem, options: DtoImageOptions) -> DtoImageProjection {
        let primary_image_tag = options
            .includes_primary_images()
            .then(|| self.primary_image_tag(item))
            .flatten();
        let primary_image_aspect_ratio = options
            .include_primary_image_aspect_ratio
            .then_some(item.default_primary_image_aspect_ratio)
            .flatten();
        let mut projection = DtoImageProjection {
            primary_image_tag,
            primary_image_aspect_ratio,
            ..DtoImageProjection::default()
        };

        match item.kind {
            DtoImageItemKind::UserView {
                view_type: CollectionType::Playlists,
                display_parent_id: Some(display_parent_id),
            } if options.includes_primary_images() => {
                self.attach_playlist_display_parent(&mut projection, display_parent_id);
            }
            DtoImageItemKind::Episode {
                season_id,
                series_id,
            } => {
                self.attach_episode_images(&mut projection, season_id, series_id, options);
            }
            DtoImageItemKind::UserView { .. } | DtoImageItemKind::Other => {}
        }

        projection
    }

    fn primary_image_tag(&self, item: &DtoImageItem) -> Option<String> {
        item.primary_image()
            .and_then(|image| self.cache_tags.get_image_cache_tag(item, image))
    }

    fn attach_playlist_display_parent(
        &self,
        projection: &mut DtoImageProjection,
        display_parent_id: Uuid,
    ) {
        let Some(parent) = self.library.get_item_by_id(display_parent_id) else {
            return;
        };
        let Some(tag) = self.primary_image_tag(&parent) else {
            return;
        };

        projection.primary_image_tag = None;
        projection.parent_primary_image_item_id = Some(parent.id);
        projection.parent_primary_image_tag = Some(tag);
    }

    fn attach_episode_images(
        &self,
        projection: &mut DtoImageProjection,
        season_id: Option<Uuid>,
        series_id: Option<Uuid>,
        options: DtoImageOptions,
    ) {
        let series = series_id.and_then(|id| self.library.get_item_by_id(id));
        let series_tag = series
            .as_ref()
            .and_then(|series| self.primary_image_tag(series));

        projection.series_primary_image_tag.clone_from(&series_tag);
        if options.include_primary_image_aspect_ratio
            && projection.primary_image_tag.is_none()
            && series_tag.is_some()
        {
            projection.primary_image_aspect_ratio = series
                .as_ref()
                .and_then(|series| series.default_primary_image_aspect_ratio);
        }

        if !options.includes_primary_images() {
            return;
        }

        let season = season_id.and_then(|id| self.library.get_item_by_id(id));
        let season_tag = season
            .as_ref()
            .and_then(|season| self.primary_image_tag(season));

        if let (Some(season), Some(tag)) = (season, season_tag) {
            projection.parent_primary_image_item_id = Some(season.id);
            projection.parent_primary_image_tag = Some(tag);
        } else if let (Some(series), Some(tag)) = (series, series_tag) {
            projection.parent_primary_image_item_id = Some(series.id);
            projection.parent_primary_image_tag = Some(tag);
        }
    }
}
