#![allow(clippy::cast_possible_truncation)]
use image::{DynamicImage, GrayImage};
use jellyfin_drawing::{embed_icc_profile, extract_icc_profile, is_grayscale_image};

/// Builds a minimal JPEG: SOI, one APP2 ICC chunk, EOI.
fn jpeg_with_icc(profile: &[u8]) -> Vec<u8> {
    let mut jpeg = vec![0xFF, 0xD8];

    let mut app2 = Vec::new();
    app2.extend_from_slice(b"ICC_PROFILE\0");
    app2.push(1);
    app2.push(1);
    app2.extend_from_slice(profile);

    let length = u16::try_from(app2.len() + 2).expect("fixture chunk fits a JPEG segment");
    jpeg.extend_from_slice(&[0xFF, 0xE2]);
    jpeg.extend_from_slice(&length.to_be_bytes());
    jpeg.extend_from_slice(&app2);

    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

#[test]
fn detects_grayscale_images_correctly() {
    let gray = DynamicImage::ImageLuma8(GrayImage::new(100, 100));
    assert!(is_grayscale_image(&gray));

    let rgb = DynamicImage::ImageRgb8(image::RgbImage::new(100, 100));
    assert!(!is_grayscale_image(&rgb));
}

#[test]
fn extracts_icc_profile_from_jpeg_app2_segment() {
    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
    
    // APP2 segment with ICC_PROFILE
    let dummy_icc = b"ICC_DUMMY_PROFILE_DATA_FOR_TEST";
    let mut app2 = Vec::new();
    app2.extend_from_slice(b"ICC_PROFILE\0");
    app2.push(1); // seq 1
    app2.push(1); // count 1
    app2.extend_from_slice(dummy_icc);

    let len = (app2.len() + 2) as u16;
    jpeg.extend_from_slice(&[0xFF, 0xE2]);
    jpeg.extend_from_slice(&len.to_be_bytes());
    jpeg.extend_from_slice(&app2);

    jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI

    let extracted = extract_icc_profile(&jpeg);
    assert_eq!(extracted.as_deref(), Some(dummy_icc.as_slice()));
}

#[test]
fn extracts_icc_profile_from_webp_iccp_chunk() {
    let mut webp = Vec::new();
    let dummy_icc = b"WEBP_ICC_TEST_DATA";
    
    webp.extend_from_slice(b"RIFF");
    let total_len = (4 + 8 + dummy_icc.len()) as u32;
    webp.extend_from_slice(&total_len.to_le_bytes());
    webp.extend_from_slice(b"WEBP");

    webp.extend_from_slice(b"ICCP");
    let chunk_len = dummy_icc.len() as u32;
    webp.extend_from_slice(&chunk_len.to_le_bytes());
    webp.extend_from_slice(dummy_icc);

    let extracted = extract_icc_profile(&webp);
    assert_eq!(extracted.as_deref(), Some(dummy_icc.as_slice()));
}

#[test]
fn embed_round_trips_through_extract() {
    let profile = b"ICC_DUMMY_PROFILE_DATA_FOR_TEST";
    let mut jpeg = jpeg_with_icc(profile);

    // Simulate an encoder result that dropped the profile on the way out.
    let reencoded = vec![0xFF, 0xD8, 0xFF, 0xD9];
    jpeg.clear();
    jpeg.extend_from_slice(&reencoded);

    embed_icc_profile(&mut jpeg, profile);
    assert_eq!(extract_icc_profile(&jpeg).as_deref(), Some(&profile[..]));
}

#[test]
fn embed_splits_profiles_larger_than_one_chunk() {
    // The ICC spec caps a chunk payload at 65519 bytes; anything larger has to
    // be spread over numbered chunks to stay readable.
    let profile = vec![0x5A_u8; 70_000];
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];

    embed_icc_profile(&mut jpeg, &profile);
    assert_eq!(extract_icc_profile(&jpeg).as_deref(), Some(profile.as_slice()));
}

#[test]
fn embed_ignores_non_jpeg_and_empty_profiles() {
    let mut png_header = vec![0x89, 0x50, 0x4E, 0x47];
    let before = png_header.clone();
    embed_icc_profile(&mut png_header, b"profile");
    assert_eq!(png_header, before);

    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
    let before = jpeg.clone();
    embed_icc_profile(&mut jpeg, &[]);
    assert_eq!(jpeg, before);
}
