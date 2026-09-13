use jellyfin_model::{
    BaseItemDto, BaseItemKind, ItemFilter, QueryFilters, QueryResult, ThemeMediaResult,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn query_result_and_item_filter_use_official_contract() {
    let result = QueryResult::from_items(vec!["first".to_owned(), "second".to_owned()]).unwrap();
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["Items"], json!(["first", "second"]));
    assert_eq!(value["TotalRecordCount"], 2);
    assert_eq!(value["StartIndex"], 0);

    assert_eq!(
        serde_json::to_value(ItemFilter::IsFavoriteOrLikes).unwrap(),
        json!("IsFavoriteOrLikes")
    );
    assert_eq!(
        serde_json::to_value(BaseItemKind::MusicVideo).unwrap(),
        json!("MusicVideo")
    );
}

#[test]
fn query_result_rejects_pagination_outside_signed_int32() {
    let maximum = usize::try_from(i32::MAX).unwrap();
    let result = QueryResult::<()>::paged(maximum, maximum, Vec::new()).unwrap();
    assert_eq!(result.start_index, i32::MAX);
    assert_eq!(result.total_record_count, i32::MAX);

    assert!(QueryResult::<()>::paged(maximum + 1, 0, Vec::new()).is_err());
    assert!(QueryResult::<()>::paged(0, maximum + 1, Vec::new()).is_err());
}

#[test]
fn theme_media_result_flattens_query_result_fields() {
    let owner_id = Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();
    let result = ThemeMediaResult {
        query_result: QueryResult::from_items(vec![BaseItemDto::default()]).unwrap(),
        owner_id,
    };

    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["OwnerId"], owner_id.simple().to_string());
    assert_eq!(
        value["Items"],
        json!([{
            "Id": Uuid::nil().simple().to_string(),
            "Type": "AggregateFolder",
            "MediaType": "Unknown"
        }])
    );
    assert_eq!(value["TotalRecordCount"], 1);
    assert_eq!(value["StartIndex"], 0);
}

#[test]
fn query_filters_use_official_names() {
    let filters = QueryFilters {
        tags: vec!["4k".to_owned()],
        ..QueryFilters::default()
    };
    let value = serde_json::to_value(filters).unwrap();
    assert_eq!(value["Genres"], json!([]));
    assert_eq!(value["Tags"], json!(["4k"]));
    assert_eq!(value["AudioLanguages"], json!([]));
    assert_eq!(value["SubtitleLanguages"], json!([]));
}
