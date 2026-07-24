use std::path::Path;

use jellyfin_drawing::{BlurHashError, generate_blur_hash};

#[tokio::test]
async fn generates_blur_hashes_for_supported_raster_contents() {
    let directory = tempfile::tempdir().unwrap();
    for (format, extension) in [
        (image::ImageFormat::Bmp, "bmp"),
        (image::ImageFormat::Gif, "gif"),
        (image::ImageFormat::Jpeg, "jpg"),
        (image::ImageFormat::Png, "png"),
        (image::ImageFormat::WebP, "webp"),
    ] {
        let path = directory.path().join(format!("source-{extension}.data"));
        write_image(&path, format);
        let (width, height, hash) = generate_blur_hash(&path).await.unwrap();
        assert_eq!((width, height), (16, 8));
        assert!(!hash.is_empty(), "format: {format:?}");
    }
}

#[tokio::test]
async fn rejects_svg_and_malformed_files_without_panicking() {
    let directory = tempfile::tempdir().unwrap();
    let svg = directory.path().join("source.svg");
    std::fs::write(
        &svg,
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"/>"#,
    )
    .unwrap();
    assert!(matches!(
        generate_blur_hash(&svg).await.unwrap_err(),
        BlurHashError::UnknownFormat(_)
    ));

    let corrupt = directory.path().join("corrupt.png");
    std::fs::write(&corrupt, b"not an image").unwrap();
    assert!(matches!(
        generate_blur_hash(&corrupt).await.unwrap_err(),
        BlurHashError::UnknownFormat(_)
    ));
}

fn write_image(path: &Path, format: image::ImageFormat) {
    let image = image::RgbaImage::from_fn(16, 8, |x, y| {
        image::Rgba([
            u8::try_from(x * 13).unwrap(),
            u8::try_from(y * 29).unwrap(),
            u8::try_from((x + y) * 7).unwrap(),
            255,
        ])
    });
    image::DynamicImage::ImageRgba8(image)
        .save_with_format(path, format)
        .unwrap();
}
