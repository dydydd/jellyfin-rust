use std::{cell::RefCell, collections::HashMap};

use chrono::{TimeZone, Utc};
use jellyfin_model::{CollectionType, ImageType};
use jellyfin_server_implementations::{
    DtoImage, DtoImageItem, DtoImageItemKind, DtoImageLibrary, DtoImageOptions,
    DtoImageProjectionService, ImageCacheTagProvider,
};
use uuid::Uuid;

#[test]
fn playlists_user_view_uses_tagged_display_parent_primary() {
    let display_parent = item(
        DtoImageItemKind::Other,
        Some("/images/playlists-custom.jpg"),
        None,
    );
    let user_view = item(
        DtoImageItemKind::UserView {
            view_type: CollectionType::Playlists,
            display_parent_id: Some(display_parent.id),
        },
        Some("/images/generated.png"),
        Some(1.25),
    );
    let service = service([display_parent.clone()], []);

    let projection = service.project(
        &user_view,
        DtoImageOptions {
            include_primary_image_aspect_ratio: true,
            ..DtoImageOptions::default()
        },
    );

    assert_eq!(projection.primary_image_tag, None);
    assert_eq!(
        projection.parent_primary_image_item_id,
        Some(display_parent.id)
    );
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:/images/playlists-custom.jpg")
    );
    assert_eq!(projection.primary_image_aspect_ratio, Some(1.25));
    assert_eq!(&*service.library().lookups.borrow(), &[display_parent.id]);
    assert_eq!(
        tagged_paths(&service),
        ["/images/generated.png", "/images/playlists-custom.jpg"]
    );
}

#[test]
fn playlists_user_view_keeps_own_primary_when_display_parent_has_none() {
    let display_parent = item(DtoImageItemKind::Other, None, None);
    let user_view = item(
        DtoImageItemKind::UserView {
            view_type: CollectionType::Playlists,
            display_parent_id: Some(display_parent.id),
        },
        Some("/images/generated.png"),
        None,
    );
    let service = service([display_parent.clone()], []);

    let projection = service.project(&user_view, DtoImageOptions::default());

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:/images/generated.png")
    );
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.parent_primary_image_tag, None);
    assert_eq!(&*service.library().lookups.borrow(), &[display_parent.id]);
    assert_eq!(tagged_paths(&service), ["/images/generated.png"]);
}

#[test]
fn episode_attaches_season_primary_and_keeps_own_primary_and_aspect_ratio() {
    let series = item(DtoImageItemKind::Other, Some("series.jpg"), Some(2.0 / 3.0));
    let season = item(DtoImageItemKind::Other, Some("season.jpg"), Some(2.0 / 3.0));
    let episode = episode(&season, &series, Some("episode.jpg"), Some(16.0 / 9.0));
    let service = service([season.clone(), series.clone()], []);

    let projection = service.project(
        &episode,
        DtoImageOptions {
            include_primary_image_aspect_ratio: true,
            ..DtoImageOptions::default()
        },
    );

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode.jpg")
    );
    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, Some(season.id));
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:season.jpg")
    );
    assert_eq!(projection.primary_image_aspect_ratio, Some(16.0 / 9.0));
    assert_eq!(
        &*service.library().lookups.borrow(),
        &[series.id, season.id]
    );
    assert_eq!(
        tagged_paths(&service),
        ["episode.jpg", "series.jpg", "season.jpg"]
    );
}

#[test]
fn episode_parent_primary_falls_back_to_series() {
    let series = item(DtoImageItemKind::Other, Some("series.jpg"), Some(2.0 / 3.0));
    let season = item(DtoImageItemKind::Other, None, Some(2.0 / 3.0));
    let episode = episode(&season, &series, Some("episode.jpg"), Some(16.0 / 9.0));
    let service = service([season.clone(), series.clone()], []);

    let projection = service.project(&episode, DtoImageOptions::default());

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode.jpg")
    );
    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, Some(series.id));
    assert_eq!(
        projection.parent_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(
        &*service.library().lookups.borrow(),
        &[series.id, season.id]
    );
}

