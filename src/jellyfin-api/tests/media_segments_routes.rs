use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{UserService, VirtualFolderService, media_segment_provider_id};
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, MediaSegmentRepository, NewBaseItem, NewDevice,
    NewMediaSegment,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Media Segment Tests\", Device=\"Test\", DeviceId=\"media-segments\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_media_segments_routes_";

#[tokio::test]
async fn media_segments_route_matches_official_contract_and_returns_persisted_segments() {
    let administrator = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_media_segments(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_media_segments(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
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

    let empty_id = fixture
        .get(
            &format!("/MediaSegments/{}", Uuid::nil()),
            &fixture.user_token,
        )
        .await;
    assert_eq!(empty_id.status(), StatusCode::BAD_REQUEST);

    let no_registered_providers = fixture
        .get_from(
            &fixture.unregistered_app,
            &format!("/MediaSegments/{}", fixture.item_id),
            &fixture.user_token,
        )
        .await;
    assert_eq!(no_registered_providers.status(), StatusCode::OK);
    assert_eq!(
        body_json(no_registered_providers).await["TotalRecordCount"],
        0,
        "persisted segments from unregistered providers must not leak"
    );

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
    assert!(
        body["Items"]
            .as_array()
            .expect("segments")
            .iter()
            .all(|segment| segment["Type"] != "Recap" && segment["Type"] != "Outro"),
        "disabled and unregistered providers must be filtered"
    );

    let available_options = body_json(
        fixture
            .get(
                "/Libraries/AvailableOptions?libraryContentType=movies",
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(
        available_options["MediaSegmentProviders"],
        serde_json::json!([
            { "Name": "Enabled Provider", "DefaultEnabled": true },
            { "Name": "Disabled Provider", "DefaultEnabled": true }
        ]),
        "available options must expose distinct registered provider names"
    );

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

    let commercial_only = body_json(
        fixture
            .get(
                &format!(
                    "/MediaSegments/{}?IncludeSegmentTypes=Commercial",
                    fixture.item_id
                ),
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(commercial_only["TotalRecordCount"], 1);
    assert_eq!(commercial_only["Items"][0]["Type"], "Commercial");

    let lowercase_intro_only = body_json(
        fixture
            .get(
                &format!(
                    "/mediasegments/{}?includesegmenttypes=intro",
                    fixture.item_id
                ),
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_intro_only["TotalRecordCount"], 1);
    assert_eq!(lowercase_intro_only["Items"][0]["Type"], "Intro");

    let numeric_intro_only = body_json(
        fixture
            .get(
                &format!("/MediaSegments/{}?includeSegmentTypes=5", fixture.item_id),
                &fixture.user_token,
            )
            .await,
    )
    .await;
    assert_eq!(numeric_intro_only["TotalRecordCount"], 1);
    assert_eq!(numeric_intro_only["Items"][0]["Type"], "Intro");

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
    unregistered_app: axum::Router,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 8,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let user = users
            .create_initial_administrator(&format!("media-segments-user-{suffix}"))
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

        let folders = VirtualFolderService::new(database.clone());
        folders
            .create(
                &format!("Media Segments {suffix}"),
                Some("movies".to_owned()),
                serde_json::json!({
                    "DisabledMediaSegmentProviders": ["DISABLED provider"]
                }),
                Vec::new(),
                false,
            )
            .await
            .expect("media-segment virtual folder");
        let folder = folders
            .list()
            .await
            .expect("virtual folder list")
            .into_iter()
            .next()
            .expect("media-segment virtual folder");
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let mut collection = NewBaseItem::new(folder.id, "CollectionFolder");
        collection.parent_id = Some(root.id);
        collection.name = Some(folder.name);
        collection.is_folder = true;
        items
            .create(collection)
            .await
            .expect("collection folder item");

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Video");
        item.parent_id = Some(folder.id);
        item.name = Some("Segmented Video".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Segmented Video {suffix}.mkv"));
        let item = items.create(item).await.expect("video item creation");
        let segments = MediaSegmentRepository::new(database.clone());
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 5,
                start_ticks: 0,
                end_ticks: 10_000_000,
                segment_provider_id: media_segment_provider_id("Enabled Provider"),
            })
            .await
            .expect("intro segment persistence");
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 1,
                start_ticks: 10_000_000,
                end_ticks: 20_000_000,
                segment_provider_id: media_segment_provider_id("enabled provider"),
            })
            .await
            .expect("commercial segment persistence");
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 3,
                start_ticks: 20_000_000,
                end_ticks: 30_000_000,
                segment_provider_id: media_segment_provider_id("Disabled Provider"),
            })
            .await
            .expect("disabled-provider segment persistence");
        segments
            .create(NewMediaSegment {
                item_id: item.id,
                segment_type: 4,
                start_ticks: 30_000_000,
                end_ticks: 40_000_000,
                segment_provider_id: media_segment_provider_id("Stale Provider"),
            })
            .await
            .expect("unregistered-provider segment persistence");

        let unregistered_app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Media Segment Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Media Segment Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_media_segment_provider_names(vec![
                "Enabled Provider".to_owned(),
                "enabled provider".to_owned(),
                "Disabled Provider".to_owned(),
            ]),
        );

        Self {
            database,
            app,
            unregistered_app,
            user_id: user.id,
            user_token,
            item_id: item.id,
        }
    }

    async fn get(&self, uri: &str, token: &str) -> axum::response::Response {
        self.get_from(&self.app, uri, token).await
    }

    async fn get_from(
        &self,
        app: &axum::Router,
        uri: &str,
        token: &str,
    ) -> axum::response::Response {
        app.clone()
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
        self.database
            .close()
            .await
            .expect("temporary database pool cleanup");
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(
        database_name.starts_with(DATABASE_PREFIX),
        "refusing to manage unexpected database name: {database_name}"
    );
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit()),
        "temporary database suffix must be UUID hex: {database_name}"
    );
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
