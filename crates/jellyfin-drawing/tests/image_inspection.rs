use std::fs;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use jellyfin_drawing::{ImageInspectionError, inspect_dimensions};

#[tokio::test]
async fn inspects_dimensions_for_supported_raster_formats() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let image = RgbaImage::from_pixel(17, 9, Rgba([20, 40, 60, 255]));

    for (name, format) in [
        ("fixture.bmp", ImageFormat::Bmp),
        ("fixture.gif", ImageFormat::Gif),
        ("fixture.jpg", ImageFormat::Jpeg),
        ("fixture.png", ImageFormat::Png),
        ("fixture.webp", ImageFormat::WebP),
    ] {
        let path = directory.path().join(name);
        let fixture = DynamicImage::ImageRgba8(image.clone());
        let fixture = if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(fixture.to_rgb8())
        } else {
            fixture
        };
        fixture
            .save_with_format(&path, format)
            .unwrap_or_else(|error| panic!("write {format:?} fixture: {error}"));

        assert_eq!(
            inspect_dimensions(&path)
                .await
                .unwrap_or_else(|error| panic!("inspect {format:?} fixture: {error}")),
            (17, 9)
        );
    }
}

#[tokio::test]
async fn detects_format_from_contents_instead_of_extension() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("actually-a-png.jpg");
    RgbaImage::from_pixel(7, 5, Rgba([0, 0, 0, 255]))
        .save_with_format(&path, ImageFormat::Png)
        .expect("write PNG fixture");

    assert_eq!(inspect_dimensions(path).await.unwrap(), (7, 5));
}

#[tokio::test]
async fn reports_corrupt_file_as_unknown_format() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("bad.png");
    fs::write(&path, b"not an image").expect("write corrupt fixture");

    assert!(matches!(
        inspect_dimensions(path).await,
        Err(ImageInspectionError::UnknownFormat(_))
    ));
}

#[tokio::test]
async fn reports_missing_file_as_file_access_error() {
    let directory = tempfile::tempdir().expect("temporary directory");

    assert!(matches!(
        inspect_dimensions(directory.path().join("missing.png")).await,
        Err(ImageInspectionError::FileAccess { .. })
    ));
}
