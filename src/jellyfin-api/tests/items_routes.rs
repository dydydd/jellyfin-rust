use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::{
    ItemByNameKind, ItemByNameService, MediaAttachmentService, MediaStreamService, PlaylistService,
    UserService,
};
use jellyfin_data::{
    ApiKeyRepository, BaseItemImageRepository, BaseItemImageType, BaseItemRepository,
    ChapterRepository, DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage,
    NewChapter, NewDevice, NewPerson, NewPersonCredit, NewTrickplayInfo, NewUserData,
    PersonRepository, TrickplayInfoRepository, UserDataRepository,
    entities::{base_item, item_value, user},
};
use jellyfin_model::{MediaAttachment, MediaStream, MediaStreamType, UserPolicy};
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
async fn settings_fields_are_requested_for_pages_and_defaulted_for_item_details() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let configured = create_item_with_data(
        &items,
        "Movie",
        &format!("SD Settings configured {}", fixture.suffix),
        root.id,
        serde_json::json!({
            "LockedFields": ["name", "OFFICIALRATING", "PluginField"],
            "IsLocked": true,
            "ForcedSortName": "Forced Movie",
            "PreferredMetadataLanguage": "ja",
            "PreferredMetadataCountryCode": "JP"
        }),
    )
    .await;
    let empty = create_item_with_data(
        &items,
        "Movie",
        &format!("SD Settings empty {}", fixture.suffix),
        root.id,
        serde_json::json!({}),
    )
    .await;

    let page_route = format!("/Items?ids={},{}", configured.id, empty.id);
    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    for dto in page["Items"].as_array().expect("item page") {
        for property in [
            "LockedFields",
            "LockData",
            "ForcedSortName",
            "PreferredMetadataLanguage",
            "PreferredMetadataCountryCode",
        ] {
            assert!(dto.get(property).is_none(), "unexpected {property}: {dto}");
        }
    }

    for (query_name, field_name) in [
        ("Fields", "Settings"),
        ("fields", "settings"),
        ("fields", "SETTINGS"),
    ] {
        let route = format!(
            "/Items?ids={},{}&{query_name}={field_name}",
            configured.id, empty.id
        );
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let page = body_json(response).await;
        let page = page["Items"].as_array().expect("settings item page");
        let configured_dto = page
            .iter()
            .find(|dto| dto["Id"] == configured.id.simple().to_string())
            .expect("configured settings dto");
        assert_eq!(
            configured_dto["LockedFields"],
            serde_json::json!(["Name", "OfficialRating"]),
            "{route}"
        );
        assert_eq!(configured_dto["LockData"], true, "{route}");
        assert_eq!(configured_dto["ForcedSortName"], "Forced Movie", "{route}");
        assert_eq!(configured_dto["PreferredMetadataLanguage"], "ja", "{route}");
        assert_eq!(
            configured_dto["PreferredMetadataCountryCode"], "JP",
            "{route}"
        );

        let empty_dto = page
            .iter()
            .find(|dto| dto["Id"] == empty.id.simple().to_string())
            .expect("empty settings dto");
        assert_eq!(empty_dto["LockedFields"], serde_json::json!([]), "{route}");
        assert_eq!(empty_dto["LockData"], false, "{route}");
        for property in [
            "ForcedSortName",
            "PreferredMetadataLanguage",
            "PreferredMetadataCountryCode",
        ] {
            assert!(
                empty_dto.get(property).is_none(),
                "unexpected {property}: {route}"
            );
        }
    }

    for route in [
        format!("/Items/{}?UserId={}", configured.id, fixture.user_id),
        format!("/Users/{}/Items/{}", fixture.user_id, configured.id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let dto = body_json(response).await;
        assert_eq!(
            dto["LockedFields"],
            serde_json::json!(["Name", "OfficialRating"])
        );
        assert_eq!(dto["LockData"], true);
        assert_eq!(dto["ForcedSortName"], "Forced Movie");
        assert_eq!(dto["PreferredMetadataLanguage"], "ja");
        assert_eq!(dto["PreferredMetadataCountryCode"], "JP");
    }

    items.delete(empty.id).await.expect("empty item cleanup");
    items
        .delete(configured.id)
        .await
        .expect("configured item cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn external_urls_follow_official_field_and_tv_hierarchy_contract() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let empty_movie = create_item_with_data(
        &items,
        "Movie",
        &format!("SD External URLs empty {}", fixture.suffix),
        root.id,
        serde_json::json!({}),
    )
    .await;

    let unrequested = body_json(
        fixture
            .request(
                &format!("/Items?ids={}", empty_movie.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(
        unrequested["Items"][0].get("ExternalUrls").is_none(),
        "ExternalUrls must be omitted unless requested: {unrequested}"
    );

    for route in [
        format!("/Items?ids={}&Fields=ExternalUrls", empty_movie.id),
        format!("/Items?ids={}&fields=externalurls", empty_movie.id),
        format!("/Items?ids={}&fields=EXTERNALURLS", empty_movie.id),
        format!("/Items?ids={}&Fields=13", empty_movie.id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let page = body_json(response).await;
        assert_eq!(page["Items"][0]["ExternalUrls"], serde_json::json!([]));
    }

    for route in [
        format!("/Items/{}?UserId={}", empty_movie.id, fixture.user_id),
        format!("/Users/{}/Items/{}", fixture.user_id, empty_movie.id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let dto = body_json(response).await;
        assert_eq!(dto["ExternalUrls"], serde_json::json!([]), "{route}");
    }

    let mut series_one = NewBaseItem::new(Uuid::new_v4(), "Series");
    series_one.name = Some(format!("SD External URLs series one {}", fixture.suffix));
    series_one.sort_name = series_one.name.clone();
    series_one.parent_id = Some(root.id);
    series_one.is_folder = true;
    series_one.data = Some(serde_json::json!({
        "ProviderIds": {
            "Imdb": "ttseries1",
            "Tmdb": "1399",
            "Tvdb": "must-not-project",
            "TvMaze": "must-not-project"
        },
        "DisplayOrder": "OriginalAirDate"
    }));
    let series_one = items.create(series_one).await.expect("first series");

    let mut season_one = NewBaseItem::new(Uuid::new_v4(), "Season");
    season_one.name = Some(format!("SD External URLs season one {}", fixture.suffix));
    season_one.sort_name = season_one.name.clone();
    season_one.parent_id = Some(series_one.id);
    season_one.series_id = Some(series_one.id);
    season_one.index_number = Some(2);
    season_one.is_folder = true;
    season_one.data = Some(serde_json::json!({
        "ProviderIds": {
            "Tmdb": "season-own-must-not-be-used",
            "Zap2It": "season zap"
        }
    }));
    let season_one = items.create(season_one).await.expect("first season");

    let mut episode_one = NewBaseItem::new(Uuid::new_v4(), "Episode");
    episode_one.name = Some(format!("SD External URLs episode one {}", fixture.suffix));
    episode_one.sort_name = episode_one.name.clone();
    episode_one.parent_id = Some(season_one.id);
    episode_one.series_id = Some(series_one.id);
    episode_one.season_id = Some(season_one.id);
    episode_one.index_number = Some(5);
    episode_one.parent_index_number = Some(99);
    episode_one.data = Some(serde_json::json!({
        "ProviderIds": {
            "Imdb": "ttepisode1",
            "Tmdb": "episode-own-must-not-be-used",
            "Zap2It": "episode zap",
            "Tvdb": "must-not-project"
        }
    }));
    let episode_one = items.create(episode_one).await.expect("first episode");

    let mut series_two = NewBaseItem::new(Uuid::new_v4(), "Series");
    series_two.name = Some(format!("SD External URLs series two {}", fixture.suffix));
    series_two.sort_name = series_two.name.clone();
    series_two.parent_id = Some(root.id);
    series_two.is_folder = true;
    series_two.data = Some(serde_json::json!({
        "ProviderIds": { "Tmdb": "42" },
        "DisplayOrder": "OriginalAirDate"
    }));
    let series_two = items.create(series_two).await.expect("second series");

    let mut season_two = NewBaseItem::new(Uuid::new_v4(), "Season");
    season_two.name = Some(format!("SD External URLs season two {}", fixture.suffix));
    season_two.sort_name = season_two.name.clone();
    season_two.parent_id = Some(series_two.id);
    season_two.series_id = Some(series_two.id);
    season_two.index_number = Some(7);
    season_two.is_folder = true;
    let season_two = items.create(season_two).await.expect("second season");

    let mut episode_two = NewBaseItem::new(Uuid::new_v4(), "Episode");
    episode_two.name = Some(format!("SD External URLs episode two {}", fixture.suffix));
    episode_two.sort_name = episode_two.name.clone();
    episode_two.parent_id = Some(season_two.id);
    episode_two.series_id = Some(series_two.id);
    episode_two.season_id = Some(season_two.id);
    episode_two.index_number = Some(3);
    let episode_two = items.create(episode_two).await.expect("second episode");

    let page_route = format!(
        "/Items?ids={},{},{}&fields=externalurls",
        season_one.id, episode_one.id, episode_two.id
    );
    let response = fixture
        .request(&page_route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK, "{page_route}");
    let page = body_json(response).await;
    let page = page["Items"].as_array().expect("external URL item page");
    let find = |id: Uuid| {
        page.iter()
            .find(|dto| dto["Id"] == id.simple().to_string())
            .expect("requested external URL dto")
    };
    assert_eq!(
        find(season_one.id)["ExternalUrls"],
        serde_json::json!([
            {
                "Name": "IMDb",
                "Url": "https://www.imdb.com/title/ttseries1/episodes/?season=2"
            },
            {
                "Name": "TMDB",
                "Url": "https://www.themoviedb.org/tv/1399/season/2"
            },
            {
                "Name": "Zap2It",
                "Url": "http://tvlistings.zap2it.com/overview.html?programSeriesId=season zap"
            }
        ])
    );
    let episode_one_urls = serde_json::json!([
        {
            "Name": "IMDb",
            "Url": "https://www.imdb.com/title/ttepisode1"
        },
        {
            "Name": "TMDB",
            "Url": "https://www.themoviedb.org/tv/1399/season/2/episode/5"
        },
        {
            "Name": "Zap2It",
            "Url": "http://tvlistings.zap2it.com/overview.html?programSeriesId=episode zap"
        }
    ]);
    assert_eq!(find(episode_one.id)["ExternalUrls"], episode_one_urls);
    assert_eq!(
        find(episode_two.id)["ExternalUrls"],
        serde_json::json!([{
            "Name": "TMDB",
            "Url": "https://www.themoviedb.org/tv/42/season/7/episode/3"
        }])
    );

    let detail_route = format!("/Items/{}?UserId={}", episode_one.id, fixture.user_id);
    let response = fixture
        .request(&detail_route, Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK, "{detail_route}");
    let detail = body_json(response).await;
    assert_eq!(detail["ExternalUrls"], episode_one_urls);

    for (id, label) in [
        (episode_two.id, "second episode"),
        (season_two.id, "second season"),
        (series_two.id, "second series"),
        (episode_one.id, "first episode"),
        (season_one.id, "first season"),
        (series_one.id, "first series"),
        (empty_movie.id, "empty movie"),
    ] {
        items.delete(id).await.expect(label);
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn remote_trailers_follow_official_field_and_single_item_contract() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let populated = create_item_with_data(
        &items,
        "Movie",
        &format!("SD Remote trailers populated {}", fixture.suffix),
        root.id,
        serde_json::json!({
            "HomePageUrl": "https://homepage.example/must-not-project",
            "RemoteTrailers": [
                "https://trailers.example/legacy-first",
                {
                    "Name": "Named second",
                    "Url": "https://trailers.example/named-second"
                }
            ]
        }),
    )
    .await;
    let empty = create_item_with_data(
        &items,
        "Movie",
        &format!("SD Remote trailers empty {}", fixture.suffix),
        root.id,
        serde_json::json!({}),
    )
    .await;
    let expected = serde_json::json!([
        {"Url": "https://trailers.example/legacy-first"},
        {
            "Name": "Named second",
            "Url": "https://trailers.example/named-second"
        }
    ]);

    let unrequested_route = format!("/Items?Ids={},{}", populated.id, empty.id);
    let unrequested = body_json(
        fixture
            .request(&unrequested_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    for dto in unrequested["Items"]
        .as_array()
        .expect("unrequested item page")
    {
        assert!(
            dto.get("RemoteTrailers").is_none(),
            "RemoteTrailers must be omitted unless requested: {dto}"
        );
        assert!(dto.get("HomePageUrl").is_none(), "{dto}");
        assert!(dto.get("TrailerUrls").is_none(), "{dto}");
    }

    for (query_name, field_name) in [
        ("Fields", "RemoteTrailers"),
        ("fields", "remotetrailers"),
        ("fields", "REMOTETRAILERS"),
        ("Fields", "35"),
    ] {
        let route = format!(
            "/Items?Ids={},{}&{query_name}={field_name}",
            populated.id, empty.id
        );
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let body = body_json(response).await;
        let page = body["Items"].as_array().expect("remote trailer item page");
        let populated_dto = page
            .iter()
            .find(|dto| dto["Id"] == populated.id.simple().to_string())
            .expect("populated remote trailer dto");
        let empty_dto = page
            .iter()
            .find(|dto| dto["Id"] == empty.id.simple().to_string())
            .expect("empty remote trailer dto");
        assert_eq!(populated_dto["RemoteTrailers"], expected, "{route}");
        assert_eq!(
            empty_dto["RemoteTrailers"],
            serde_json::json!([]),
            "{route}"
        );
        assert!(populated_dto.get("HomePageUrl").is_none(), "{route}");
        assert!(populated_dto.get("TrailerUrls").is_none(), "{route}");
    }

    for (route, expected_trailers) in [
        (
            format!("/Items/{}?UserId={}", populated.id, fixture.user_id),
            expected.clone(),
        ),
        (
            format!("/Users/{}/Items/{}", fixture.user_id, populated.id),
            expected,
        ),
        (
            format!("/Items/{}?UserId={}", empty.id, fixture.user_id),
            serde_json::json!([]),
        ),
        (
            format!("/Users/{}/Items/{}", fixture.user_id, empty.id),
            serde_json::json!([]),
        ),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let dto = body_json(response).await;
        assert_eq!(dto["RemoteTrailers"], expected_trailers, "{route}");
        assert!(dto.get("HomePageUrl").is_none(), "{route}");
        assert!(dto.get("TrailerUrls").is_none(), "{route}");
    }

    items.delete(empty.id).await.expect("empty item cleanup");
    items
        .delete(populated.id)
        .await
        .expect("populated item cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn chapters_follow_official_field_order_image_and_version_contract() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let chapters = ChapterRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let marker = Uuid::new_v4().simple().to_string();

    let mut primary = NewBaseItem::new(Uuid::new_v4(), "Movie");
    primary.name = Some(format!("SD Chapters primary {marker}"));
    primary.sort_name = primary.name.clone();
    primary.parent_id = Some(root.id);
    primary.media_type = Some("Video".to_owned());
    primary.path = Some(format!("/media/{marker}-primary.mkv"));
    let primary = items.create(primary).await.expect("primary chapter item");

    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
    alternate.name = Some(format!("SD Chapters alternate {marker}"));
    alternate.sort_name = alternate.name.clone();
    alternate.parent_id = Some(root.id);
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some(format!("/media/{marker}-alternate.mkv"));
    alternate.primary_version_id = Some(primary.id);
    let alternate = items
        .create(alternate)
        .await
        .expect("alternate chapter item");

    let stored = chapters
        .replace(
            primary.id,
            vec![
                NewChapter {
                    index_number: 9,
                    start_position_ticks: 900,
                    end_position_ticks: 999,
                    name: Some("Later".to_owned()),
                },
                NewChapter {
                    index_number: 2,
                    start_position_ticks: 100,
                    end_position_ticks: 199,
                    name: Some("Opening".to_owned()),
                },
            ],
        )
        .await
        .expect("primary chapters");
    let image_modified = "2026-01-02T03:04:05Z"
        .parse::<chrono::DateTime<Utc>>()
        .expect("chapter image date");
    chapters
        .set_image_data(
            stored[1].id,
            format!("/metadata/{marker}-chapter.jpg"),
            image_modified,
        )
        .await
        .expect("chapter image metadata");
    chapters
        .replace(
            alternate.id,
            vec![NewChapter {
                index_number: 0,
                start_position_ticks: 50,
                end_position_ticks: 99,
                name: Some("Alternate only".to_owned()),
            }],
        )
        .await
        .expect("alternate chapters");

    let without_fields = body_json(
        fixture
            .request(
                &format!("/Items?Ids={}", primary.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(without_fields["Items"][0].get("Chapters").is_none());

    for (query_name, fields) in [
        ("Fields", "Chapters"),
        ("fields", "chapters"),
        ("fields", "4"),
    ] {
        let route = format!(
            "/Items?Ids={}&{query_name}={fields}&EnableImages=false&EnableImageTypes=Primary&ImageTypeLimit=0",
            primary.id
        );
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let body = body_json(response).await;
        let dto = &body["Items"][0];
        let projected = dto["Chapters"].as_array().expect("requested chapters");
        assert_eq!(projected.len(), 2, "{route}");
        assert_eq!(projected[0]["StartPositionTicks"], 100, "{route}");
        assert_eq!(projected[0]["Name"], "Opening", "{route}");
        assert_eq!(
            projected[0]["ImagePath"],
            format!("/metadata/{marker}-chapter.jpg"),
            "{route}"
        );
        assert_eq!(
            projected[0]["ImageDateModified"], "2026-01-02T03:04:05.0000000Z",
            "{route}"
        );
        assert_eq!(
            projected[0]["ImageTag"],
            jellyfin_controller::image_cache_tag(
                primary.path.as_deref().expect("primary path"),
                image_modified
            ),
            "{route}"
        );
        assert_eq!(projected[1]["StartPositionTicks"], 900, "{route}");
        assert_eq!(
            projected[1]["ImageDateModified"], "0001-01-01T00:00:00.0000000Z",
            "{route}"
        );
        assert!(projected[1].get("ImagePath").is_none(), "{route}");
        assert!(projected[1].get("ImageTag").is_none(), "{route}");
        assert!(dto.get("ImageTags").is_none(), "{route}");
        assert_eq!(dto["ImageBlurHashes"], serde_json::json!({}), "{route}");
    }

    for route in [
        format!("/Items/{}?UserId={}", primary.id, fixture.user_id),
        format!("/Users/{}/Items/{}", fixture.user_id, primary.id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let dto = body_json(response).await;
        assert_eq!(dto["Chapters"].as_array().unwrap().len(), 2, "{route}");
        assert_eq!(dto["Chapters"][0]["Name"], "Opening", "{route}");
    }

    let alternate_route = format!("/Users/{}/Items/{}", fixture.user_id, alternate.id);
    let alternate_dto = body_json(
        fixture
            .request(&alternate_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(alternate_dto["Id"], alternate.id.simple().to_string());
    assert_eq!(
        alternate_dto["Chapters"],
        serde_json::json!([{
            "StartPositionTicks": 50,
            "Name": "Alternate only",
            "ImageDateModified": "0001-01-01T00:00:00.0000000Z"
        }])
    );

    let empty_route = format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_ids[0]);
    let empty_dto = body_json(
        fixture
            .request(&empty_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(empty_dto["Chapters"], serde_json::json!([]));
    let empty_page = body_json(
        fixture
            .request(
                &format!("/Items?Ids={}&Fields=Chapters", fixture.item_ids[0]),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(empty_page["Items"][0]["Chapters"], serde_json::json!([]));

    items
        .delete(alternate.id)
        .await
        .expect("alternate chapter cleanup");
    items
        .delete(primary.id)
        .await
        .expect("primary chapter cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn download_and_play_access_fields_follow_official_field_and_user_policy_rules() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.name = Some(format!("Access movie {}", fixture.suffix));
    movie.sort_name = movie.name.clone();
    movie.parent_id = Some(root.id);
    movie.path = Some(format!("/media/access-{}.mkv", fixture.suffix));
    let movie = items.create(movie).await.expect("access movie");

    let mut photo = NewBaseItem::new(Uuid::new_v4(), "Photo");
    photo.name = Some(format!("Access photo {}", fixture.suffix));
    photo.sort_name = photo.name.clone();
    photo.parent_id = Some(root.id);
    let photo = items.create(photo).await.expect("access photo");

    let route = format!("/Items?ids={},{}", movie.id, photo.id);
    let page = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
    for dto in page["Items"].as_array().expect("access page") {
        assert!(dto.get("CanDownload").is_none(), "{dto}");
        assert!(dto.get("PlayAccess").is_none(), "{dto}");
    }

    for fields in [
        "CanDownload,PlayAccess",
        "candownload,playaccess",
        "CANDOWNLOAD,PLAYACCESS",
        "2,23",
    ] {
        let page = body_json(
            fixture
                .request(
                    &format!("{route}&fields={fields}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        for dto in page["Items"].as_array().expect("requested access page") {
            assert_eq!(dto["CanDownload"], true, "{fields}: {dto}");
            assert_eq!(dto["PlayAccess"], "Full", "{fields}: {dto}");
        }
    }

    let users = UserService::new(fixture.database.clone());
    let original_policy: UserPolicy = serde_json::from_value(
        users
            .get(fixture.user_id)
            .await
            .expect("access user")
            .policy,
    )
    .expect("access user policy");
    let mut blocked_policy = original_policy.clone();
    blocked_policy.enable_content_downloading = false;
    blocked_policy.enable_media_playback = false;
    users
        .update_policy(fixture.user_id, &blocked_policy)
        .await
        .expect("blocked access policy");

    for route in [
        format!("/Items?ids={}&fields=CanDownload,PlayAccess", movie.id),
        format!("/Items/{}?UserId={}", movie.id, fixture.user_id),
        format!("/Users/{}/Items/{}", fixture.user_id, movie.id),
    ] {
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let body = body_json(response).await;
        let dto = body
            .get("Items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .unwrap_or(&body);
        assert_eq!(dto["CanDownload"], false, "{route}: {dto}");
        assert_eq!(dto["PlayAccess"], "None", "{route}: {dto}");
    }

    let global = body_json(
        fixture
            .request(
                &format!(
                    "/Items?ids={}&fields=2,23&api_key={}",
                    movie.id, fixture.api_key_token
                ),
                None,
            )
            .await,
    )
    .await;
    let global = &global["Items"][0];
    assert_eq!(global["CanDownload"], true);
    assert!(global.get("PlayAccess").is_none(), "{global}");

    let targeted = body_json(
        fixture
            .request(
                &format!(
                    "/Items?ids={}&UserId={}&Fields=CanDownload,PlayAccess&api_key={}",
                    movie.id, fixture.user_id, fixture.api_key_token
                ),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(targeted["Items"][0]["CanDownload"], false);
    assert_eq!(targeted["Items"][0]["PlayAccess"], "None");

    users
        .update_policy(fixture.user_id, &original_policy)
        .await
        .expect("restore access policy");
    items.delete(photo.id).await.expect("access photo cleanup");
    items.delete(movie.id).await.expect("access movie cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn can_delete_uses_intrinsic_folder_policy_and_single_playlist_override() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let collection = create_item(
        &items,
        "CollectionFolder",
        &format!("SD Delete collection {}", fixture.suffix),
        root.id,
    )
    .await;
    let mut scoped_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    scoped_movie.name = Some(format!("SD Delete scoped {}", fixture.suffix));
    scoped_movie.sort_name = scoped_movie.name.clone();
    scoped_movie.parent_id = Some(collection.id);
    scoped_movie.path = Some(format!("/media/delete-scoped-{}.mkv", fixture.suffix));
    let scoped_movie = items.create(scoped_movie).await.expect("scoped movie");

    let mut outside_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    outside_movie.name = Some(format!("SD Delete outside {}", fixture.suffix));
    outside_movie.sort_name = outside_movie.name.clone();
    outside_movie.parent_id = Some(root.id);
    outside_movie.path = Some(format!("/media/delete-outside-{}.mkv", fixture.suffix));
    let outside_movie = items.create(outside_movie).await.expect("outside movie");

    let no_path = create_item(
        &items,
        "Movie",
        &format!("SD Delete no path {}", fixture.suffix),
        collection.id,
    )
    .await;
    let playlist = PlaylistService::new(fixture.database.clone());
    let playlist_id = playlist
        .create(
            format!("SD Delete playlist {}", fixture.suffix),
            fixture.user_id,
            false,
            None,
            &[],
            &[],
        )
        .await
        .expect("owner playlist");

    let ids = format!("{},{},{}", scoped_movie.id, outside_movie.id, no_path.id);
    let unrequested = body_json(
        fixture
            .request(&format!("/Items?Ids={ids}"), Some(&fixture.user_token))
            .await,
    )
    .await;
    for dto in unrequested["Items"].as_array().expect("unrequested page") {
        assert!(dto.get("CanDelete").is_none(), "{dto}");
    }

    let users = UserService::new(fixture.database.clone());
    let original_policy: UserPolicy = serde_json::from_value(
        users
            .get(fixture.user_id)
            .await
            .expect("delete policy user")
            .policy,
    )
    .expect("delete policy");
    let mut scoped_policy = original_policy.clone();
    scoped_policy.enable_content_deletion = false;
    scoped_policy.enable_content_deletion_from_folders = vec![collection.id.to_string()];
    users
        .update_policy(fixture.user_id, &scoped_policy)
        .await
        .expect("scoped delete policy");

    for field in ["CanDelete", "candelete", "CANDELETE", "1"] {
        let page = body_json(
            fixture
                .request(
                    &format!("/Items?Ids={ids}&Fields={field}"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        let page = page["Items"].as_array().expect("delete page");
        let can_delete = |id: Uuid| {
            page.iter()
                .find(|dto| dto["Id"] == id.simple().to_string())
                .expect("requested delete DTO")["CanDelete"]
                .as_bool()
                .expect("CanDelete bool")
        };
        assert!(can_delete(scoped_movie.id), "{field}");
        assert!(!can_delete(outside_movie.id), "{field}");
        assert!(!can_delete(no_path.id), "{field}");
    }

    let global = body_json(
        fixture
            .request(
                &format!(
                    "/Items?Ids={ids}&Fields=CanDelete&api_key={}",
                    fixture.api_key_token
                ),
                None,
            )
            .await,
    )
    .await;
    let global = global["Items"].as_array().expect("global delete page");
    for id in [scoped_movie.id, outside_movie.id] {
        let dto = global
            .iter()
            .find(|dto| dto["Id"] == id.simple().to_string())
            .expect("global delete DTO");
        assert_eq!(dto["CanDelete"], true, "{dto}");
    }
    let no_path_dto = global
        .iter()
        .find(|dto| dto["Id"] == no_path.id.simple().to_string())
        .expect("global no-path DTO");
    assert_eq!(no_path_dto["CanDelete"], false);

    let targeted = body_json(
        fixture
            .request(
                &format!(
                    "/Items?Ids={ids}&UserId={}&Fields=CanDelete&api_key={}",
                    fixture.user_id, fixture.api_key_token
                ),
                None,
            )
            .await,
    )
    .await;
    let targeted = targeted["Items"].as_array().expect("targeted delete page");
    let targeted_outside = targeted
        .iter()
        .find(|dto| dto["Id"] == outside_movie.id.simple().to_string())
        .expect("targeted outside DTO");
    assert_eq!(targeted_outside["CanDelete"], false);

    for route in [
        format!("/Items/{}?UserId={}", scoped_movie.id, fixture.user_id),
        format!("/Users/{}/Items/{}", fixture.user_id, scoped_movie.id),
    ] {
        let dto = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(dto["CanDelete"], true, "{route}: {dto}");
    }
    for route in [
        format!("/Items/Root?Fields=CanDownload&UserId={}", fixture.user_id),
        format!("/Users/{}/Items/Root?Fields=CanDownload", fixture.user_id),
    ] {
        let dto = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(dto["CanDelete"], false, "{route}: {dto}");
    }

    let playlist_page = body_json(
        fixture
            .request(
                &format!("/Items?Ids={playlist_id}&Fields=CanDelete"),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(playlist_page["Items"][0]["CanDelete"], false);
    let owner_detail = body_json(
        fixture
            .request(
                &format!("/Items/{playlist_id}?UserId={}", fixture.user_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(owner_detail["CanDelete"], true);
    let admin_detail = body_json(
        fixture
            .request(
                &format!("/Items/{playlist_id}?UserId={}", fixture.admin_id),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_detail["CanDelete"], true);

    users
        .update_policy(fixture.user_id, &original_policy)
        .await
        .expect("restore delete policy");
    items.delete(playlist_id).await.expect("playlist cleanup");
    items
        .delete(outside_movie.id)
        .await
        .expect("outside movie cleanup");
    items
        .delete(collection.id)
        .await
        .expect("collection cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn has_lyrics_uses_persisted_lyric_streams_for_item_and_page_dtos() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut audio = NewBaseItem::new(Uuid::new_v4(), "Audio");
    audio.name = Some(format!("Has lyrics audio {}", fixture.suffix));
    audio.sort_name = audio.name.clone();
    audio.parent_id = Some(root.id);
    audio.media_type = Some("Audio".to_owned());
    audio.path = Some(format!("/media/has-lyrics-{}.flac", fixture.suffix));
    audio.data = Some(serde_json::json!({
        "Lyrics": { "Lyrics": [{ "Text": "stale JSON" }] }
    }));
    let audio = items.create(audio).await.expect("audio item");

    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.name = Some(format!("Has lyrics movie {}", fixture.suffix));
    movie.sort_name = movie.name.clone();
    movie.parent_id = Some(root.id);
    movie.media_type = Some("Video".to_owned());
    movie.path = Some(format!("/media/has-lyrics-{}.mkv", fixture.suffix));
    movie.data = Some(serde_json::json!({
        "Lyrics": { "Lyrics": [{ "Text": "not an audio item" }] }
    }));
    let movie = items.create(movie).await.expect("movie item");

    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            movie.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Lyric,
                codec: Some("lrc".to_owned()),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("non-audio lyric stream");

    let page_route = format!("/Items?ids={},{}", audio.id, movie.id);
    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    let audio_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == audio.id.simple().to_string())
        .expect("audio page dto");
    assert_eq!(audio_dto["HasLyrics"], false);
    let movie_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == movie.id.simple().to_string())
        .expect("movie page dto");
    assert!(movie_dto.get("HasLyrics").is_none());

    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            audio.id,
            vec![
                MediaStream {
                    index: 0,
                    stream_type: MediaStreamType::Audio,
                    codec: Some("flac".to_owned()),
                    ..MediaStream::default()
                },
                MediaStream {
                    index: 1,
                    stream_type: MediaStreamType::Lyric,
                    codec: Some("lrc".to_owned()),
                    ..MediaStream::default()
                },
            ],
        )
        .await
        .expect("audio lyric stream");

    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    let audio_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == audio.id.simple().to_string())
        .expect("audio page dto with lyrics");
    assert_eq!(audio_dto["HasLyrics"], true);

    for route in [
        format!("/Users/{}/Items/{}", fixture.user_id, audio.id),
        format!("/Items/{}?UserId={}", audio.id, fixture.user_id),
    ] {
        let dto = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(dto["HasLyrics"], true, "{route}");
    }

    items.delete(movie.id).await.expect("movie cleanup");
    items.delete(audio.id).await.expect("audio cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn audio_page_dtos_project_requested_stream_and_source_fields() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut audio = NewBaseItem::new(Uuid::new_v4(), "Audio");
    audio.name = Some(format!("Audio stream fields {}", fixture.suffix));
    audio.sort_name = audio.name.clone();
    audio.parent_id = Some(root.id);
    audio.media_type = Some("Audio".to_owned());
    audio.path = Some(format!("/media/audio-streams-{}.flac", fixture.suffix));
    audio.data = Some(serde_json::json!({"Container": "flac,mp3"}));
    let audio = items.create(audio).await.expect("audio item");

    // Legacy rows can identify an AudioBook only by item type. Official AudioBook inherits Audio
    // and therefore still exposes one placeholder media source when it has no persisted path.
    let mut audio_book = NewBaseItem::new(Uuid::new_v4(), "AudioBook");
    audio_book.name = Some(format!("AudioBook stream fields {}", fixture.suffix));
    audio_book.sort_name = audio_book.name.clone();
    audio_book.parent_id = Some(root.id);
    let audio_book = items.create(audio_book).await.expect("audio book item");

    let streams = MediaStreamService::new(fixture.database.clone());
    for item_id in [audio.id, audio_book.id] {
        streams
            .save_media_streams(
                item_id,
                vec![
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("flac".to_owned()),
                        language: Some("deu".to_owned()),
                        bit_rate: Some(256_000),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        language: Some("eng".to_owned()),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Lyric,
                        codec: Some("lrc".to_owned()),
                        language: Some("jpn".to_owned()),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("audio streams");
    }

    for (fields, expect_top_level, expect_sources) in [
        ("MediaSources", false, true),
        ("MediaStreams", true, false),
        ("MediaSources,MediaStreams", true, true),
    ] {
        let route = format!("/Items?ids={},{}&Fields={fields}", audio.id, audio_book.id);
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        for item in [&audio, &audio_book] {
            let dto = body["Items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|dto| dto["Id"] == item.id.simple().to_string())
                .expect("audio DTO");

            if item.id == audio.id {
                assert_eq!(dto["Container"], "flac,mp3", "{route}: {dto}");
            } else {
                assert!(dto.get("Container").is_none(), "{route}: {dto}");
            }

            if expect_top_level {
                assert_audio_stream_fields(&dto["MediaStreams"], &route);
            } else {
                assert!(dto.get("MediaStreams").is_none(), "{route}: {dto}");
            }

            if expect_sources {
                let sources = dto["MediaSources"].as_array().expect("media sources");
                assert_eq!(sources.len(), 1, "{route}: {dto}");
                assert_eq!(sources[0]["Bitrate"], 256_000, "{route}: {dto}");
                assert_audio_stream_fields(&sources[0]["MediaStreams"], &route);
                if item.id == audio.id {
                    assert_eq!(sources[0]["Container"], "flac", "{route}: {dto}");
                } else {
                    assert_eq!(sources[0]["Type"], "Placeholder", "{route}: {dto}");
                }
            } else {
                assert!(dto.get("MediaSources").is_none(), "{route}: {dto}");
            }
        }
    }

    items
        .delete(audio_book.id)
        .await
        .expect("audio book cleanup");
    items.delete(audio.id).await.expect("audio cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn has_subtitles_uses_persisted_subtitle_streams_for_item_and_page_dtos() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut video = NewBaseItem::new(Uuid::new_v4(), "Movie");
    video.name = Some(format!("Has subtitles video {}", fixture.suffix));
    video.sort_name = video.name.clone();
    video.parent_id = Some(root.id);
    video.media_type = Some("Video".to_owned());
    video.path = Some(format!("/media/has-subtitles-{}.mkv", fixture.suffix));
    video.data = Some(serde_json::json!({ "HasSubtitles": false }));
    let video = items.create(video).await.expect("video item");

    let mut stale_video = NewBaseItem::new(Uuid::new_v4(), "Episode");
    stale_video.name = Some(format!("Stale subtitles video {}", fixture.suffix));
    stale_video.sort_name = stale_video.name.clone();
    stale_video.parent_id = Some(root.id);
    stale_video.media_type = Some("Video".to_owned());
    stale_video.path = Some(format!("/media/stale-subtitles-{}.mkv", fixture.suffix));
    stale_video.data = Some(serde_json::json!({ "HasSubtitles": true }));
    let stale_video = items.create(stale_video).await.expect("stale video item");

    let mut audio = NewBaseItem::new(Uuid::new_v4(), "Audio");
    audio.name = Some(format!("Subtitle stream audio {}", fixture.suffix));
    audio.sort_name = audio.name.clone();
    audio.parent_id = Some(root.id);
    audio.media_type = Some("Audio".to_owned());
    audio.path = Some(format!("/media/subtitle-stream-{}.flac", fixture.suffix));
    let audio = items.create(audio).await.expect("audio item");

    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            audio.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Subtitle,
                codec: Some("srt".to_owned()),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("non-video subtitle stream");

    let page_route = format!("/Items?ids={},{},{}", video.id, stale_video.id, audio.id);
    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    for dto in page["Items"].as_array().unwrap() {
        assert!(dto.get("HasSubtitles").is_none(), "{dto}");
    }

    for route in [
        format!("/Users/{}/Items/{}", fixture.user_id, stale_video.id),
        format!("/Items/{}?UserId={}", stale_video.id, fixture.user_id),
    ] {
        let dto = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert!(dto.get("HasSubtitles").is_none(), "{route}: {dto}");
    }

    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            video.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Subtitle,
                codec: Some("ass".to_owned()),
                language: Some("jpn".to_owned()),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("video subtitle stream");

    let page = body_json(
        fixture
            .request(&page_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    let video_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == video.id.simple().to_string())
        .expect("video page dto with subtitles");
    assert_eq!(video_dto["HasSubtitles"], true);
    let stale_video_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == stale_video.id.simple().to_string())
        .expect("stale video page dto");
    assert!(stale_video_dto.get("HasSubtitles").is_none());
    let audio_dto = page["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == audio.id.simple().to_string())
        .expect("audio page dto");
    assert!(audio_dto.get("HasSubtitles").is_none());

    for route in [
        format!("/Users/{}/Items/{}", fixture.user_id, video.id),
        format!("/Items/{}?UserId={}", video.id, fixture.user_id),
    ] {
        let dto = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(dto["HasSubtitles"], true, "{route}");
    }

    items
        .delete_many(&[video.id, stale_video.id, audio.id])
        .await
        .expect("subtitle DTO cleanup");
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

    for prefix in ["", "/api"] {
        for path in [
            format!("Users/{}/Items", fixture.user_id),
            format!("users/{}/items", fixture.user_id),
        ] {
            for trailing in ["/", "//"] {
                let route = format!("{prefix}/{path}{trailing}?ids={item_id}");
                let response = fixture.request(&route, Some(&fixture.user_token)).await;
                assert_eq!(response.status(), StatusCode::OK, "{route}");
                let body = body_json(response).await;
                assert_eq!(body["TotalRecordCount"], 1, "{route}");
                assert_eq!(body["Items"][0]["Id"], item_id.simple().to_string());
            }
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
    let item_values = ItemValueRepository::new(fixture.database.clone());
    let studio = item_values
        .link(
            movie.id,
            item_value::ItemValueType::Studios,
            &format!("Swift Studio {}", fixture.suffix),
        )
        .await
        .expect("studio link");
    let genre = item_values
        .link(
            movie.id,
            item_value::ItemValueType::Genre,
            &format!("Swift Genre {}", fixture.suffix),
        )
        .await
        .expect("genre link");
    let people = PersonRepository::new(fixture.database.clone());
    let unknown_name = format!("Unknown Person {}", fixture.suffix);
    let director_name = format!("Director Person {}", fixture.suffix);
    let created_people = people
        .replace_credits(
            movie.id,
            vec![
                NewPersonCredit {
                    person: NewPerson::new(&unknown_name),
                    person_type: "Cinematographer".to_owned(),
                    role: String::new(),
                    sort_order: None,
                    list_order: 0,
                },
                NewPersonCredit {
                    person: NewPerson::new(&director_name),
                    person_type: "director".to_owned(),
                    role: String::new(),
                    sort_order: None,
                    list_order: 1,
                },
            ],
        )
        .await
        .expect("person credits");
    let item_by_name = ItemByNameService::new(fixture.database.clone());
    item_by_name.set_directories(
        fixture.storage_root.join("programdata"),
        fixture.storage_root.join("metadata"),
    );
    let canonical_people = vec![
        item_by_name
            .resolve_direct(ItemByNameKind::Person, &unknown_name)
            .await
            .expect("canonical unknown Person"),
        item_by_name
            .resolve_direct(ItemByNameKind::Person, &director_name)
            .await
            .expect("canonical director Person"),
    ];
    assert_ne!(canonical_people[0].id, created_people[0].id);
    assert_ne!(canonical_people[1].id, created_people[1].id);
    let mut legacy_person = NewBaseItem::new(Uuid::new_v4(), "Person");
    legacy_person.name = Some(unknown_name.clone());
    legacy_person.sort_name = legacy_person.name.clone();
    let legacy_person = items
        .create(legacy_person)
        .await
        .expect("same-name legacy Person");
    BaseItemImageRepository::new(fixture.database.clone())
        .replace(
            canonical_people[0].id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: format!("/media/canonical-person-{}.jpg", fixture.suffix),
                date_modified: Utc::now(),
                width: Some(300),
                height: Some(450),
                blurhash: Some("canonical-person-blurhash".to_owned()),
            }],
        )
        .await
        .expect("canonical Person primary image");

    let body = body_json(
        fixture
            .request(
                &format!("/Items?ids={}&Fields=RemoteTrailers", movie.id),
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
        item["People"][0]["Id"],
        canonical_people[0].id.simple().to_string()
    );
    assert_eq!(
        item["People"][1]["Id"],
        canonical_people[1].id.simple().to_string()
    );
    assert!(item["People"][0]["PrimaryImageTag"].is_string());
    let person_tag = item["People"][0]["PrimaryImageTag"]
        .as_str()
        .expect("canonical Person image tag");
    assert_eq!(
        item["People"][0]["ImageBlurHashes"]["Primary"][person_tag],
        "canonical-person-blurhash"
    );
    assert!(item["People"][1].get("PrimaryImageTag").is_none());
    assert!(item["People"][1].get("ImageBlurHashes").is_none());
    assert_eq!(item["ImageBlurHashes"], serde_json::json!({}));
    assert_eq!(
        item["Studios"],
        serde_json::json!([{
            "Name": format!("Swift Studio {}", fixture.suffix),
            "Id": studio.item_value_id.simple().to_string()
        }])
    );
    assert_eq!(
        item["Genres"],
        serde_json::json!([format!("Swift Genre {}", fixture.suffix)])
    );
    assert_eq!(
        item["GenreItems"],
        serde_json::json!([{
            "Name": format!("Swift Genre {}", fixture.suffix),
            "Id": genre.item_value_id.simple().to_string()
        }])
    );
    assert_eq!(
        item["RemoteTrailers"],
        serde_json::json!([
            {"Url": "https://trailers.example/legacy"},
            {"Name": "Trailer", "Url": "https://trailers.example/named"}
        ])
    );

    for person_ids in ["personIds", "PersonIds", "personids"] {
        let filtered = body_json(
            fixture
                .request(
                    &format!(
                        "/Items?Ids={}&{person_ids}={}&PersonTypes=Cinematographer",
                        movie.id, canonical_people[0].id
                    ),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_eq!(filtered["TotalRecordCount"], 1, "{person_ids}");
        assert_eq!(filtered["Items"][0]["Id"], movie.id.simple().to_string());
    }
    let internal_id_filtered = body_json(
        fixture
            .request(
                &format!("/Items?Ids={}&PersonIds={}", movie.id, created_people[0].id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(internal_id_filtered["TotalRecordCount"], 0);

    items.delete(movie.id).await.expect("movie cleanup");
    items
        .delete(legacy_person.id)
        .await
        .expect("legacy Person cleanup");
    for person in canonical_people {
        items
            .delete(person.id)
            .await
            .expect("canonical Person cleanup");
    }
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
    for (item, name, blurhash) in [
        (&first, "first.jpg", Some("first-blurhash")),
        (&second, "second.jpg", None),
    ] {
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
                    blurhash: blurhash.map(str::to_owned),
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
        assert!(item["ImageBlurHashes"].is_object());
    }
    assert_ne!(
        returned[0]["ImageTags"]["Primary"],
        returned[1]["ImageTags"]["Primary"]
    );
    let first_tag = returned[0]["ImageTags"]["Primary"]
        .as_str()
        .expect("first image tag");
    assert_eq!(
        returned[0]["ImageBlurHashes"]["Primary"][first_tag],
        "first-blurhash"
    );
    assert_eq!(returned[1]["ImageBlurHashes"], serde_json::json!({}));

    items.delete(first.id).await.expect("first cleanup");
    items.delete(second.id).await.expect("second cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn tv_hierarchy_images_match_official_parent_fields_and_image_options() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let images = BaseItemImageRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut series = NewBaseItem::new(Uuid::new_v4(), "Series");
    series.name = Some(format!("Image Series {}", fixture.suffix));
    series.parent_id = Some(root.id);
    series.is_folder = true;
    let series = items.create(series).await.expect("series");

    let mut season = NewBaseItem::new(Uuid::new_v4(), "Season");
    season.name = Some(format!("Image Season {}", fixture.suffix));
    season.parent_id = Some(series.id);
    season.series_id = Some(series.id);
    season.is_folder = true;
    let season = items.create(season).await.expect("season");

    let mut inherited_season = NewBaseItem::new(Uuid::new_v4(), "Season");
    inherited_season.name = Some(format!("Inherited Image Season {}", fixture.suffix));
    inherited_season.parent_id = Some(series.id);
    inherited_season.series_id = Some(series.id);
    inherited_season.is_folder = true;
    let inherited_season = items
        .create(inherited_season)
        .await
        .expect("inherited image season");

    let mut episode = NewBaseItem::new(Uuid::new_v4(), "Episode");
    episode.name = Some(format!("Image Episode {}", fixture.suffix));
    episode.parent_id = Some(season.id);
    episode.season_id = Some(season.id);
    episode.series_id = Some(series.id);
    episode.media_type = Some("Video".to_owned());
    let episode = items.create(episode).await.expect("episode");

    replace_images(
        &images,
        series.id,
        &[
            (BaseItemImageType::Primary, "series-primary.jpg"),
            (BaseItemImageType::Logo, "series-logo.png"),
            (BaseItemImageType::Thumb, "series-thumb.jpg"),
            (BaseItemImageType::Backdrop, "series-backdrop.jpg"),
        ],
    )
    .await;
    replace_images(
        &images,
        season.id,
        &[
            (BaseItemImageType::Primary, "season-primary.jpg"),
            (BaseItemImageType::Logo, "season-logo.png"),
            (BaseItemImageType::Thumb, "season-thumb.jpg"),
            (BaseItemImageType::Backdrop, "season-backdrop.jpg"),
        ],
    )
    .await;

    let episode_detail = body_json(
        fixture
            .request(
                &format!("/Users/{}/Items/{}", fixture.user_id, episode.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(episode_detail["SeriesPrimaryImageTag"].is_string());
    assert_eq!(
        episode_detail["ParentPrimaryImageItemId"],
        season.id.simple().to_string()
    );
    assert_eq!(
        episode_detail["ParentLogoItemId"],
        season.id.simple().to_string()
    );
    assert_eq!(
        episode_detail["ParentThumbItemId"],
        series.id.simple().to_string()
    );
    assert_eq!(
        episode_detail["ParentBackdropItemId"],
        season.id.simple().to_string()
    );

    let season_detail = body_json(
        fixture
            .request(
                &format!("/Users/{}/Items/{}", fixture.user_id, season.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(season_detail["SeriesPrimaryImageTag"].is_string());
    assert!(season_detail.get("ParentPrimaryImageItemId").is_none());
    assert!(season_detail.get("ParentLogoItemId").is_none());
    assert!(season_detail.get("ParentThumbItemId").is_none());
    assert!(season_detail.get("ParentBackdropItemId").is_none());

    let inherited_season_detail = body_json(
        fixture
            .request(
                &format!("/Users/{}/Items/{}", fixture.user_id, inherited_season.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(inherited_season_detail["SeriesPrimaryImageTag"].is_string());
    assert!(
        inherited_season_detail
            .get("ParentPrimaryImageItemId")
            .is_none()
    );
    assert_eq!(
        inherited_season_detail["ParentLogoItemId"],
        series.id.simple().to_string()
    );
    assert_eq!(
        inherited_season_detail["ParentThumbItemId"],
        series.id.simple().to_string()
    );
    assert_eq!(
        inherited_season_detail["ParentBackdropItemId"],
        series.id.simple().to_string()
    );

    let images_disabled = body_json(
        fixture
            .request(
                &format!(
                    "/Items?Ids={},{}&EnableImages=false",
                    episode.id, inherited_season.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    for item in images_disabled["Items"].as_array().expect("disabled items") {
        assert!(item["SeriesPrimaryImageTag"].is_string(), "{item}");
        assert!(item.get("ImageTags").is_none(), "{item}");
        assert!(item.get("ParentPrimaryImageItemId").is_none(), "{item}");
        assert!(item.get("ParentLogoItemId").is_none(), "{item}");
        assert!(item.get("ParentThumbItemId").is_none(), "{item}");
        assert!(item.get("ParentBackdropItemId").is_none(), "{item}");
    }

    let logo_only = body_json(
        fixture
            .request(
                &format!(
                    "/Items?Ids={}&EnableImageTypes=Logo&ImageTypeLimit=1",
                    episode.id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let logo_only = &logo_only["Items"][0];
    assert!(logo_only["SeriesPrimaryImageTag"].is_string());
    assert_eq!(
        logo_only["ParentLogoItemId"],
        season.id.simple().to_string()
    );
    assert!(logo_only.get("ParentPrimaryImageItemId").is_none());
    assert!(logo_only.get("ParentThumbItemId").is_none());
    assert!(logo_only.get("ParentBackdropItemId").is_none());

    items.delete(episode.id).await.expect("episode cleanup");
    items
        .delete(inherited_season.id)
        .await
        .expect("inherited season cleanup");
    items.delete(season.id).await.expect("season cleanup");
    items.delete(series.id).await.expect("series cleanup");
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
    media.data = Some(serde_json::json!({
        "Container": "mkv,webm",
        "IsoType": "dvd",
        "Video3DFormat": "mvc",
        "Timestamp": 1
    }));
    let media = items.create(media).await.expect("media item");
    let alternate_path = format!("/media/page-{}-2160p.mkv", fixture.suffix);
    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
    alternate.name = media.name.clone();
    alternate.sort_name = media.sort_name.clone();
    alternate.parent_id = Some(root.id);
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some(alternate_path.clone());
    alternate.primary_version_id = Some(media.id);
    alternate.data = Some(serde_json::json!({
        "Bitrate": 25_000_000,
        "Container": "mpegts",
        "Size": 987_654_321,
        "VideoType": "BluRay",
        "IsoType": 1,
        "Video3DFormat": "3",
        "Timestamp": "valid"
    }));
    let alternate = items.create(alternate).await.expect("alternate media item");
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
    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            alternate.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("hevc".to_owned()),
                bit_rate: Some(24_000_000),
                path: Some(alternate_path),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("alternate media streams");
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
    let sources = item["MediaSources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["Id"], media.id.simple().to_string());
    assert_eq!(sources[1]["Id"], alternate.id.simple().to_string());
    assert_eq!(item["Container"], "mkv,webm");
    assert_eq!(sources[0]["Container"], "mkv");
    assert_eq!(item["VideoType"], "VideoFile");
    assert_eq!(sources[0]["VideoType"], "VideoFile");
    assert_eq!(sources[1]["VideoType"], "BluRay");
    assert_eq!(item["IsoType"], "Dvd");
    assert_eq!(item["Video3DFormat"], "MVC");
    assert!(item.get("Timestamp").is_none());
    assert_eq!(sources[0]["IsoType"], "Dvd");
    assert_eq!(sources[0]["Video3DFormat"], "MVC");
    assert_eq!(sources[0]["Timestamp"], "Zero");
    assert_eq!(sources[1]["IsoType"], "BluRay");
    assert_eq!(sources[1]["Video3DFormat"], "HalfTopAndBottom");
    assert_eq!(sources[1]["Timestamp"], "Valid");
    assert_eq!(sources[1]["Bitrate"], 25_000_000);
    assert_eq!(sources[1]["Container"], "mpegts");
    assert_eq!(sources[1]["Size"], 987_654_321);
    assert_eq!(sources[1]["ETag"].as_str().unwrap().len(), 32);
    assert_eq!(sources[1]["Name"], "2160p");
    assert_eq!(sources[1]["MediaStreams"][0]["Codec"], "hevc");
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
    assert_eq!(
        item["MediaStreams"][1]["DisplayTitle"],
        "German - Dolby Digital - Default"
    );
    assert_eq!(
        item["MediaSources"][0]["MediaStreams"][3]["DisplayTitle"],
        "English - SRT"
    );

    let page_without_media_sources = body_json(
        fixture
            .request(
                &format!("/Items?recursive=true&searchTerm={}", fixture.suffix),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    let item_without_media_sources = page_without_media_sources["Items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == media.id.simple().to_string())
        .expect("projected item without media sources");
    assert_eq!(item_without_media_sources["Container"], "mkv,webm");
    assert!(item_without_media_sources.get("MediaSources").is_none());

    items
        .delete(alternate.id)
        .await
        .expect("alternate media cleanup");
    items.delete(media.id).await.expect("media cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn item_pages_hide_policy_blocked_alternate_media_sources() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let marker = Uuid::new_v4().simple().to_string();
    let primary_path = format!("/media/{marker}-primary.mkv");
    let hidden_path = format!("/media/{marker}-private.mkv");

    let mut primary = NewBaseItem::new(Uuid::new_v4(), "Movie");
    primary.name = Some(format!("Visible version {marker}"));
    primary.sort_name = primary.name.clone();
    primary.parent_id = Some(root.id);
    primary.media_type = Some("Video".to_owned());
    primary.path = Some(primary_path.clone());
    let primary = items.create(primary).await.expect("primary version");

    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
    alternate.name = primary.name.clone();
    alternate.sort_name = primary.sort_name.clone();
    alternate.parent_id = Some(root.id);
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some(hidden_path.clone());
    alternate.primary_version_id = Some(primary.id);
    let alternate = items.create(alternate).await.expect("private alternate");

    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            alternate.id,
            vec![MediaStream {
                index: 0,
                stream_type: MediaStreamType::Video,
                codec: Some("private-hevc".to_owned()),
                path: Some(hidden_path.clone()),
                ..MediaStream::default()
            }],
        )
        .await
        .expect("private alternate stream");
    MediaAttachmentService::new(fixture.database.clone())
        .save_media_attachments(
            alternate.id,
            vec![MediaAttachment {
                index: 1,
                file_name: Some("private-attachment.jpg".to_owned()),
                ..MediaAttachment::default()
            }],
        )
        .await
        .expect("private alternate attachment");
    ItemValueRepository::new(fixture.database.clone())
        .link(
            alternate.id,
            item_value::ItemValueType::Tags,
            "PrivateVersion",
        )
        .await
        .expect("private alternate tag");

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.blocked_tags = vec!["privateversion".to_owned()];
    policy.enable_video_playback_transcoding = false;
    policy.enable_playback_remuxing = false;
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted user policy");

    let route = format!(
        "/Items?ids={}&fields=MediaSources,MediaStreams,MediaSourceCount",
        primary.id
    );
    let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
    assert_eq!(body["TotalRecordCount"], 1);
    let dto = &body["Items"][0];
    let sources = dto["MediaSources"].as_array().expect("media sources");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["Id"], primary.id.simple().to_string());
    assert_eq!(sources[0]["SupportsDirectPlay"], true);
    assert_eq!(sources[0]["SupportsDirectStream"], false);
    assert_eq!(sources[0]["SupportsTranscoding"], false);
    assert!(dto.get("MediaSourceCount").is_none());
    let serialized = serde_json::to_string(dto).unwrap();
    assert!(!serialized.contains(&alternate.id.simple().to_string()));
    assert!(!serialized.contains(&hidden_path));
    assert!(!serialized.contains("private-hevc"));
    assert!(!serialized.contains("private-attachment.jpg"));

    items.delete(alternate.id).await.expect("alternate cleanup");
    items.delete(primary.id).await.expect("primary cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn episode_media_source_count_is_projected_without_loading_sources() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let marker = Uuid::new_v4().simple().to_string();

    let mut primary = NewBaseItem::new(Uuid::new_v4(), "Episode");
    primary.name = Some(format!("{marker} Primary"));
    primary.sort_name = primary.name.clone();
    primary.parent_id = Some(root.id);
    primary.media_type = Some("Video".to_owned());
    primary.path = Some(format!("/media/{marker} - 1080p.mkv"));
    let primary = items.create(primary).await.expect("primary episode");

    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Episode");
    alternate.name = primary.name.clone();
    alternate.sort_name = primary.sort_name.clone();
    alternate.parent_id = Some(root.id);
    alternate.media_type = Some("Video".to_owned());
    alternate.path = Some(format!("/media/{marker} - 720p.mkv"));
    alternate.primary_version_id = Some(primary.id);
    let alternate = items.create(alternate).await.expect("alternate episode");

    let mut hidden_alternate = NewBaseItem::new(Uuid::new_v4(), "Episode");
    hidden_alternate.name = primary.name.clone();
    hidden_alternate.sort_name = primary.sort_name.clone();
    hidden_alternate.parent_id = Some(root.id);
    hidden_alternate.media_type = Some("Video".to_owned());
    hidden_alternate.path = Some(format!("/media/{marker} - private.mkv"));
    hidden_alternate.primary_version_id = Some(primary.id);
    let hidden_alternate = items
        .create(hidden_alternate)
        .await
        .expect("hidden alternate episode");
    ItemValueRepository::new(fixture.database.clone())
        .link(
            hidden_alternate.id,
            item_value::ItemValueType::Tags,
            "PrivateVersion",
        )
        .await
        .expect("hidden alternate tag");
    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.blocked_tags = vec!["privateversion".to_owned()];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted user policy");

    let mut singleton = NewBaseItem::new(Uuid::new_v4(), "Episode");
    singleton.name = Some(format!("{marker} Singleton"));
    singleton.sort_name = singleton.name.clone();
    singleton.parent_id = Some(root.id);
    singleton.media_type = Some("Video".to_owned());
    singleton.path = Some(format!("/media/{marker} - singleton.mkv"));
    let singleton = items.create(singleton).await.expect("singleton episode");

    for (search_term, include_item_types) in [
        ("searchTerm", "includeItemTypes"),
        ("SearchTerm", "IncludeItemTypes"),
        ("searchterm", "includeitemtypes"),
    ] {
        let route = format!(
            "/Items?recursive=true&{search_term}={marker}&{include_item_types}=Episode&fields=mediasourcecount"
        );
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 2, "{route}");
        let returned = body["Items"].as_array().expect("items");
        let grouped = returned
            .iter()
            .find(|item| item["Id"] == primary.id.simple().to_string())
            .expect("grouped primary episode");
        assert_eq!(grouped["MediaSourceCount"], 2, "{route}");
        assert!(grouped.get("MediaSources").is_none(), "{route}");
        let single = returned
            .iter()
            .find(|item| item["Id"] == singleton.id.simple().to_string())
            .expect("singleton episode");
        assert!(single.get("MediaSourceCount").is_none(), "{route}");
    }

    items
        .delete(hidden_alternate.id)
        .await
        .expect("hidden alternate cleanup");
    items.delete(alternate.id).await.expect("alternate cleanup");
    items.delete(primary.id).await.expect("primary cleanup");
    items.delete(singleton.id).await.expect("singleton cleanup");
    fixture.cleanup().await;
}

#[tokio::test]
async fn recursive_item_count_is_opt_in_for_pages_and_defaulted_for_details() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut folder = NewBaseItem::new(Uuid::new_v4(), "Folder");
    folder.name = Some(format!("Recursive count {}", fixture.suffix));
    folder.sort_name = folder.name.clone();
    folder.parent_id = Some(root.id);
    folder.is_folder = true;
    let folder = items.create(folder).await.expect("counted folder");
    let primary = create_item(
        &items,
        "Movie",
        &format!("Primary recursive {}", fixture.suffix),
        folder.id,
    )
    .await;
    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
    alternate.parent_id = Some(folder.id);
    alternate.primary_version_id = Some(primary.id);
    let _alternate = items.create(alternate).await.expect("alternate leaf");
    let mut virtual_item = NewBaseItem::new(Uuid::new_v4(), "Episode");
    virtual_item.parent_id = Some(folder.id);
    virtual_item.is_virtual_item = true;
    let _virtual_item = items.create(virtual_item).await.expect("virtual leaf");
    let mut nested = NewBaseItem::new(Uuid::new_v4(), "Folder");
    nested.parent_id = Some(folder.id);
    nested.is_folder = true;
    let nested = items.create(nested).await.expect("nested folder");
    let _nested_leaf = create_item(
        &items,
        "Episode",
        &format!("Nested recursive {}", fixture.suffix),
        nested.id,
    )
    .await;

    let without_field = body_json(
        fixture
            .request(
                &format!("/Items?ids={}", folder.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(
        without_field["Items"][0]
            .get("RecursiveItemCount")
            .is_none()
    );

    for field in [
        "RecursiveItemCount",
        "recursiveItemCount",
        "recursiveitemcount",
    ] {
        let with_field = body_json(
            fixture
                .request(
                    &format!("/Items?ids={}&fields={field}", folder.id),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_eq!(with_field["Items"][0]["RecursiveItemCount"], 2);
    }

    let detail = body_json(
        fixture
            .request(
                &format!("/Users/{}/Items/{}", fixture.user_id, folder.id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(detail["RecursiveItemCount"], 2);

    items
        .delete(folder.id)
        .await
        .expect("folder subtree cleanup");
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
    for (search_term, start_index, enable_total_record_count) in [
        ("searchTerm", "startIndex", "enableTotalRecordCount"),
        ("SearchTerm", "StartIndex", "EnableTotalRecordCount"),
        ("searchterm", "startindex", "enabletotalrecordcount"),
    ] {
        let route = format!(
            "/Items?recursive=true&{search_term}={}&{start_index}=1&limit=2&{enable_total_record_count}=true",
            fixture.suffix.to_uppercase()
        );
        let response = fixture.request(&route, Some(&fixture.user_token)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        let body = body_json(response).await;
        assert_eq!(body["TotalRecordCount"], 4, "{route}");
        assert_eq!(body["StartIndex"], 1, "{route}");
        assert_eq!(body["Items"].as_array().unwrap().len(), 2, "{route}");
        for item in body["Items"].as_array().unwrap() {
            assert!(!item["ServerId"].as_str().unwrap().is_empty());
            assert!(item["Name"].as_str().unwrap().contains(&fixture.suffix));
            assert!(item.get("item_type").is_none());
        }
    }

    for (search_term, include_item_types) in [
        ("searchTerm", "includeItemTypes"),
        ("SearchTerm", "IncludeItemTypes"),
        ("searchterm", "includeitemtypes"),
    ] {
        let movie_route = format!(
            "/Items?recursive=true&{search_term}={}&{include_item_types}=Movie",
            fixture.suffix.to_uppercase()
        );
        let movies = body_json(
            fixture
                .request(&movie_route, Some(&fixture.user_token))
                .await,
        )
        .await;
        assert_eq!(movies["TotalRecordCount"], 2, "{movie_route}");
        assert!(
            movies["Items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["Type"] == "Movie"),
            "{movie_route}"
        );
    }

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

    // ItemsController declares signed nullable Int32 pagination. PostgreSQL uses an
    // unsigned query representation internally, but its externally visible behavior
    // must retain the official negative-offset echo and SQLite's negative-limit
    // unlimited behavior expected by the Android and Swift SDKs.
    for start_index in ["startIndex", "StartIndex", "startindex"] {
        for prefix in [
            "/Items".to_owned(),
            format!("/Users/{}/Items", fixture.user_id),
            format!("/users/{}/items", fixture.user_id),
        ] {
            let route = format!(
                "{prefix}?ids={},{}&{start_index}=-1&limit=1",
                fixture.item_ids[0], fixture.item_ids[2]
            );
            let page = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
            assert_eq!(page["StartIndex"], -1, "{route}");
            assert_eq!(page["TotalRecordCount"], 2, "{route}");
            assert_eq!(page["Items"].as_array().unwrap().len(), 1, "{route}");
        }
    }
    for limit_name in ["limit", "Limit"] {
        for (limit, expected_len) in [("0", 0), ("-1", 2)] {
            let route = format!(
                "/Items?ids={},{}&{limit_name}={limit}",
                fixture.item_ids[0], fixture.item_ids[2]
            );
            let page = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
            assert_eq!(page["StartIndex"], 0, "{route}");
            assert_eq!(
                page["Items"].as_array().unwrap().len(),
                expected_len,
                "{route}"
            );
        }
    }
    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        let route = format!("/Items?recursive=true&{query}");
        assert_eq!(
            fixture
                .request(&route, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{route}"
        );
    }
    let resume_route = format!(
        "/Users/{}/Items/Resume?startIndex=-1&limit=-1",
        fixture.user_id
    );
    let resume = body_json(
        fixture
            .request(&resume_route, Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(resume["StartIndex"], -1, "{resume_route}");
    assert_eq!(
        resume["Items"].as_array().unwrap().len(),
        2,
        "{resume_route}"
    );
    let lowercase_resume = body_json(
        fixture
            .request(
                "/useritems/resume?startindex=-1&limit=-1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_resume, resume);

    for (search_term, sort_by, sort_order) in [
        ("searchTerm", "sortBy", "sortOrder"),
        ("SearchTerm", "SortBy", "SortOrder"),
        ("searchterm", "sortby", "sortorder"),
    ] {
        let descending_route = format!(
            "/Items?recursive=true&{search_term}={}&{sort_by}=SortName&{sort_order}=Descending",
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
            ],
            "{descending_route}"
        );
    }
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
async fn resume_exclude_active_sessions_is_version_aware_and_user_scoped() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let user_data = UserDataRepository::new(fixture.database.clone());
    let primary_id = fixture.item_ids[0];
    let retained_id = fixture.item_ids[1];

    let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
    alternate.name = Some(format!("A alternate {}", fixture.suffix));
    alternate.sort_name = alternate.name.clone();
    alternate.parent_id = items
        .get(primary_id)
        .await
        .expect("primary lookup")
        .expect("primary item")
        .parent_id;
    alternate.primary_version_id = Some(primary_id);
    let alternate = items.create(alternate).await.expect("alternate creation");
    upsert_resume(
        &user_data,
        fixture.user_id,
        alternate.id,
        "alternate-version",
        500,
        Utc::now() + Duration::hours(1),
    )
    .await;

    let devices = DeviceRepository::new(fixture.database.clone());
    let user_session = devices
        .find_by_token(&fixture.user_token)
        .await
        .expect("user session lookup")
        .expect("user session");
    devices
        .update_playback_state(
            user_session.id,
            serde_json::json!({}),
            Some(serde_json::json!({
                "Id": primary_id.simple().to_string(),
                "Type": "Movie"
            })),
            None,
            None,
            false,
        )
        .await
        .expect("user playback start");

    // A different user's active session must not affect the requested user's
    // Resume page, even when both users are playing resumable items.
    let admin_session = devices
        .find_by_token(&fixture.admin_token)
        .await
        .expect("administrator session lookup")
        .expect("administrator session");
    devices
        .update_playback_state(
            admin_session.id,
            serde_json::json!({}),
            Some(serde_json::json!({
                "Id": retained_id.simple().to_string(),
                "Type": "Episode"
            })),
            None,
            None,
            false,
        )
        .await
        .expect("administrator playback start");

    let route = format!(
        "/UserItems/Resume?userId={}&searchTerm={}",
        fixture.user_id,
        fixture.suffix.to_uppercase()
    );
    for suffix in ["", "&excludeActiveSessions=false"] {
        let body = body_json(
            fixture
                .request(&format!("{route}{suffix}"), Some(&fixture.admin_token))
                .await,
        )
        .await;
        assert_eq!(body["TotalRecordCount"], 2, "{suffix}");
        assert_eq!(body["Items"][0]["Id"], alternate.id.simple().to_string());
        assert_eq!(body["Items"][1]["Id"], retained_id.simple().to_string());
    }

    for parameter in [
        "excludeActiveSessions",
        "ExcludeActiveSessions",
        "excludeactivesessions",
        "exclude_active_sessions",
    ] {
        let excluded = body_json(
            fixture
                .request(
                    &format!("{route}&{parameter}=true&limit=1"),
                    Some(&fixture.admin_token),
                )
                .await,
        )
        .await;
        assert_eq!(excluded["TotalRecordCount"], 1, "{parameter}");
        assert_eq!(excluded["StartIndex"], 0, "{parameter}");
        assert_eq!(excluded["Items"].as_array().unwrap().len(), 1);
        assert_eq!(
            excluded["Items"][0]["Id"],
            retained_id.simple().to_string(),
            "{parameter}"
        );
    }

    devices
        .clear_playback_state(user_session.id, None, None)
        .await
        .expect("user playback stop");
    let after_stop = body_json(
        fixture
            .request(
                &format!("{route}&excludeActiveSessions=true"),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(after_stop["TotalRecordCount"], 2);
    assert_eq!(
        after_stop["Items"][0]["Id"],
        alternate.id.simple().to_string()
    );

    items.delete(alternate.id).await.expect("alternate cleanup");
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
    for key in ["isHd", "IsHD", "ishd"] {
        let route = format!("/Items?ids={ids}&{key}=true");
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 1, "{route}");
        assert_eq!(
            body["Items"][0]["Id"],
            hd.id.simple().to_string(),
            "{route}"
        );
    }

    for key in ["is4K", "Is4K", "is4k"] {
        let route = format!("/Items?ids={ids}&{key}=true");
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 1, "{route}");
        assert_eq!(
            body["Items"][0]["Id"],
            four_k.id.simple().to_string(),
            "{route}"
        );
    }

    for key in ["audioLanguages", "AudioLanguages", "audiolanguages"] {
        let route = format!("/Items?ids={ids}&{key}=eng");
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 1, "{route}");
        assert_eq!(
            body["Items"][0]["Id"],
            audio.id.simple().to_string(),
            "{route}"
        );
    }

    for key in ["hasImdbId", "HasImdbId", "hasimdbid"] {
        let route = format!("/Items?ids={ids}&{key}=true");
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 1, "{route}");
        assert_eq!(
            body["Items"][0]["Id"],
            hd.id.simple().to_string(),
            "{route}"
        );
    }

    for key in ["excludeItemIds", "ExcludeItemIds", "excludeitemids"] {
        let route = format!("/Items?ids={ids}&{key}={}", hd.id);
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body["TotalRecordCount"], 3, "{route}");
        assert!(
            body["Items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["Id"] != hd.id.simple().to_string()),
            "{route}"
        );
    }

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
    for route in ["/Items", "/Items/Latest", "/Items/Suggestions"] {
        for user_id in ["userId", "UserId", "userid"] {
            let route = format!("{route}?{user_id}={}", fixture.admin_id);
            assert_eq!(
                fixture
                    .request(&route, Some(&fixture.user_token))
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "{route}"
            );
        }
    }
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

#[tokio::test]
async fn latest_query_names_are_case_insensitive() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let user_data = UserDataRepository::new(fixture.database.clone());
    let mut played = NewUserData::new(fixture.item_ids[0], fixture.user_id, "main");
    played.played = true;
    user_data.upsert(played).await.expect("played user data");

    for (include_item_types, is_played) in [
        ("includeItemTypes", "isPlayed"),
        ("IncludeItemTypes", "IsPlayed"),
        ("includeitemtypes", "isplayed"),
    ] {
        let route = format!("/Items/Latest?{include_item_types}=Movie&{is_played}=true&limit=10");
        let body = body_json(fixture.request(&route, Some(&fixture.user_token)).await).await;
        assert_eq!(body.as_array().expect("latest items").len(), 1, "{route}");
        assert_eq!(
            body[0]["Id"],
            fixture.item_ids[0].simple().to_string(),
            "{route}"
        );
    }

    for group_items in ["groupItems", "GroupItems", "groupitems"] {
        let route = format!("/Items/Latest?{group_items}=not-bool");
        assert_eq!(
            fixture
                .request(&route, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{route}"
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
    api_key_token: String,
    item_ids: Vec<Uuid>,
    storage_root: std::path::PathBuf,
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
            "Suggested %",
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
        let storage_root = std::env::temp_dir().join(format!("items-routes-{suffix}"));
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
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("items-key-{suffix}"))
            .await
            .expect("items API key")
            .access_token;

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
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("image-cache"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
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
            api_key_token,
            item_ids: vec![first.id, second.id, third.id, nested.id],
            storage_root,
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
        ApiKeyRepository::new(self.database.clone())
            .revoke(&self.api_key_token)
            .await
            .expect("items API key cleanup");
        let items = BaseItemRepository::new(self.database.clone());
        for item_id in self.item_ids.into_iter().take(3) {
            items.delete(item_id).await.expect("item cleanup");
        }
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        let _ = tokio::fs::remove_dir_all(self.storage_root).await;
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

async fn replace_images(
    repository: &BaseItemImageRepository,
    item_id: Uuid,
    images: &[(BaseItemImageType, &str)],
) {
    repository
        .replace(
            item_id,
            &images
                .iter()
                .map(|(image_type, path)| NewBaseItemImage {
                    image_type: *image_type,
                    image_index: 0,
                    path: (*path).to_owned(),
                    date_modified: Utc::now(),
                    width: None,
                    height: None,
                    blurhash: None,
                })
                .collect::<Vec<_>>(),
        )
        .await
        .expect("images must persist");
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

fn assert_audio_stream_fields(streams: &Value, route: &str) {
    let streams = streams.as_array().expect("media streams");
    assert_eq!(streams.len(), 3, "{route}: {streams:?}");
    for (stream_type, language) in [("Audio", "deu"), ("Subtitle", "eng"), ("Lyric", "jpn")] {
        let stream = streams
            .iter()
            .find(|stream| stream["Type"] == stream_type)
            .expect("requested stream type");
        assert_eq!(stream["Language"], language, "{route}: {stream}");
    }
}
