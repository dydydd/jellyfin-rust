use jellyfin_model::ImageType;
use jellyfin_providers::manager::item_image_provider::{ImageItem, ImageItemKind};
use jellyfin_providers::manager::local_image_provider::LocalImageProvider;
use std::collections::HashSet;

#[test]
fn local_image_provider_discovers_movie_artwork() {
    let mut files = HashSet::new();
    files.insert("/media/movies/The Matrix (1999)/poster.jpg".to_owned());
    files.insert("/media/movies/The Matrix (1999)/fanart.png".to_owned());
    files.insert("/media/movies/The Matrix (1999)/banner.jpg".to_owned());
    files.insert("/media/movies/The Matrix (1999)/logo.png".to_owned());
    files.insert("/media/movies/The Matrix (1999)/clearart.png".to_owned());
    files.insert("/media/movies/The Matrix (1999)/disc.png".to_owned());

    let item = ImageItem {
        kind: ImageItemKind::Video,
        path: Some("/media/movies/The Matrix (1999)/The Matrix.mkv".to_owned()),
        containing_folder_path: Some("/media/movies/The Matrix (1999)".to_owned()),
        ..Default::default()
    };

    let discovered = LocalImageProvider::get_images(&item, |p| files.contains(p));

    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Primary && img.path.ends_with("poster.jpg"))
    );
    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Backdrop && img.path.ends_with("fanart.png"))
    );
    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Banner && img.path.ends_with("banner.jpg"))
    );
    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Logo && img.path.ends_with("logo.png"))
    );
    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Art && img.path.ends_with("clearart.png"))
    );
    assert!(
        discovered
            .iter()
            .any(|img| img.image_type == ImageType::Disc && img.path.ends_with("disc.png"))
    );
}

#[test]
fn local_image_provider_discovers_season_artwork_from_series() {
    let mut files = HashSet::new();
    files.insert("/media/tv/Breaking Bad/season01-poster.jpg".to_owned());
    files.insert("/media/tv/Breaking Bad/season01-fanart.jpg".to_owned());
    files.insert("/media/tv/Breaking Bad/season-specials-poster.jpg".to_owned());

    let season_1 = LocalImageProvider::get_season_images_from_series_folder(
        "/media/tv/Breaking Bad",
        1,
        |p| files.contains(p),
    );
    assert_eq!(season_1.len(), 2);
    assert!(
        season_1
            .iter()
            .any(|img| img.image_type == ImageType::Primary)
    );
    assert!(
        season_1
            .iter()
            .any(|img| img.image_type == ImageType::Backdrop)
    );

    let specials = LocalImageProvider::get_season_images_from_series_folder(
        "/media/tv/Breaking Bad",
        0,
        |p| files.contains(p),
    );
    assert_eq!(specials.len(), 1);
    assert_eq!(specials[0].image_type, ImageType::Primary);
}
