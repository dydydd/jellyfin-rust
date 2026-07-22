use jellyfin_model::{ImageFormat, InvalidImageFormat};

#[test]
fn all_official_image_formats_have_mime_types() {
    let expected = [
        (ImageFormat::Bmp, "image/bmp"),
        (ImageFormat::Gif, "image/gif"),
        (ImageFormat::Jpg, "image/jpeg"),
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Webp, "image/webp"),
        (ImageFormat::Svg, "image/svg+xml"),
    ];
    assert_eq!(ImageFormat::ALL.len(), 6);
    for (format, mime_type) in expected {
        assert_eq!(format.get_mime_type(), mime_type);
    }
}

#[test]
fn official_invalid_values_are_rejected_for_mime_types() {
    for value in [i32::MIN, i32::MAX, -1, 6] {
        assert_eq!(ImageFormat::try_from(value), Err(InvalidImageFormat(value)));
    }
}

#[test]
fn all_official_image_formats_have_extensions() {
    let expected = [
        (ImageFormat::Bmp, ".bmp"),
        (ImageFormat::Gif, ".gif"),
        (ImageFormat::Jpg, ".jpg"),
        (ImageFormat::Png, ".png"),
        (ImageFormat::Webp, ".webp"),
        (ImageFormat::Svg, ".svg"),
    ];
    assert_eq!(ImageFormat::ALL.len(), 6);
    for (format, extension) in expected {
        assert_eq!(format.get_extension(), extension);
    }
}

#[test]
fn official_invalid_values_are_rejected_for_extensions() {
    for value in [i32::MIN, i32::MAX, -1, 6] {
        assert_eq!(ImageFormat::try_from(value), Err(InvalidImageFormat(value)));
    }
}
