use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemImage, BaseItemImageRepository, BaseItemImageStoreError,
    BaseItemImageType, BaseItemRepository, entities::base_item,
};
use jellyfin_model::{CollectionType, ImageType};
use serde::{Deserialize, Deserializer};
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
    pub blur_hash: Option<String>,
}

/// Stable primary-image metadata used by batched relation DTO projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtoPrimaryImageMetadata {
    pub tag: String,
    pub blur_hash: Option<String>,
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
    Season {
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
    pub path: Option<String>,
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
    pub parent_logo_item_id: Option<Uuid>,
    pub parent_logo_image_tag: Option<String>,
    pub parent_thumb_item_id: Option<Uuid>,
    pub parent_thumb_image_tag: Option<String>,
    pub primary_image_aspect_ratio: Option<f64>,
    /// Whether `ImageTags` is enabled by the DTO image options. The map is
    /// present even when no enabled image types produce a tag.
    pub image_tags_present: bool,
    pub image_tags: HashMap<String, String>,
    /// Whether `BackdropImageTags` is enabled by the DTO image options. The
    /// array is present even when no backdrops exist, but omitted when the
    /// caller filtered the Backdrop image type out.
    pub backdrop_image_tags_present: bool,
    pub backdrop_image_tags: Vec<String>,
    pub parent_backdrop_image_item_id: Option<Uuid>,
    pub parent_backdrop_image_tags: Vec<String>,
    pub image_blur_hashes: HashMap<ImageType, HashMap<String, String>>,
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

impl<C: ImageCacheTagProvider + ?Sized> ImageCacheTagProvider for Arc<C> {
    fn get_image_cache_tag(&self, item: &DtoImageItem, image: &DtoImage) -> Option<String> {
        self.as_ref().get_image_cache_tag(item, image)
    }
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
    /// Projects image DTO fields for several items with set-based item and
    /// image loads.
    ///
    /// # Errors
    ///
    /// Returns a database, corrupt-image-row, or persisted-metadata error.
    pub async fn project_many(
        &self,
        item_ids: &[Uuid],
        options: DtoImageOptions,
    ) -> Result<HashMap<Uuid, DtoImageProjection>, PersistedDtoImageProjectionError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let requested_ids = item_ids.iter().copied().collect::<HashSet<_>>();
        let mut models = self
            .items
            .get_many(item_ids)
            .await?
            .into_iter()
            .map(|item| (item.id, item))
            .collect::<HashMap<_, _>>();
        let mut metadata_by_id = HashMap::with_capacity(models.len());
        let mut related_ids = Vec::new();
        for model in models.values_mut() {
            let metadata = persisted_metadata(model)?;
            related_ids.extend(related_item_ids(persisted_item_kind(model, &metadata)));
            metadata_by_id.insert(model.id, metadata);
        }
        related_ids.retain(|id| !models.contains_key(id));
        related_ids.sort_unstable();
        related_ids.dedup();
        for mut model in self.items.get_many(&related_ids).await? {
            let metadata = persisted_metadata(&mut model)?;
            metadata_by_id.insert(model.id, metadata);
            models.insert(model.id, model);
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
            let metadata = metadata_by_id.remove(&id).unwrap_or_default();
            projected_items.insert(
                id,
                DtoImageItem {
                    id,
                    kind: persisted_item_kind(&model, &metadata),
                    images: images_by_item.remove(&id).unwrap_or_default(),
                    path: model.path,
                    default_primary_image_aspect_ratio: metadata.default_primary_image_aspect_ratio,
                },
            );
        }
        let requested_items = projected_items
            .values()
            .filter(|item| requested_ids.contains(&item.id))
            .cloned()
            .collect::<Vec<_>>();
        let service = DtoImageProjectionService::new(
            PreloadedDtoImageLibrary {
                items: projected_items,
            },
            BorrowedCacheTags(&self.cache_tags),
        );
        Ok(requested_items
            .iter()
            .map(|item| (item.id, service.project(item, options)))
            .collect())
    }

    /// Loads stable primary-image cache tags for several concrete item IDs.
    ///
    /// # Errors
    ///
    /// Returns a database or corrupt-image-row error.
    pub async fn primary_image_tags(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, String>, PersistedDtoImageProjectionError> {
        Ok(self
            .primary_image_metadata(item_ids)
            .await?
            .into_iter()
            .map(|(item_id, metadata)| (item_id, metadata.tag))
            .collect())
    }

    /// Loads stable primary-image cache tags and persisted `BlurHash` values for
    /// several concrete item IDs with one item query and one image query.
    ///
    /// # Errors
    ///
    /// Returns a database or corrupt-image-row error.
    pub async fn primary_image_metadata(
        &self,
        item_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, DtoPrimaryImageMetadata>, PersistedDtoImageProjectionError> {
        if item_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let paths = self
            .items
            .get_many(item_ids)
            .await?
            .into_iter()
            .map(|item| (item.id, item.path))
            .collect::<HashMap<_, _>>();
        let mut metadata = HashMap::new();
        for image in self.images.list_many(item_ids).await? {
            if image.image_type != BaseItemImageType::Primary || image.image_index != 0 {
                continue;
            }
            let Some(path) = paths.get(&image.item_id) else {
                continue;
            };
            let item = DtoImageItem {
                id: image.item_id,
                kind: DtoImageItemKind::Other,
                images: Vec::new(),
                path: path.clone(),
                default_primary_image_aspect_ratio: None,
            };
            let image = persisted_image(image);
            if let Some(tag) = self.cache_tags.get_image_cache_tag(&item, &image) {
                metadata.insert(
                    item.id,
                    DtoPrimaryImageMetadata {
                        tag,
                        blur_hash: image.blur_hash.filter(|value| !value.is_empty()),
                    },
                );
            }
        }
        Ok(metadata)
    }

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
        Ok(self
            .project_many(&[item_id], options)
            .await?
            .remove(&item_id))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct PersistedDtoImageMetadata {
    #[serde(deserialize_with = "deserialize_collection_type")]
    view_type: Option<CollectionType>,
    display_parent_id: Option<Uuid>,
    default_primary_image_aspect_ratio: Option<f64>,
}

fn deserialize_collection_type<'de, D>(deserializer: D) -> Result<Option<CollectionType>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(
        Option::<serde_json::Value>::deserialize(deserializer)?.and_then(|value| {
            value
                .as_str()
                .map(str::trim)
                .and_then(|value| value.parse().ok())
        }),
    )
}

fn persisted_metadata(
    item: &mut base_item::Model,
) -> Result<PersistedDtoImageMetadata, PersistedDtoImageProjectionError> {
    item.data
        .take()
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
    } else if item.item_type.eq_ignore_ascii_case("Season") {
        DtoImageItemKind::Season {
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
        DtoImageItemKind::Season { series_id } => [series_id, None],
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
        blur_hash: image.blurhash,
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
        let primary_image = options
            .includes_primary_images()
            .then(|| self.primary_image_metadata(item))
            .flatten();
        let primary_image_tag = primary_image.as_ref().map(|image| image.tag.clone());
        let primary_image_aspect_ratio = options
            .include_primary_image_aspect_ratio
            .then_some(item.default_primary_image_aspect_ratio)
            .flatten();
        let mut image_tags = HashMap::new();
        let mut backdrop_image_tags = Vec::new();
        let mut image_blur_hashes = HashMap::new();
        if let Some(tag) = primary_image_tag.as_deref() {
            image_tags.insert("Primary".to_owned(), tag.to_owned());
        }
        if let Some(image) = primary_image.as_ref() {
            record_blur_hash(&mut image_blur_hashes, ImageType::Primary, image);
        }
        if options.enable_images {
            self.attach_image_tags(
                item,
                options,
                &mut image_tags,
                &mut backdrop_image_tags,
                &mut image_blur_hashes,
            );
        }
        let mut projection = DtoImageProjection {
            primary_image_tag,
            primary_image_aspect_ratio,
            image_tags_present: options.enable_images,
            image_tags,
            backdrop_image_tags_present: options.enable_images,
            backdrop_image_tags,
            image_blur_hashes,
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
            DtoImageItemKind::Season { series_id } => {
                self.attach_season_images(&mut projection, series_id, options);
            }
            DtoImageItemKind::UserView { .. } | DtoImageItemKind::Other => {}
        }

        projection
    }

    fn attach_image_tags(
        &self,
        item: &DtoImageItem,
        _options: DtoImageOptions,
        image_tags: &mut HashMap<String, String>,
        backdrop_image_tags: &mut Vec<String>,
        image_blur_hashes: &mut HashMap<ImageType, HashMap<String, String>>,
    ) {
        for image in &item.images {
            if image.image_type == ImageType::Primary {
                continue;
            }
            let Some(metadata) = self.tagged_image(item, image) else {
                continue;
            };
            if image.image_type == ImageType::Backdrop {
                backdrop_image_tags.push(metadata.tag.clone());
            } else {
                image_tags.insert(
                    image_type_name(image.image_type).to_owned(),
                    metadata.tag.clone(),
                );
                image_blur_hashes.remove(&image.image_type);
            }
            record_blur_hash(image_blur_hashes, image.image_type, &metadata);
        }
    }

    fn primary_image_metadata(&self, item: &DtoImageItem) -> Option<DtoPrimaryImageMetadata> {
        item.primary_image()
            .and_then(|image| self.tagged_image(item, image))
    }

    fn tagged_image(
        &self,
        item: &DtoImageItem,
        image: &DtoImage,
    ) -> Option<DtoPrimaryImageMetadata> {
        self.cache_tags
            .get_image_cache_tag(item, image)
            .map(|tag| DtoPrimaryImageMetadata {
                tag,
                blur_hash: image.blur_hash.clone().filter(|value| !value.is_empty()),
            })
    }

    fn attach_playlist_display_parent(
        &self,
        projection: &mut DtoImageProjection,
        display_parent_id: Uuid,
    ) {
        let Some(parent) = self.library.get_item_by_id(display_parent_id) else {
            return;
        };
        let Some(metadata) = self.primary_image_metadata(&parent) else {
            return;
        };

        if let Some(tag) = projection.primary_image_tag.take() {
            remove_blur_hash(&mut projection.image_blur_hashes, ImageType::Primary, &tag);
        }
        projection.image_tags.remove("Primary");
        projection.parent_primary_image_item_id = Some(parent.id);
        projection.parent_primary_image_tag = Some(metadata.tag.clone());
        record_blur_hash(
            &mut projection.image_blur_hashes,
            ImageType::Primary,
            &metadata,
        );
    }

    fn attach_episode_images(
        &self,
        projection: &mut DtoImageProjection,
        season_id: Option<Uuid>,
        series_id: Option<Uuid>,
        options: DtoImageOptions,
    ) {
        let series = series_id.and_then(|id| self.library.get_item_by_id(id));
        let season = season_id.and_then(|id| self.library.get_item_by_id(id));
        let series_image = series
            .as_ref()
            .and_then(|series| self.primary_image_metadata(series));
        let series_tag = series_image.as_ref().map(|image| image.tag.clone());

        projection.series_primary_image_tag.clone_from(&series_tag);
        if let Some(image) = series_image.as_ref() {
            record_blur_hash(&mut projection.image_blur_hashes, ImageType::Primary, image);
        }
        if options.include_primary_image_aspect_ratio
            && projection.primary_image_tag.is_none()
            && series_tag.is_some()
        {
            projection.primary_image_aspect_ratio = series
                .as_ref()
                .and_then(|series| series.default_primary_image_aspect_ratio);
        }

        if options.includes_primary_images() {
            let season_image = season
                .as_ref()
                .and_then(|season| self.primary_image_metadata(season));

            if let (Some(season), Some(image)) = (season.as_ref(), season_image) {
                projection.parent_primary_image_item_id = Some(season.id);
                projection.parent_primary_image_tag = Some(image.tag.clone());
                record_blur_hash(
                    &mut projection.image_blur_hashes,
                    ImageType::Primary,
                    &image,
                );
            } else if let (Some(series), Some(tag)) = (series.as_ref(), series_tag) {
                projection.parent_primary_image_item_id = Some(series.id);
                projection.parent_primary_image_tag = Some(tag);
            }
        }

        if options.enable_images {
            self.attach_inherited_images(
                projection,
                [season.as_ref(), series.as_ref()].into_iter().flatten(),
            );
        }
    }

    fn attach_season_images(
        &self,
        projection: &mut DtoImageProjection,
        series_id: Option<Uuid>,
        options: DtoImageOptions,
    ) {
        let series = series_id.and_then(|id| self.library.get_item_by_id(id));
        let series_image = series
            .as_ref()
            .and_then(|series| self.primary_image_metadata(series));
        let series_tag = series_image.as_ref().map(|image| image.tag.clone());

        projection.series_primary_image_tag = series_tag;
        if let Some(image) = series_image.as_ref() {
            record_blur_hash(&mut projection.image_blur_hashes, ImageType::Primary, image);
        }
        if options.include_primary_image_aspect_ratio
            && projection.primary_image_tag.is_none()
            && projection.series_primary_image_tag.is_some()
        {
            projection.primary_image_aspect_ratio = series
                .as_ref()
                .and_then(|series| series.default_primary_image_aspect_ratio);
        }

        if options.enable_images {
            self.attach_inherited_images(projection, series.as_ref());
        }
    }

    fn attach_inherited_images<'a>(
        &self,
        projection: &mut DtoImageProjection,
        parents: impl IntoIterator<Item = &'a DtoImageItem>,
    ) {
        let inherits_logo = !projection.image_tags.contains_key("Logo");
        let inherits_thumb = !projection.image_tags.contains_key("Thumb");
        let inherits_backdrop = projection.backdrop_image_tags.is_empty();

        for parent in parents {
            if inherits_logo
                && projection.parent_logo_item_id.is_none()
                && let Some(image) = self.image_metadata(parent, ImageType::Logo)
            {
                projection.parent_logo_item_id = Some(parent.id);
                projection.parent_logo_image_tag = Some(image.tag.clone());
                record_blur_hash(&mut projection.image_blur_hashes, ImageType::Logo, &image);
            }

            // Jellyfin deliberately lets a Series thumb replace a Season thumb. Iterating
            // Season then Series and keeping the last tagged parent preserves that behavior.
            if inherits_thumb && let Some(image) = self.image_metadata(parent, ImageType::Thumb) {
                if let Some(tag) = projection.parent_thumb_image_tag.take() {
                    remove_blur_hash(&mut projection.image_blur_hashes, ImageType::Thumb, &tag);
                }
                projection.parent_thumb_item_id = Some(parent.id);
                projection.parent_thumb_image_tag = Some(image.tag.clone());
                record_blur_hash(&mut projection.image_blur_hashes, ImageType::Thumb, &image);
            }

            if inherits_backdrop && projection.parent_backdrop_image_item_id.is_none() {
                let images = self.image_metadata_many(parent, ImageType::Backdrop);
                if !images.is_empty() {
                    projection.parent_backdrop_image_item_id = Some(parent.id);
                    projection.parent_backdrop_image_tags =
                        images.iter().map(|image| image.tag.clone()).collect();
                    for image in &images {
                        record_blur_hash(
                            &mut projection.image_blur_hashes,
                            ImageType::Backdrop,
                            image,
                        );
                    }
                }
            }
        }
    }

    fn image_metadata(
        &self,
        item: &DtoImageItem,
        image_type: ImageType,
    ) -> Option<DtoPrimaryImageMetadata> {
        item.images
            .iter()
            .find(|image| image.image_type == image_type)
            .and_then(|image| self.tagged_image(item, image))
    }

    fn image_metadata_many(
        &self,
        item: &DtoImageItem,
        image_type: ImageType,
    ) -> Vec<DtoPrimaryImageMetadata> {
        item.images
            .iter()
            .filter(|image| image.image_type == image_type)
            .filter_map(|image| self.tagged_image(item, image))
            .collect()
    }
}

