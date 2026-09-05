use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::MediaAttachmentService;
use jellyfin_controller::MediaStreamService;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DeviceRepository,
    ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewPerson, NewPersonCredit,
    NewTrickplayInfo, NewUserData, PersonRepository, TrickplayInfoRepository, UserDataRepository,
    entities::{base_item, item_value, user},
};
use jellyfin_model::{MediaAttachment, MediaStream, MediaStreamType};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Items Tests\", DeviceId=\"items-tests\", Device=\"Test\", Version=\"1.0\"";
static ITEMS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn official_items_controller_contract() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .request("/Items", Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::OK
    );

    let missing_user = Uuid::new_v4();
    for route in [
        format!("/Users/{missing_user}/Items"),
        format!("/Users/{missing_user}/Items/Resume"),
    ] {
        assert_eq!(
            fixture
                .request(&route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    for route in [
        format!("/Items?userId={}", fixture.user_id),
        format!("/Users/{}/Items", fixture.user_id),
        format!("/Users/{}/Items/Resume", fixture.user_id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["Items"].is_array());
        assert!(body["TotalRecordCount"].is_number());
        assert!(body["StartIndex"].is_number());
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn legacy_item_collection_accepts_empty_trailing_path_segments() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let item_id = fixture.item_ids[0];

    let anonymous_route = format!("/Users/{}/Items//?ids={item_id}", fixture.user_id);
    assert_eq!(
        fixture.request(&anonymous_route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    for prefix in ["", "/api", "/emby"] {
        for trailing in ["/", "//"] {
            let route = format!(
                "{prefix}/Users/{}/Items{trailing}?ids={item_id}",
                fixture.user_id
            );
            let response = fixture.request(&route, Some(&fixture.user_token)).await;
            assert_eq!(response.status(), StatusCode::OK, "{route}");
            let body = body_json(response).await;
            assert_eq!(body["TotalRecordCount"], 1, "{route}");
            assert_eq!(body["Items"][0]["Id"], item_id.simple().to_string());
        }
    }

    let item_route = format!("/Users/{}/Items/{item_id}", fixture.user_id);
    let response = fixture
        .request(&item_route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await["Id"],
        item_id.simple().to_string()
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn item_metadata_matches_swift_sdk_object_and_array_shapes() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let movie = create_item_with_data(
        &items,
        "MediaBrowser.Controller.Entities.Movies.Movie",
        &format!("Swift DTO {}", fixture.suffix),
        root.id,
        serde_json::json!({
            "Tagline": "One line",
            "EndDate": "2020-01-02",
            "ExtraType": "behindthescenes",
            "AirDays": ["monday", "Funday"],
            "Video3DFormat": "mvc",
            "RemoteTrailers": [
                "https://trailers.example/legacy",
                {"Name": "Trailer", "Url": "https://trailers.example/named"}
            ]
        }),
    )
    .await;
    let studio = ItemValueRepository::new(fixture.database.clone())
        .link(
            movie.id,
            item_value::ItemValueType::Studios,
            &format!("Swift Studio {}", fixture.suffix),
        )
        .await
        .expect("studio link");
    let people = PersonRepository::new(fixture.database.clone());
    let created_people = people
        .replace_credits(
            movie.id,
            vec![
                NewPersonCredit {
                    person: NewPerson::new(format!("Unknown Person {}", fixture.suffix)),
                    person_type: "Cinematographer".to_owned(),
                    role: String::new(),
                    sort_order: None,
                    list_order: 0,
                },
                NewPersonCredit {
                    person: NewPerson::new(format!("Director Person {}", fixture.suffix)),
                    person_type: "director".to_owned(),
                    role: String::new(),
                    sort_order: None,
                    list_order: 1,
                },
            ],
        )
        .await
        .expect("person credits");

    let body = body_json(
        fixture
            .request(
                &format!("/Items?ids={}", movie.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let item = &body["Items"][0];
    assert_eq!(item["Type"], "Movie");
    assert_eq!(item["Taglines"], serde_json::json!(["One line"]));
    assert!(item.get("Tagline").is_none());
    assert_eq!(item["EndDate"], "2020-01-02T00:00:00.000Z");
    assert_eq!(item["ExtraType"], "BehindTheScenes");
    assert_eq!(item["AirDays"], serde_json::json!(["Monday"]));
    assert_eq!(item["Video3DFormat"], "MVC");
    assert_eq!(item["People"][0]["Type"], "Unknown");
    assert_eq!(item["People"][1]["Type"], "Director");
    assert_eq!(
        item["Studios"],
        serde_json::json!([{
            "Name": format!("Swift Studio {}", fixture.suffix),
            "Id": studio.item_value_id.simple().to_string()
        }])
    );
    assert_eq!(
        item["RemoteTrailers"],
        serde_json::json!([
            {"Url": "https://trailers.example/legacy"},
            {"Name": "Trailer", "Url": "https://trailers.example/named"}
        ])
    );

    items.delete(movie.id).await.expect("movie cleanup");
    for person in created_people {
        people.delete(person.id).await.expect("person cleanup");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn item_pages_preserve_image_tags_with_batched_projection() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let images = BaseItemImageRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let first = create_item(
        &items,
        "Movie",
        &format!("First image {}", fixture.suffix),
        root.id,
    )
    .await;
    let second = create_item(
        &items,
        "Movie",
        &format!("Second image {}", fixture.suffix),
        root.id,
    )
    .await;
    for (item, name) in [(&first, "first.jpg"), (&second, "second.jpg")] {
        images
            .replace(
                item.id,
                &[NewBaseItemImage {
                    image_type: BaseItemImageType::Primary,
                    image_index: 0,
                    path: format!("/media/{name}"),
                    date_modified: Utc::now(),
                    width: Some(600),
                    height: Some(900),
                    blurhash: None,
                }],
            )
            .await
            .expect("primary image");
    }

    let body = body_json(
        fixture
            .request(
                &format!("/Items?ids={},{}", first.id, second.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let returned = body["Items"].as_array().expect("items");
    assert_eq!(returned.len(), 2);
    for item in returned {
        assert!(item["ImageTags"]["Primary"].is_string());
    }
    assert_ne!(
        returned[0]["ImageTags"]["Primary"],
        returned[1]["ImageTags"]["Primary"]
    );

    items.delete(first.id).await.expect("first cleanup");
    items.delete(second.id).await.expect("second cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn media_stream_fields_are_projected_for_item_pages() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let path = format!("/media/page-{}.mkv", fixture.suffix);
    let mut media = NewBaseItem::new(Uuid::new_v4(), "Movie");
    media.name = Some(format!("Page Media {}", fixture.suffix));
    media.sort_name = media.name.clone();
    media.parent_id = Some(root.id);
    media.media_type = Some("Video".to_owned());
    media.path = Some(path.clone());
    let media = items.create(media).await.expect("media item");
    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            media.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Video,
                    codec: Some("h264".to_owned()),
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("ac3".to_owned()),
                    language: Some("ger".to_owned()),
                    path: Some(path.clone()),
                    is_default: true,
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 2,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("aac".to_owned()),
                    language: Some("eng".to_owned()),
                    path: Some(path.clone()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 3,
                    stream_type: MediaStreamType::Subtitle,
                    codec: Some("srt".to_owned()),
                    language: Some("eng".to_owned()),
                    path: Some(path),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("media streams");
    let mut remembered = NewUserData::new(media.id, fixture.user_id, media.id.to_string());
    remembered.audio_stream_index = Some(2);
    remembered.subtitle_stream_index = Some(-1);
    UserDataRepository::new(fixture.database.clone())
        .upsert(remembered)
        .await
        .expect("remembered streams");
    MediaAttachmentService::new(fixture.database.clone())
        .save_media_attachments(
            media.id,
            vec![MediaAttachment {
                index: 4,
                codec: Some("mjpeg".to_owned()),
                file_name: Some("poster.jpg".to_owned()),
                mime_type: Some("image/jpeg".to_owned()),
                ..MediaAttachment::default()
            }],
        )
        .await
        .expect("media attachments");

    let route = format!(
        "/Items?recursive=true&searchTerm={}&fields=MediaSources,MediaStreams",
        fixture.suffix
    );
    let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
    let item = body["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == media.id.simple().to_string())
        .expect("projected item");
    assert_eq!(item["MediaSources"].as_array().unwrap().len(), 1);
    assert_eq!(
        item["MediaSources"][0]["MediaAttachments"][0]["FileName"],
        "poster.jpg"
    );
    assert_eq!(item["MediaSources"][0]["MediaAttachments"][0]["Index"], 4);
    assert_eq!(item["MediaSources"][0]["DefaultAudioStreamIndex"], 2);
    assert_eq!(item["MediaSources"][0]["DefaultSubtitleStreamIndex"], -1);
    assert_eq!(item["MediaStreams"].as_array().unwrap().len(), 4);
    assert_eq!(item["MediaSources"][0]["MediaStreams"][0]["Type"], "Video");
    assert_eq!(item["MediaStreams"][0]["Type"], "Video");
    assert_eq!(item["MediaStreams"][1]["Language"], "deu");

    items.delete(media.id).await.expect("media cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn trickplay_field_is_opt_in_batched_and_matches_official_shape() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let mut video = NewBaseItem::new(Uuid::new_v4(), "Movie");
    video.name = Some(format!("Trickplay {}", fixture.suffix));
    video.sort_name = video.name.clone();
    video.parent_id = Some(root.id);
    video.media_type = Some("Video".to_owned());
    let video = items.create(video).await.expect("video item");
    let mut empty_video = NewBaseItem::new(Uuid::new_v4(), "Movie");
    empty_video.name = Some(format!("Empty Trickplay {}", fixture.suffix));
    empty_video.sort_name = empty_video.name.clone();
    empty_video.parent_id = Some(root.id);
    empty_video.media_type = Some("Video".to_owned());
    let empty_video = items.create(empty_video).await.expect("empty video item");
    let trickplay = TrickplayInfoRepository::new(fixture.database.clone());
    trickplay
        .upsert(
            video.id,
            NewTrickplayInfo {
                width: 320,
                height: 180,
                tile_width: 4,
                tile_height: 3,
                thumbnail_count: 25,
                interval: 1_500,
                bandwidth: 42_000,
            },
        )
        .await
        .expect("trickplay metadata");

    let without_field = body_json(
        fixture
            .request(
                &format!("/Items?ids={},{}", video.id, empty_video.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(
        without_field["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("Trickplay").is_none())
    );

    let with_field = body_json(
        fixture
            .request(
                &format!(
                    "/Items?ids={},{}&Fields=Trickplay",
                    video.id, empty_video.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let projected = with_field["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == video.id.simple().to_string())
        .unwrap();
    assert_eq!(
        projected["Trickplay"][video.id.simple().to_string()]["320"],
        serde_json::json!({
            "Width": 320,
            "Height": 180,
            "TileWidth": 4,
            "TileHeight": 3,
            "ThumbnailCount": 25,
            "Interval": 1500,
            "Bandwidth": 42000
        })
    );
    let empty = with_field["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == empty_video.id.simple().to_string())
        .unwrap();
    assert_eq!(empty["Trickplay"], serde_json::json!({}));

    items.delete(video.id).await.expect("video cleanup");
    items
        .delete(empty_video.id)
        .await
        .expect("empty video cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn postgres_item_queries_apply_recursive_filters_and_pagination() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let route = format!(
        "/Items?recursive=true&searchTerm={}&startIndex=1&limit=2",
        fixture.suffix.to_uppercase()
    );
    let response = fixture.request(&route, Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["TotalRecordCount"], 4);
    assert_eq!(body["StartIndex"], 1);
    assert_eq!(body["Items"].as_array().unwrap().len(), 2);
    for item in body["Items"].as_array().unwrap() {
        assert!(!item["ServerId"].as_str().unwrap().is_empty());
        assert!(item["Name"].as_str().unwrap().contains(&fixture.suffix));
        assert!(item.get("item_type").is_none());
    }

    let movie_route = format!(
        "/Items?recursive=true&searchTerm={}&includeItemTypes=Movie",
        fixture.suffix.to_uppercase()
    );
    let movies = body_json(
        fixture
            .request(&movie_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(movies["TotalRecordCount"], 2);
    assert!(
        movies["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Movie")
    );

    let without_total = body_json(
        fixture
            .request(
                &format!(
                    "/Items?recursive=true&searchTerm={}&limit=1&enableTotalRecordCount=false",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(without_total["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        without_total["TotalRecordCount"], 3,
        "search-provider results keep the full candidate count even when total counts are disabled"
    );

    let descending_route = format!(
        "/Items?recursive=true&searchTerm={}&sortBy=SortName&sortOrder=Descending",
        fixture.suffix
    );
    let descending = body_json(
        fixture
            .request(&descending_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    let names = descending["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            format!("A {}", fixture.suffix),
            format!("B {}", fixture.suffix),
            format!("C {}", fixture.suffix),
            format!("D {}", fixture.suffix)
        ]
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn delimited_and_repeated_item_filters_reach_postgres_queries() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;

    let included = body_json(
        fixture
            .request(
                &format!(
                    "/Items?recursive=true&searchTerm={}&includeItemTypes=Movie&includeItemTypes=Episode",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(included["TotalRecordCount"], 3);

    let excluded = body_json(
        fixture
            .request(
                &format!(
                    "/Items?recursive=true&searchTerm={}&excludeItemTypes=Episode,,Video",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(excluded["TotalRecordCount"], 2);

    let selected_with_search = body_json(
        fixture
            .request(
                &format!(
                    "/Items?recursive=true&searchTerm={}&ids={},not-a-uuid,,{}",
                    fixture.suffix, fixture.item_ids[0], fixture.item_ids[2]
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        selected_with_search["TotalRecordCount"], 4,
        "official search combines explicit ids with every provider candidate"
    );

    let selected = body_json(
        fixture
            .request(
                &format!("/Items?ids={},{}", fixture.item_ids[0], fixture.item_ids[2]),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(selected["TotalRecordCount"], 2);

    let media_types = body_json(
        fixture
            .request(
                &format!(
                    "/Items?recursive=true&searchTerm={}&mediaTypes=Video&mediaTypes=Audio",
                    fixture.suffix
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(media_types["TotalRecordCount"], 0);

    fixture.cleanup().await;
}

#[tokio::test]
async fn resume_is_deduplicated_recent_first_paginated_and_user_scoped() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let route = format!(
        "/Users/{}/Items/Resume?searchTerm={}",
        fixture.user_id,
        fixture.suffix.to_uppercase()
    );
    let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
    assert_eq!(body["TotalRecordCount"], 2);
    let ids = body["Items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["Id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            fixture.item_ids[0].simple().to_string(),
            fixture.item_ids[1].simple().to_string()
        ]
    );

    let page_route = format!("{route}&startIndex=1&limit=1");
    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(page["TotalRecordCount"], 2);
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        page["Items"][0]["Id"],
        fixture.item_ids[1].simple().to_string()
    );

    let no_total_route = format!("{route}&limit=1&enableTotalRecordCount=false");
    let no_total = body_json(
        fixture
            .request(&no_total_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().unwrap().len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 1);
    fixture.cleanup().await;
}

#[tokio::test]
async fn collection_folder_with_include_item_types_defaults_to_recursive() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let collection = create_item(
        &items,
        "CollectionFolder",
        &format!("Collection {}", fixture.suffix),
        root.id,
    )
    .await;
    let nested = create_item(
        &items,
        "Folder",
        &format!("Nested {}", fixture.suffix),
        collection.id,
    )
    .await;
    let movie = create_item(
        &items,
        "Movie",
        &format!("Deep Movie {}", fixture.suffix),
        nested.id,
    )
    .await;

    let route = format!("/Items?parentId={}&includeItemTypes=Movie", collection.id);
    let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
    assert_eq!(body["TotalRecordCount"], 1);
    assert_eq!(body["Items"][0]["Id"], movie.id.simple().to_string());

    items
        .delete(collection.id)
        .await
        .expect("collection cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn advanced_items_filters_are_applied_to_the_public_query() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let sd = create_item_with_data(
        &items,
        "Movie",
        &format!("SD {}", fixture.suffix),
        root.id,
        serde_json::json!({ "IsLocked": true }),
    )
    .await;
    let hd = create_item_with_data(
        &items,
        "Movie",
        &format!("HD {}", fixture.suffix),
        root.id,
        serde_json::json!({ "ProviderIds": { "Imdb": "tt1234567" } }),
    )
    .await;
    let four_k = create_item_with_data(
        &items,
        "Movie",
        &format!("4K {}", fixture.suffix),
        root.id,
        serde_json::Value::Null,
    )
    .await;
    let audio = create_item_with_data(
        &items,
        "Audio",
        &format!("Audio {}", fixture.suffix),
        root.id,
        serde_json::Value::Null,
    )
    .await;
    let streams = MediaStreamService::new(fixture.database.clone());
    streams
        .save_media_streams(
            sd.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                width: Some(640),
                height: Some(360),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("sd stream");
    streams
        .save_media_streams(
            hd.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                width: Some(1920),
                height: Some(1080),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("hd stream");
    streams
        .save_media_streams(
            four_k.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                width: Some(3840),
                height: Some(2160),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("4k stream");
    streams
        .save_media_streams(
            audio.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Audio,
                language: Some("eng".to_owned()),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("audio stream");

    let ids = format!("{},{},{},{}", sd.id, hd.id, four_k.id, audio.id);
    let hd_route = format!("/Items?ids={ids}&isHd=true");
    let hd_body = body_json(fixture.request(&hd_route, Some(&fixture.user_token)).await).await;
    assert_eq!(hd_body["TotalRecordCount"], 1);
    assert_eq!(hd_body["Items"][0]["Id"], hd.id.simple().to_string());

    let four_k_route = format!("/Items?ids={ids}&is4K=true");
    let four_k_body = body_json(
        fixture
            .request(&four_k_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(four_k_body["TotalRecordCount"], 1);
    assert_eq!(
        four_k_body["Items"][0]["Id"],
        four_k.id.simple().to_string()
    );

    let language_route = format!("/Items?ids={ids}&audioLanguages=eng");
    let language_body = body_json(
        fixture
            .request(&language_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(language_body["TotalRecordCount"], 1);
    assert_eq!(
        language_body["Items"][0]["Id"],
        audio.id.simple().to_string()
    );

    let provider_route = format!("/Items?ids={ids}&hasImdbId=true");
    let provider_body = body_json(
        fixture
            .request(&provider_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(provider_body["TotalRecordCount"], 1);
    assert_eq!(provider_body["Items"][0]["Id"], hd.id.simple().to_string());

    let exclude_route = format!("/Items?ids={ids}&excludeItemIds={}", hd.id);
    let exclude_body = body_json(
        fixture
            .request(&exclude_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(exclude_body["TotalRecordCount"], 3);
    assert!(
        exclude_body["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Id"] != hd.id.simple().to_string())
    );

    for item_id in [sd.id, hd.id, four_k.id, audio.id] {
        items.delete(item_id).await.expect("item cleanup");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn item_query_authentication_and_target_permissions_are_enforced() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture.request("/Items", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for suffix in ["Items", "Items/Resume"] {
        let admin_route = format!("/Users/{}/{suffix}", fixture.admin_id);
        assert_eq!(
            fixture
                .request(&admin_route, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let user_route = format!("/Users/{}/{suffix}", fixture.user_id);
        assert_eq!(
            fixture
                .request(&user_route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::OK
        );
    }
    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    suffix: String,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_ids: Vec<Uuid>,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        for pattern in ["items-admin-%", "items-user-%"] {
            user::Entity::delete_many()
                .filter(user::Column::Username.like(pattern))
                .exec(&database)
                .await
                .expect("stale items test users must be removed");
        }
        for pattern in [
            "SD %",
            "HD %",
            "4K %",
            "Audio %",
            "Collection %",
            "Nested %",
            "Deep Movie %",
        ] {
            base_item::Entity::delete_many()
                .filter(base_item::Column::Name.like(pattern))
                .exec(&database)
                .await
                .expect("stale items test rows must be removed");
        }
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("items-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("items-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("items-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("items-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let first = create_item(&items, "Movie", &format!("A {suffix}"), root.id).await;
        let second = create_item(&items, "Episode", &format!("B {suffix}"), root.id).await;
        let third = create_item(&items, "Movie", &format!("C {suffix}"), root.id).await;
        let nested = create_item(&items, "Video", &format!("D {suffix}"), third.id).await;

        let user_data = UserDataRepository::new(database.clone());
        let now = Utc::now();
        upsert_resume(
            &user_data,
            user.id,
            first.id,
            "main",
            100,
            now - Duration::hours(2),
        )
        .await;
        upsert_resume(&user_data, user.id, first.id, "alternate", 200, now).await;
        upsert_resume(
            &user_data,
            user.id,
            second.id,
            "main",
            300,
            now - Duration::hours(1),
        )
        .await;
        upsert_resume(&user_data, user.id, third.id, "main", 0, now).await;
        upsert_resume(&user_data, admin.id, nested.id, "main", 400, now).await;

        let state = AppState::new(
            database.clone(),
            "Items Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let app = jellyfin_api::router(state);
        Self {
            database,
            app,
            suffix,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_ids: vec![first.id, second.id, third.id, nested.id],
        }
    }

    async fn request(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let items = BaseItemRepository::new(self.database.clone());
        for item_id in self.item_ids.into_iter().take(3) {
            items.delete(item_id).await.expect("item cleanup");
        }
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    item.is_folder = item_type == "Folder" || item_type == "CollectionFolder";
    repository.create(item).await.expect("item creation")
}

async fn create_item_with_data(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    data: serde_json::Value,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    item.data = (!data.is_null()).then_some(data);
    repository.create(item).await.expect("item creation")
}

async fn upsert_resume(
    repository: &UserDataRepository,
    user_id: Uuid,
    item_id: Uuid,
    key: &str,
    position: i64,
    last_played_date: chrono::DateTime<Utc>,
) {
    let mut data = NewUserData::new(item_id, user_id, key);
    data.playback_position_ticks = position;
    data.last_played_date = Some(last_played_date);
    repository.upsert(data).await.expect("resume data");
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Items Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
