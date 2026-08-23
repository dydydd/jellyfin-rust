use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, MediaSegmentRepository, NewBaseItem, NewDevice,
    NewMediaSegment,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Media Segment Tests\", Device=\"Test\", DeviceId=\"media-segments\", Version=\"1.0\"";

#[tokio::test]
async fn media_segments_route_matches_official_contract_and_returns_persisted_segments() {
    let fixture = Fixture::new().await;
    let route = format!(
        "/MediaSegments/{}?includeSegmentTypes=Intro,Commercial",
        fixture.item_id
    );

    let unauthenticated = fixture
        .app
        .clone()
        .oneshot(Request::get(&route).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let missing = fixture
        .get(
            &format!("/MediaSegments/{}", Uuid::new_v4()),
            &fixture.user_token,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let response = fixture.get(&route, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["StartIndex"], 0);
    assert!(body.get("items").is_none());
    assert_eq!(body["Items"][0]["Type"], "Intro");
    assert_eq!(body["Items"][1]["Type"], "Commercial");

    let response = fixture
        .get(
            &format!("/MediaSegments/{}", fixture.item_id),
            &fixture.user_token,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["Items"][0]["StartTicks"], 0);
    assert_eq!(body["Items"][0]["EndTicks"], 10_000_000);
    assert_eq!(body["Items"][1]["StartTicks"], 10_000_000);

    let intro_only = body_json(
        fixture
            .get(
                &format!(
                    "/MediaSegments/{}?includeSegmentTypes=Intro",
                    fixture.item_id
                ),
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(intro_only["TotalRecordCount"], 1);
    assert_eq!(intro_only["Items"][0]["Type"], "Intro");

    let unknown_only = body_json(
        fixture
            .get(
                &format!(
                    "/MediaSegments/{}?includeSegmentTypes=Unknown",
                    fixture.item_id
                ),
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(unknown_only["TotalRecordCount"], 0);

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let user = users
            .create(&format!("media-segments-user-{suffix}"))
            .await
            .expect("user creation");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Media Segment Tests",
                "1.0",
                "Test",
                format!("media-segments-{suffix}"),
            ))
            .await
            .expect("session creation")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Video");
        item.name = Some("Segmented Video".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Segmented Video {suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("video item creation");
        let segments = MediaSegmentRepository::new(database.clone());
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 5,
                start_ticks: 0,
                end_ticks: 10_000_000,
                segment_provider_id: "test-provider".to_owned(),
            })
            .await
            .expect("intro segment persistence");
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 1,
                start_ticks: 10_000_000,
                end_ticks: 20_000_000,
                segment_provider_id: "test-provider".to_owned(),
            })
            .await
            .expect("commercial segment persistence");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Media Segment Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            user_id: user.id,
            user_token,
            item_id: item.id,
        }
    }

    async fn get(&self, uri: &str, token: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.eq(self.item_id))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
