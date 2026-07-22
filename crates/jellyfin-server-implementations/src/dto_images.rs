use std::collections::HashMap;

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError,
    BaseItemImageType, BaseItemRepository, entities::base_item,
};
use jellyfin_model::{CollectionType, ImageType};
use serde::Deserialize;
use thiserror::Error;
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

/// Failure while loading and projecting persisted item images.
#[derive(Debug, Error)]
pub enum PersistedDtoImageProjectionError {
    #[error(transparent)]
    Item(#[from] BaseItemError),
    #[error(transparent)]
    Image(#[from] BaseItemImageStoreError),
    #[error("invalid image-projection metadata for base item {item_id}")]
    Metadata {
        item_id: Uuid,
        #[source]
        source: serde_json::Error,
    },
}

/// `PostgreSQL` adapter that preloads persisted items and images before applying
/// the synchronous Jellyfin image-inheritance rules.
#[derive(Clone)]
pub struct PersistedDtoImageProjectionService<C> {
    items: BaseItemRepository,
    images: BaseItemImageRepository,
    cache_tags: C,
}

impl<C> PersistedDtoImageProjectionService<C> {
    #[must_use]
    pub const fn new(
        items: BaseItemRepository,
        images: BaseItemImageRepository,
        cache_tags: C,
    ) -> Self {
        Self {
            items,
            images,
            cache_tags,
        }
    }

    #[must_use]
    pub const fn cache_tags(&self) -> &C {
        &self.cache_tags
    }
}

impl<C: ImageCacheTagProvider> PersistedDtoImageProjectionService<C> {
    /// Loads an item and the parent candidates required by Jellyfin's primary
    /// image inheritance behavior, then projects its DTO image fields.
    ///
    /// The image rows are fetched in one set-based `SeaORM` query after the
    /// small relation set has been resolved. Missing parent rows behave like a
    /// library cache miss; a missing requested item returns `None`.
    ///
    /// # Errors
    ///
    /// Returns a database, corrupt-image-row, or persisted metadata error.
    pub async fn project(
        &self,
        item_id: Uuid,
        options: DtoImageOptions,
    ) -> Result<Option<DtoImageProjection>, PersistedDtoImageProjectionError> {
        let Some(requested) = self.items.get(item_id).await? else {
            return Ok(None);
        };
        let requested_metadata = persisted_metadata(&requested)?;
        let requested_kind = persisted_item_kind(&requested, &requested_metadata);

        let mut models = HashMap::from([(requested.id, requested)]);
        for related_id in related_item_ids(requested_kind) {
            if !models.contains_key(&related_id)
                && let Some(related) = self.items.get(related_id).await?
            {
                models.insert(related.id, related);
            }
        }

        let model_ids = models.keys().copied().collect::<Vec<_>>();
        let mut images_by_item = HashMap::<Uuid, Vec<DtoImage>>::new();
        for image in self.images.list_many(&model_ids).await? {
            images_by_item
                .entry(image.item_id)
                .or_default()
                .push(persisted_image(image));
        }

        let mut projected_items = HashMap::with_capacity(models.len());
        for (id, model) in models {
            let metadata = if id == item_id {
                requested_metadata.clone()
            } else {
                persisted_metadata(&model)?
            };
            projected_items.insert(
                id,
                DtoImageItem {
                    id,
                    kind: persisted_item_kind(&model, &metadata),
                    images: images_by_item.remove(&id).unwrap_or_default(),
                    default_primary_image_aspect_ratio: metadata.default_primary_image_aspect_ratio,
                },
            );
        }

        let Some(requested) = projected_items.remove(&item_id) else {
            return Ok(None);
        };
        let service = DtoImageProjectionService::new(
            PreloadedDtoImageLibrary {
                items: projected_items,
            },
            BorrowedCacheTags(&self.cache_tags),
        );
        Ok(Some(service.project(&requested, options)))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct PersistedDtoImageMetadata {
    view_type: Option<CollectionType>,
    display_parent_id: Option<Uuid>,
    default_primary_image_aspect_ratio: Option<f64>,
}

fn persisted_metadata(
    item: &base_item::Model,
) -> Result<PersistedDtoImageMetadata, PersistedDtoImageProjectionError> {
    item.data
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(|source| PersistedDtoImageProjectionError::Metadata {
            item_id: item.id,
            source,
        })
}

fn persisted_item_kind(
    item: &base_item::Model,
    metadata: &PersistedDtoImageMetadata,
) -> DtoImageItemKind {
    if item.item_type.eq_ignore_ascii_case("Episode") {
        DtoImageItemKind::Episode {
            season_id: item.season_id,
            series_id: item.series_id,
        }
    } else if item.item_type.eq_ignore_ascii_case("UserView") {
        DtoImageItemKind::UserView {
            view_type: metadata.view_type.unwrap_or(CollectionType::Unknown),
            display_parent_id: metadata.display_parent_id,
        }
    } else {
        DtoImageItemKind::Other
    }
}

fn related_item_ids(kind: DtoImageItemKind) -> impl Iterator<Item = Uuid> {
    let ids = match kind {
        DtoImageItemKind::UserView {
            display_parent_id, ..
        } => [display_parent_id, None],
        DtoImageItemKind::Episode {
            season_id,
            series_id,
        } => [series_id, season_id],
        DtoImageItemKind::Other => [None, None],
    };
    ids.into_iter().flatten()
}

fn persisted_image(image: BaseItemImage) -> DtoImage {
    DtoImage {
        image_type: model_image_type(image.image_type),
        path: image.path,
        date_modified: image.date_modified,
        width: image.width,
        height: image.height,
    }
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

struct PreloadedDtoImageLibrary {
    items: HashMap<Uuid, DtoImageItem>,
}

impl DtoImageLibrary for PreloadedDtoImageLibrary {
    fn get_item_by_id(&self, item_id: Uuid) -> Option<DtoImageItem> {
        self.items.get(&item_id).cloned()
    }
}

struct BorrowedCacheTags<'a, C>(&'a C);

impl<C: ImageCacheTagProvider> ImageCacheTagProvider for BorrowedCacheTags<'_, C> {
    fn get_image_cache_tag(&self, item: &DtoImageItem, image: &DtoImage) -> Option<String> {
        self.0.get_image_cache_tag(item, image)
    }
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
