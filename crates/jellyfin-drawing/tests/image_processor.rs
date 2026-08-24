use std::{fs, time::SystemTime};

use image::{DynamicImage, ImageFormat as DecoderFormat, Rgba, RgbaImage};
use jellyfin_drawing::{
    ImageCollageOptions, ImageProcessingError, ImageProcessingRequest, ImageProcessor, ImageSource,
    create_collage,
};
use jellyfin_model::ImageFormat;
use tempfile::TempDir;

fn fixture(width: u32, height: u32) -> (TempDir, ImageSource) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_path = directory.path().join("source.png");
    let image = RgbaImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    });
    image.save(&source_path).expect("write source fixture");
    let date_modified = fs::metadata(&source_path)
        .and_then(|metadata| metadata.modified())
        .expect("source modification time");
    (
        directory,
        ImageSource::new(source_path, date_modified).with_dimensions(width, height),
    )
}

fn processor(directory: &TempDir) -> ImageProcessor {
    ImageProcessor::new(directory.path().join("cache"), 2).expect("image processor")
}

#[tokio::test]
async fn returns_original_for_default_request() {
    let (directory, source) = fixture(80, 40);
    let result = processor(&directory)
        .process(&source, &ImageProcessingRequest::default())
        .await
        .expect("process original");

    assert_eq!(result.path, source.path);
    assert_eq!(result.mime_type, "image/png");
    assert_eq!(result.date_modified, source.date_modified);
}

#[tokio::test]
async fn max_width_resizes_without_changing_aspect_ratio() {
    let (directory, source) = fixture(80, 40);
    let request = ImageProcessingRequest {
        max_width: Some(20),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("resize image");

    assert_ne!(result.path, source.path);
    assert_eq!(image::image_dimensions(result.path).unwrap(), (20, 10));
}

#[tokio::test]
async fn fill_resizes_and_center_crops() {
    let (directory, source) = fixture(80, 40);
    let request = ImageProcessingRequest {
        fill_width: Some(20),
        fill_height: Some(20),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("fill image");
    let image = image::open(result.path).expect("decode result").to_rgba8();

    assert_eq!(image.dimensions(), (20, 20));
    assert_eq!(image.get_pixel(0, 10), &Rgba([255, 0, 0, 255]));
    assert_eq!(image.get_pixel(19, 10), &Rgba([0, 0, 255, 255]));
}

#[tokio::test]
async fn converts_format_and_uses_requested_jpeg_quality() {
    let (directory, source) = fixture(80, 40);
    let request = ImageProcessingRequest {
        format: Some(ImageFormat::Jpg),
        quality: 75,
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("convert image");

    assert_eq!(result.mime_type, "image/jpeg");
    assert_eq!(result.path.extension().unwrap(), "jpg");
    assert_eq!(
        image::ImageReader::open(result.path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .format(),
        Some(DecoderFormat::Jpeg)
    );
}

#[tokio::test]
async fn reuses_stable_cache_file() {
    let (directory, source) = fixture(80, 40);
    let request = ImageProcessingRequest {
        max_width: Some(20),
        ..ImageProcessingRequest::default()
    };
    let processor = processor(&directory);
    let first = processor
        .process(&source, &request)
        .await
        .expect("first process");
    let first_bytes = fs::read(&first.path).expect("read first cache value");
    let first_modified = fs::metadata(&first.path)
        .and_then(|metadata| metadata.modified())
        .expect("first cache modification time");
    let second = processor
        .process(&source, &request)
        .await
        .expect("second process");

    assert_eq!(second.path, first.path);
    assert_eq!(fs::read(&second.path).unwrap(), first_bytes);
    assert_eq!(second.date_modified, first_modified);
}

#[tokio::test]
async fn reports_corrupt_source_as_decode_error() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_path = directory.path().join("bad.png");
    fs::write(&source_path, b"not an image").expect("write corrupt source");
    let source = ImageSource::new(source_path, SystemTime::now());
    let request = ImageProcessingRequest {
        max_width: Some(20),
        ..ImageProcessingRequest::default()
    };
    let error = processor(&directory)
        .process(&source, &request)
        .await
        .expect_err("corrupt image must fail");

    assert!(matches!(error, ImageProcessingError::Decode { .. }));
}

#[tokio::test]
async fn normalizes_empty_badges_and_draws_active_decorations() {
    let (directory, source) = fixture(80, 40);
    let normalized = ImageProcessingRequest {
        percent_played: Some(100.0),
        unplayed_count: Some(0),
        ..ImageProcessingRequest::default()
    };
    assert_eq!(
        processor(&directory)
            .process(&source, &normalized)
            .await
            .expect("inactive badges")
            .path,
        source.path
    );

    let decorated = ImageProcessingRequest {
        percent_played: Some(25.0),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &decorated)
        .await
        .expect("percent played decoration");
    let result = image::open(result.path).unwrap().to_rgba8();
    assert_eq!(result.get_pixel(1, 35), &Rgba([0, 164, 220, 255]));

    let unplayed = ImageProcessingRequest {
        unplayed_count: Some(12),
        foreground_layer: Some("0.6".to_owned()),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &unplayed)
        .await
        .expect("unplayed and foreground decorations");
    assert_ne!(result.path, source.path);
}

#[tokio::test]
async fn background_color_is_composited_beneath_transparency() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_path = directory.path().join("transparent.png");
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 0])))
        .save(&source_path)
        .expect("write transparent fixture");
    let source = ImageSource::new(source_path, SystemTime::now()).with_dimensions(2, 2);
    let request = ImageProcessingRequest {
        background_color: Some("#00ff00".into()),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("apply background");
    let result = image::open(result.path).unwrap().to_rgba8();

    assert_eq!(result.get_pixel(0, 0), &Rgba([0, 255, 0, 255]));
}