#[test]
fn episode_without_parent_primaries_keeps_only_own_primary() {
    let series = item(DtoImageItemKind::Other, None, Some(2.0 / 3.0));
    let season = item(DtoImageItemKind::Other, None, Some(2.0 / 3.0));
    let episode = episode(&season, &series, Some("episode.jpg"), Some(16.0 / 9.0));
    let service = service([season.clone(), series.clone()], []);

    let projection = service.project(&episode, DtoImageOptions::default());

    assert_eq!(
        projection.primary_image_tag.as_deref(),
        Some("tag:episode.jpg")
    );
    assert_eq!(projection.series_primary_image_tag, None);
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.parent_primary_image_tag, None);
    assert_eq!(
        &*service.library().lookups.borrow(),
        &[series.id, season.id]
    );
    assert_eq!(tagged_paths(&service), ["episode.jpg"]);
}

#[test]
fn episode_inherits_logo_thumb_and_backdrop_with_official_parent_priority() {
    let mut series = item(DtoImageItemKind::Other, Some("series.jpg"), Some(2.0 / 3.0));
    series.images.extend([
        image(ImageType::Logo, "series-logo.png"),
        image(ImageType::Thumb, "series-thumb.jpg"),
        image(ImageType::Backdrop, "series-backdrop.jpg"),
    ]);
    let mut season = item(DtoImageItemKind::Other, Some("season.jpg"), Some(2.0 / 3.0));
    season.images.extend([
        image(ImageType::Logo, "season-logo.png"),
        image(ImageType::Thumb, "season-thumb.jpg"),
        image(ImageType::Backdrop, "season-backdrop.jpg"),
    ]);
    let episode = episode(&season, &series, None, None);
    let service = service([season.clone(), series.clone()], []);

    let projection = service.project(&episode, DtoImageOptions::default());

    assert_eq!(projection.parent_logo_item_id, Some(season.id));
    assert_eq!(
        projection.parent_logo_image_tag.as_deref(),
        Some("tag:season-logo.png")
    );
    assert_eq!(projection.parent_thumb_item_id, Some(series.id));
    assert_eq!(
        projection.parent_thumb_image_tag.as_deref(),
        Some("tag:series-thumb.jpg")
    );
    assert_eq!(projection.parent_backdrop_image_item_id, Some(season.id));
    assert_eq!(
        projection.parent_backdrop_image_tags,
        ["tag:season-backdrop.jpg"]
    );
}

#[test]
fn local_episode_images_suppress_inherited_logo_thumb_and_backdrop() {
    let mut series = item(DtoImageItemKind::Other, None, None);
    series.images.extend([
        image(ImageType::Logo, "series-logo.png"),
        image(ImageType::Thumb, "series-thumb.jpg"),
        image(ImageType::Backdrop, "series-backdrop.jpg"),
    ]);
    let season = item(DtoImageItemKind::Other, None, None);
    let mut episode = episode(&season, &series, None, None);
    episode.images.extend([
        image(ImageType::Logo, "episode-logo.png"),
        image(ImageType::Thumb, "episode-thumb.jpg"),
        image(ImageType::Backdrop, "episode-backdrop.jpg"),
    ]);
    let service = service([season, series], []);

    let projection = service.project(&episode, DtoImageOptions::default());

    assert_eq!(projection.image_tags["Logo"], "tag:episode-logo.png");
    assert_eq!(projection.image_tags["Thumb"], "tag:episode-thumb.jpg");
    assert_eq!(projection.backdrop_image_tags, ["tag:episode-backdrop.jpg"]);
    assert_eq!(projection.parent_logo_item_id, None);
    assert_eq!(projection.parent_thumb_item_id, None);
    assert_eq!(projection.parent_backdrop_image_item_id, None);
}

