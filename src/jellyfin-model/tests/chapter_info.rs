use jellyfin_model::ChapterInfo;
use serde_json::json;

#[test]
fn missing_chapter_image_uses_official_datetime_min_value() {
    assert_eq!(
        serde_json::to_value(ChapterInfo::default()).unwrap(),
        json!({
            "StartPositionTicks": 0,
            "ImageDateModified": "0001-01-01T00:00:00.0000000Z"
        })
    );
}
