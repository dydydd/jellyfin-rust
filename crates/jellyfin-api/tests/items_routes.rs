use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::MediaStreamService;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewUserData, UserDataRepository,
    entities::user,
};
use jellyfin_model::{MediaStream, MediaStreamType};
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
async fn media_stream_fields_are_projected_for_item_pages() {
    let _guard = ITEMS_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");

    let mut media = NewBaseItem::new(Uuid::new_v4(), "Audio");
    media.name = Some(format!("Page Media {}", fixture.suffix));
    media.sort_name = media.name.clone();
    media.parent_id = Some(root.id);
    media.media_type = Some("Audio".to_owned());
    media.path = Some(format!("/media/page-{}.mkv", fixture.suffix));
    let media = items.create(media).await.expect("media item");
    MediaStreamService::new(fixture.database.clone())
        .save_media_streams(
            media.id,
            &[MediaStream {
                index: 0,
                stream_type: MediaStreamType::Audio,
                codec: Some("ac3".to_owned()),
                language: Some("ger".to_owned()),
                path: Some(format!("/media/page-{}.mkv", fixture.suffix)),
                is_default: true,
                ..MediaStream::default()
            }],
        )
        .await
        .expect("media streams");

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
    assert_eq!(item["MediaStreams"].as_array().unwrap().len(), 1);
    assert_eq!(item["MediaStreams"][0]["Language"], "deu");

    items.delete(media.id).await.expect("media cleanup");
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

    let selected = body_json(
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