#[test]
fn season_uses_series_primary_and_inherited_images_without_parent_primary() {
    let mut series = item(DtoImageItemKind::Other, Some("series.jpg"), Some(2.0 / 3.0));
    series.images.extend([
        image(ImageType::Logo, "series-logo.png"),
        image(ImageType::Thumb, "series-thumb.jpg"),
        image(ImageType::Backdrop, "series-backdrop.jpg"),
    ]);
    let season = season(&series, None, None);
    let service = service([series.clone()], []);

    let projection = service.project(
        &season,
        DtoImageOptions {
            include_primary_image_aspect_ratio: true,
            ..DtoImageOptions::default()
        },
    );

    assert_eq!(
        projection.series_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(projection.parent_primary_image_item_id, None);
    assert_eq!(projection.parent_primary_image_tag, None);
    assert_eq!(projection.primary_image_aspect_ratio, Some(2.0 / 3.0));
    assert_eq!(projection.parent_logo_item_id, Some(series.id));
    assert_eq!(
        projection.parent_logo_image_tag.as_deref(),
        Some("tag:series-logo.png")
    );
    assert_eq!(projection.parent_thumb_item_id, Some(series.id));
    assert_eq!(
        projection.parent_thumb_image_tag.as_deref(),
        Some("tag:series-thumb.jpg")
    );
    assert_eq!(projection.parent_backdrop_image_item_id, Some(series.id));
    assert_eq!(
        projection.parent_backdrop_image_tags,
        ["tag:series-backdrop.jpg"]
    );
}

#[test]
fn disabling_images_keeps_series_primary_without_inherited_parent_fields() {
    let mut series = item(DtoImageItemKind::Other, Some("series.jpg"), None);
    series
        .images
        .push(image(ImageType::Logo, "series-logo.png"));
    let season = season(&series, None, None);
    let episode = episode(&season, &series, None, None);
    let service = service([season.clone(), series], []);
    let options = DtoImageOptions {
        enable_images: false,
        primary_image_limit: 0,
        include_primary_image_aspect_ratio: false,
    };

    for item in [&episode, &season] {
        let projection = service.project(item, options);
        assert_eq!(
            projection.series_primary_image_tag.as_deref(),
            Some("tag:series.jpg")
        );
        assert!(projection.image_tags.is_empty());
        assert_eq!(projection.parent_primary_image_item_id, None);
        assert_eq!(projection.parent_logo_item_id, None);
        assert_eq!(projection.parent_thumb_item_id, None);
        assert_eq!(projection.parent_backdrop_image_item_id, None);
    }
}

#[test]
fn unavailable_cache_tags_do_not_replace_own_image_and_series_ratio_is_fallback() {
    let display_parent = item(DtoImageItemKind::Other, Some("parent.jpg"), Some(2.0 / 3.0));
    let user_view = item(
        DtoImageItemKind::UserView {
            view_type: CollectionType::Playlists,
            display_parent_id: Some(display_parent.id),
        },
        Some("view.jpg"),
        Some(1.25),
    );
    let playlist_service = service([display_parent], ["parent.jpg"]);

    let playlist_projection = playlist_service.project(
        &user_view,
        DtoImageOptions {
            include_primary_image_aspect_ratio: true,
            ..DtoImageOptions::default()
        },
    );

    assert_eq!(
        playlist_projection.primary_image_tag.as_deref(),
        Some("tag:view.jpg")
    );
    assert_eq!(playlist_projection.parent_primary_image_item_id, None);
    assert_eq!(playlist_projection.primary_image_aspect_ratio, Some(1.25));

    let series = item(DtoImageItemKind::Other, Some("series.jpg"), Some(2.0 / 3.0));
    let season = item(DtoImageItemKind::Other, Some("season.jpg"), Some(2.0 / 3.0));
    let episode = episode(&season, &series, Some("episode.jpg"), Some(16.0 / 9.0));
    let episode_service = service(
        [season.clone(), series.clone()],
        ["episode.jpg", "season.jpg"],
    );

    let episode_projection = episode_service.project(
        &episode,
        DtoImageOptions {
            include_primary_image_aspect_ratio: true,
            ..DtoImageOptions::default()
        },
    );

    assert_eq!(episode_projection.primary_image_tag, None);
    assert_eq!(
        episode_projection.series_primary_image_tag.as_deref(),
        Some("tag:series.jpg")
    );
    assert_eq!(
        episode_projection.parent_primary_image_item_id,
        Some(series.id)
    );
    assert_eq!(
        episode_projection.primary_image_aspect_ratio,
        Some(2.0 / 3.0)
    );
    assert_eq!(
        &*episode_service.library().lookups.borrow(),
        &[series.id, season.id]
    );
    assert_eq!(
        tagged_paths(&episode_service),
        ["episode.jpg", "series.jpg", "season.jpg"]
    );
}

#[test]
fn projection_exposes_all_image_tags_and_backdrop_tags() {
    let item = DtoImageItem {
        id: Uuid::new_v4(),
        kind: DtoImageItemKind::Other,
        images: vec![
            image(ImageType::Primary, "poster.jpg"),
            image(ImageType::Logo, "logo.png"),
            image(ImageType::Thumb, "landscape.jpg"),
            image(ImageType::Backdrop, "backdrop-1.jpg"),
            image(ImageType::Backdrop, "backdrop-2.jpg"),
        ],
        path: Some("/media/title.mkv".to_owned()),
        default_primary_image_aspect_ratio: Some(2.0 / 3.0),
    };
    let service = service([], []);

    let projection = service.project(&item, DtoImageOptions::default());

    assert_eq!(projection.image_tags["Primary"], "tag:poster.jpg");
    assert_eq!(projection.image_tags["Logo"], "tag:logo.png");
    assert_eq!(projection.image_tags["Thumb"], "tag:landscape.jpg");
    assert_eq!(
        projection.backdrop_image_tags,
        ["tag:backdrop-1.jpg", "tag:backdrop-2.jpg"]
    );
}

#[test]
fn projection_exposes_only_nonempty_blur_hashes_for_tagged_images() {
    let mut item = item(DtoImageItemKind::Other, None, None);
    item.images = vec![
        image_with_hash(ImageType::Primary, "poster.jpg", Some("poster-hash")),
        image_with_hash(ImageType::Logo, "logo.png", Some("logo-hash")),
        image_with_hash(ImageType::Backdrop, "backdrop.jpg", Some("backdrop-hash")),
        image_with_hash(ImageType::Thumb, "thumb.jpg", Some("")),
    ];
    let service = service([], []);

    let projection = service.project(&item, DtoImageOptions::default());

    assert_eq!(
        projection.image_blur_hashes[&ImageType::Primary],
        HashMap::from([("tag:poster.jpg".to_owned(), "poster-hash".to_owned())])
    );
    assert_eq!(
        projection.image_blur_hashes[&ImageType::Logo],
        HashMap::from([("tag:logo.png".to_owned(), "logo-hash".to_owned())])
    );
    assert_eq!(
        projection.image_blur_hashes[&ImageType::Backdrop],
        HashMap::from([("tag:backdrop.jpg".to_owned(), "backdrop-hash".to_owned())])
    );
    assert!(!projection.image_blur_hashes.contains_key(&ImageType::Thumb));
}

#[test]
fn inherited_and_series_primary_blur_hashes_follow_exposed_tags() {
    let mut series = item(DtoImageItemKind::Other, None, None);
    series.images = vec![
        image_with_hash(ImageType::Primary, "series.jpg", Some("series-hash")),
        image_with_hash(ImageType::Logo, "series-logo.png", Some("logo-hash")),
    ];
    let season = season(&series, None, None);
    let episode = episode(&season, &series, None, None);
    let service = service([season, series], []);

    let projection = service.project(
        &episode,
        DtoImageOptions {
            enable_images: false,
            primary_image_limit: 0,
            include_primary_image_aspect_ratio: false,
        },
    );

    assert_eq!(
        projection.image_blur_hashes[&ImageType::Primary],
        HashMap::from([("tag:series.jpg".to_owned(), "series-hash".to_owned())])
    );
    assert!(!projection.image_blur_hashes.contains_key(&ImageType::Logo));
}

fn item(
    kind: DtoImageItemKind,
    primary_path: Option<&str>,
    default_primary_image_aspect_ratio: Option<f64>,
) -> DtoImageItem {
    DtoImageItem {
        id: Uuid::new_v4(),
        kind,
        images: primary_path.into_iter().map(primary_image).collect(),
        path: None,
        default_primary_image_aspect_ratio,
    }
}

fn episode(
    season: &DtoImageItem,
    series: &DtoImageItem,
    primary_path: Option<&str>,
    default_primary_image_aspect_ratio: Option<f64>,
) -> DtoImageItem {
    item(
        DtoImageItemKind::Episode {
            season_id: Some(season.id),
            series_id: Some(series.id),
        },
        primary_path,
        default_primary_image_aspect_ratio,
    )
}

fn season(
    series: &DtoImageItem,
    primary_path: Option<&str>,
    default_primary_image_aspect_ratio: Option<f64>,
) -> DtoImageItem {
    item(
        DtoImageItemKind::Season {
            series_id: Some(series.id),
        },
        primary_path,
        default_primary_image_aspect_ratio,
    )
}

fn primary_image(path: &str) -> DtoImage {
    image(ImageType::Primary, path)
}

fn image(image_type: ImageType, path: &str) -> DtoImage {
    image_with_hash(image_type, path, None)
}

fn image_with_hash(image_type: ImageType, path: &str, blur_hash: Option<&str>) -> DtoImage {
    DtoImage {
        image_type,
        path: path.to_owned(),
        date_modified: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
        width: None,
        height: None,
        blur_hash: blur_hash.map(str::to_owned),
    }
}

fn service(
    items: impl IntoIterator<Item = DtoImageItem>,
    unavailable_paths: impl IntoIterator<Item = &'static str>,
) -> DtoImageProjectionService<RecordingLibrary, RecordingCacheTags> {
    DtoImageProjectionService::new(
        RecordingLibrary {
            items: items.into_iter().map(|item| (item.id, item)).collect(),
            ..RecordingLibrary::default()
        },
        RecordingCacheTags {
            unavailable_paths: unavailable_paths.into_iter().collect(),
            ..RecordingCacheTags::default()
        },
    )
}

fn tagged_paths(
    service: &DtoImageProjectionService<RecordingLibrary, RecordingCacheTags>,
) -> Vec<String> {
    service.cache_tags().tagged_paths.borrow().clone()
}

#[derive(Debug, Default)]
struct RecordingLibrary {
    items: HashMap<Uuid, DtoImageItem>,
    lookups: RefCell<Vec<Uuid>>,
}

impl DtoImageLibrary for RecordingLibrary {
    fn get_item_by_id(&self, item_id: Uuid) -> Option<DtoImageItem> {
        self.lookups.borrow_mut().push(item_id);
        self.items.get(&item_id).cloned()
    }
}

#[derive(Debug, Default)]
struct RecordingCacheTags {
    unavailable_paths: Vec<&'static str>,
    tagged_paths: RefCell<Vec<String>>,
}

impl ImageCacheTagProvider for RecordingCacheTags {
    fn get_image_cache_tag(&self, _item: &DtoImageItem, image: &DtoImage) -> Option<String> {
        self.tagged_paths.borrow_mut().push(image.path.clone());
        (!self.unavailable_paths.contains(&image.path.as_str()))
            .then(|| format!("tag:{}", image.path))
    }
}
