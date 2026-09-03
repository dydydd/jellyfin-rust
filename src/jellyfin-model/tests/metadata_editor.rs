use jellyfin_model::{CollectionType, MetadataEditorInfo, ParentalRating, ParentalRatingScore};
use serde_json::json;

#[test]
fn metadata_editor_contract_uses_official_pascal_and_rating_names() {
    let info = MetadataEditorInfo {
        parental_rating_options: vec![ParentalRating::new(
            "PG-13",
            Some(ParentalRatingScore {
                score: 13,
                sub_score: Some(0),
            }),
        )],
        content_type: Some(CollectionType::TvShows),
        ..MetadataEditorInfo::default()
    };

    assert_eq!(
        serde_json::to_value(info).unwrap(),
        json!({
            "ParentalRatingOptions": [{
                "Name": "PG-13",
                "Value": 13,
                "RatingScore": { "score": 13, "subScore": 0 }
            }],
            "Countries": [],
            "Cultures": [],
            "ExternalIdInfos": [],
            "ContentType": "tvshows",
            "ContentTypeOptions": []
        })
    );
}
