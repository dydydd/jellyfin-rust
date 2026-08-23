use jellyfin_model::{
    BaseItemDto, BaseItemKind, BaseItemPerson, MediaType, NameGuidPair, PersonKind,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn base_item_dto_uses_official_wire_contract() {
    let id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let dto = BaseItemDto {
        name: Some("Top Gun".to_owned()),
        server_id: Some("server-1".to_owned()),
        id,
        item_type: BaseItemKind::Movie,
        is_folder: Some(false),
        media_type: MediaType::Video,
        genres: Some(vec!["Action".to_owned()]),
        people: Some(vec![BaseItemPerson {
            name: Some("Tony Scott".to_owned()),
            id,
            role: Some("Director".to_owned()),
            person_type: PersonKind::Director,
            ..Default::default()
        }]),
        studios: Some(vec![NameGuidPair {
            name: Some("Paramount".to_owned()),
            id,
        }]),
        ..Default::default()
    };

    let value = serde_json::to_value(dto).unwrap();
    assert_eq!(value["Name"], "Top Gun");
    assert_eq!(value["ServerId"], "server-1");
    assert_eq!(value["Id"], id.simple().to_string());
    assert_eq!(value["Type"], "Movie");
    assert!(value.get("item_type").is_none());
    assert_eq!(value["MediaType"], "Video");
    assert_eq!(value["IsFolder"], false);
    assert_eq!(value["Genres"], json!(["Action"]));
    assert_eq!(value["People"][0]["Id"], id.simple().to_string());
    assert_eq!(value["People"][0]["Type"], "Director");
    assert_eq!(value["Studios"][0]["Name"], "Paramount");
    assert!(value.get("Etag").is_none());
    assert!(value.get("PremiereDate").is_none());
    assert!(value.get("MediaSources").is_none());
}

#[test]
fn base_item_dto_deserializes_official_pascal_case_json() {
    let id = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
    let dto: BaseItemDto = serde_json::from_value(json!({
        "Name": "The Matrix",
        "ServerId": "server-2",
        "Id": id.simple().to_string(),
        "Type": "Movie",
        "MediaType": "Video",
        "IsFolder": false,
        "ParentId": Uuid::nil().simple().to_string(),
        "RunTimeTicks": 9000000000_i64
    }))
    .unwrap();

    assert_eq!(dto.name.as_deref(), Some("The Matrix"));
    assert_eq!(dto.item_type, BaseItemKind::Movie);
    assert_eq!(dto.media_type, MediaType::Video);
    assert_eq!(dto.is_folder, Some(false));
    assert_eq!(dto.parent_id, Some(Uuid::nil()));
    assert_eq!(dto.run_time_ticks, Some(9_000_000_000));
}