#[tokio::test]
async fn resize_preserves_transparency_when_png_is_supported() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_path = directory.path().join("transparent.png");
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 0])))
        .save(&source_path)
        .expect("write transparent fixture");
    let source = ImageSource::new(source_path, SystemTime::now()).with_dimensions(8, 4);
    let request = ImageProcessingRequest {
        max_width: Some(4),
        supported_formats: vec![ImageFormat::Jpg, ImageFormat::Png],
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("resize transparent source");

    assert_eq!(result.mime_type, "image/png");
    assert_eq!(
        image::open(result.path)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0[3],
        0
    );
}

#[tokio::test]
async fn supports_all_required_encoded_formats() {
    let (directory, source) = fixture(8, 4);
    for format in [
        ImageFormat::Bmp,
        ImageFormat::Gif,
        ImageFormat::Jpg,
        ImageFormat::Png,
        ImageFormat::Webp,
    ] {
        let request = ImageProcessingRequest {
            format: Some(format),
            quality: if format == ImageFormat::Png { 80 } else { 90 },
            max_width: Some(4),
            ..ImageProcessingRequest::default()
        };
        let result = processor(&directory)
            .process(&source, &request)
            .await
            .unwrap_or_else(|error| panic!("encode {format:?}: {error}"));
        assert_eq!(image::image_dimensions(result.path).unwrap(), (4, 2));
    }
}

#[tokio::test]
async fn animated_gif_is_returned_unchanged_even_with_transform_options() {
    let (directory, source) = fixture(8, 4);
    let gif_path = directory.path().join("animated.gif");
    image::open(&source.path)
        .unwrap()
        .save_with_format(&gif_path, DecoderFormat::Gif)
        .expect("write gif fixture");
    let gif = ImageSource::new(gif_path.clone(), SystemTime::now()).with_dimensions(8, 4);
    let request = ImageProcessingRequest {
        max_width: Some(4),
        format: Some(ImageFormat::Jpg),
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&gif, &request)
        .await
        .expect("serve gif unchanged");

    assert_eq!(result.path, gif_path);
    assert_eq!(result.mime_type, "image/gif");
}

#[tokio::test]
async fn chooses_first_supported_format_and_does_not_return_unaccepted_source() {
    let (directory, source) = fixture(8, 4);
    let request = ImageProcessingRequest {
        supported_formats: vec![ImageFormat::Bmp, ImageFormat::Jpg],
        ..ImageProcessingRequest::default()
    };
    let result = processor(&directory)
        .process(&source, &request)
        .await
        .expect("negotiate format");

    assert_ne!(result.path, source.path);
    assert_eq!(result.mime_type, "image/bmp");
    assert_eq!(result.path.extension().unwrap(), "bmp");
}

#[tokio::test]
async fn creates_grid_collage_with_requested_dimensions() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut inputs = Vec::new();
    for index in 0..4 {
        let path = directory.path().join(format!("input-{index}.png"));
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            40,
            30,
            Rgba([u8::from(index > 1) * 255, 80, 120, 255]),
        ))
        .save(&path)
        .expect("write collage input");
        inputs.push(path);
    }
    let output = directory.path().join("collage.jpg");
    let result = create_collage(ImageCollageOptions {
        input_paths: inputs,
        output_path: output.clone(),
        width: 120,
        height: 90,
        thumb_layout: false,
    })
    .await
    .expect("create grid collage");

    assert_eq!(result, output);
    assert_eq!(image::image_dimensions(output).unwrap(), (120, 90));
}

#[tokio::test]
async fn creates_thumb_collage_and_rejects_invalid_options() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = directory.path().join("first.png");
    let second = directory.path().join("second.png");
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(80, 40, Rgba([255, 0, 0, 255])))
        .save(&first)
        .expect("first input");
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(80, 40, Rgba([0, 0, 255, 255])))
        .save(&second)
        .expect("second input");
    let output = directory.path().join("thumb.jpg");
    create_collage(ImageCollageOptions {
        input_paths: vec![first, second],
        output_path: output.clone(),
        width: 120,
        height: 60,
        thumb_layout: true,
    })
    .await
    .expect("create thumb collage");
    assert_eq!(image::image_dimensions(output).unwrap(), (120, 60));

    let error = create_collage(ImageCollageOptions {
        input_paths: Vec::new(),
        output_path: directory.path().join("empty.jpg"),
        width: 120,
        height: 60,
        thumb_layout: false,
    })
    .await
    .expect_err("empty collage must fail");
    assert!(matches!(error, ImageProcessingError::InvalidCollageOptions));
}