fn record_blur_hash(
    image_blur_hashes: &mut HashMap<ImageType, HashMap<String, String>>,
    image_type: ImageType,
    image: &DtoPrimaryImageMetadata,
) {
    if let Some(blur_hash) = image.blur_hash.as_ref() {
        image_blur_hashes
            .entry(image_type)
            .or_default()
            .insert(image.tag.clone(), blur_hash.clone());
    }
}

fn remove_blur_hash(
    image_blur_hashes: &mut HashMap<ImageType, HashMap<String, String>>,
    image_type: ImageType,
    tag: &str,
) {
    let Some(hashes) = image_blur_hashes.get_mut(&image_type) else {
        return;
    };
    hashes.remove(tag);
    if hashes.is_empty() {
        image_blur_hashes.remove(&image_type);
    }
}

const fn image_type_name(image_type: ImageType) -> &'static str {
    match image_type {
        ImageType::Primary => "Primary",
        ImageType::Art => "Art",
        ImageType::Backdrop => "Backdrop",
        ImageType::Banner => "Banner",
        ImageType::Logo => "Logo",
        ImageType::Thumb => "Thumb",
        ImageType::Disc => "Disc",
        ImageType::Box => "Box",
        ImageType::Screenshot => "Screenshot",
        ImageType::Menu => "Menu",
        ImageType::Chapter => "Chapter",
        ImageType::BoxRear => "BoxRear",
        ImageType::Profile => "Profile",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_view_type_is_case_insensitive_and_tolerates_unknown_values() {
        for (value, expected) in [
            (
                serde_json::json!({ "ViewType": "MoViEs" }),
                Some(CollectionType::Movies),
            ),
            (serde_json::json!({ "ViewType": " mixed " }), None),
            (serde_json::json!({ "ViewType": "not-a-collection" }), None),
            (serde_json::json!({ "ViewType": 1 }), None),
            (
                serde_json::json!({ "ViewType": { "Value": "movies" } }),
                None,
            ),
            (serde_json::json!({ "ViewType": ["movies"] }), None),
            (serde_json::json!({ "ViewType": true }), None),
            (serde_json::json!({ "ViewType": null }), None),
            (serde_json::json!({}), None),
        ] {
            let metadata: PersistedDtoImageMetadata =
                serde_json::from_value(value).expect("persisted image metadata");
            assert_eq!(metadata.view_type, expected);
        }
    }
}
