use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, USER_ROOT_FOLDER_ID,
    entities::{base_item, user},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Library Tests\", Device=\"Test\", DeviceId=\"user-library\", Version=\"1.0\"";

#[tokio::test]
async fn official_nonexistent_user_routes_return_not_found() {
    let fixture = UserLibraryFixture::new().await;
    let missing_user_id = Uuid::new_v4();
    let routes = [
        format!("/Users/{missing_user_id}/Items/Root"),
        format!("/Users/{missing_user_id}/Items/{}", fixture.root_id),
        format!("/Users/{missing_user_id}/Items/{}/Intros", fixture.root_id),
        format!(
            "/Users/{missing_user_id}/Items/{}/LocalTrailers",
            fixture.root_id
        ),
        format!(
            "/Users/{missing_user_id}/Items/{}/SpecialFeatures",
            fixture.root_id
        ),
        format!("/Users/{missing_user_id}/Items/{}/Lyrics", fixture.root_id),
    ];
    for route in routes {
        let response = request(&fixture.app, &route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn official_nonexistent_item_routes_return_not_found() {
    let fixture = UserLibraryFixture::new().await;
    let item_id = Uuid::new_v4();
    for suffix in [
        "",
        "/Intros",
        "/LocalTrailers",
        "/SpecialFeatures",
        "/Lyrics",
    ] {
        let route = format!(
            "/Users/{}/Items/{item_id}{suffix}",
            fixture.administrator_id
        );
        let response = request(&fixture.app, &route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn valid_legacy_routes_cover_the_flaky_official_success_paths() {
    let fixture = UserLibraryFixture::new().await;

    let root_route = format!("/Users/{}/Items/Root", fixture.user_id);
    let root = get_json(&fixture.app, &root_route, &fixture.user_token).await;
    assert_base_item(&root, fixture.root_id, "UserRootFolder", "Root");

    let item_route = format!("/Users/{}/Items/{}", fixture.user_id, fixture.item_id);
    let item = get_json(&fixture.app, &item_route, &fixture.user_token).await;
    assert_base_item(&item, fixture.item_id, "Audio", "Test Song");
    assert_eq!(item["HasLyrics"], true);
    assert!(item.get("item_type").is_none());

    let intros = get_json(
        &fixture.app,
        &format!("{item_route}/Intros"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(intros["TotalRecordCount"], 1);
    assert_eq!(intros["StartIndex"], 0);
    assert_eq!(
        intros["Items"][0]["Id"],
        fixture.intro_id.simple().to_string()
    );
    assert!(intros.get("total_record_count").is_none());

    let trailers = get_json(
        &fixture.app,
        &format!("{item_route}/LocalTrailers"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(trailers.as_array().unwrap().len(), 1);
    assert_eq!(trailers[0]["Id"], fixture.trailer_id.simple().to_string());
    assert_eq!(trailers[0]["ExtraType"], "Trailer");

    let features = get_json(
        &fixture.app,
        &format!("{item_route}/SpecialFeatures"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(features.as_array().unwrap().len(), 1);
    assert_eq!(features[0]["Id"], fixture.feature_id.simple().to_string());
    assert_eq!(features[0]["ExtraType"], "Featurette");

    let lyrics = get_json(
        &fixture.app,
        &format!("{item_route}/Lyrics"),
        &fixture.user_token,
    )
    .await;
    assert_eq!(lyrics["Metadata"]["Artist"], "Test Artist");
    assert_eq!(lyrics["Lyrics"][0]["Text"], "First line");

    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_self_admin_and_current_routes_are_enforced() {
    let fixture = UserLibraryFixture::new().await;
    let legacy_routes = [
        format!("/Users/{}/Items/Root", fixture.administrator_id),
        format!(
            "/Users/{}/Items/{}",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/Intros",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/LocalTrailers",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/SpecialFeatures",
            fixture.administrator_id, fixture.item_id
        ),
        format!(
            "/Users/{}/Items/{}/Lyrics",
            fixture.administrator_id, fixture.item_id
        ),
    ];
    for route in &legacy_routes {
        let response = fixture
            .app
            .clone()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{route}");

        let response = request(&fixture.app, route, &fixture.user_token).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{route}");

        let response = request(&fixture.app, route, &fixture.administrator_token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
    }

    for route in [
        "/Items/Root".to_owned(),
        format!("/Items/{}", fixture.item_id),
        format!("/Items/{}/Intros", fixture.item_id),
        format!("/Items/{}/LocalTrailers", fixture.item_id),
        format!("/Items/{}/SpecialFeatures", fixture.item_id),
        format!("/Audio/{}/Lyrics", fixture.item_id),
    ] {
        let response = request(&fixture.app, &route, &fixture.user_token).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
    }

    let admin_for_user = format!("/Items/Root?userId={}", fixture.user_id);
    let response = request(&fixture.app, &admin_for_user, &fixture.administrator_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let regular_for_admin = format!("/Items/Root?userId={}", fixture.administrator_id);
    let response = request(&fixture.app, &regular_for_admin, &fixture.user_token).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    fixture.cleanup().await;
}

#[tokio::test]
async fn concurrent_initialization_converges_on_one_postgres_root() {
    let database = test_database().await;
    let first = BaseItemRepository::new(database.clone());
    let second = BaseItemRepository::new(database.clone());
    let third = BaseItemRepository::new(database.clone());
    let (first, second, third) = tokio::join!(
        first.ensure_user_root(),
        second.ensure_user_root(),
        third.ensure_user_root()
    );
    assert_eq!(first.unwrap().id, USER_ROOT_FOLDER_ID);
    assert_eq!(second.unwrap().id, USER_ROOT_FOLDER_ID);
    assert_eq!(third.unwrap().id, USER_ROOT_FOLDER_ID);
    let root_count = base_item::Entity::find()
        .filter(base_item::Column::ItemType.eq("UserRootFolder"))
        .count(&database)
        .await
        .expect("root count");
    assert_eq!(root_count, 1);
}

struct UserLibraryFixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    root_id: Uuid,
    item_id: Uuid,
    intro_id: Uuid,
    trailer_id: Uuid,
    feature_id: Uuid,
}

impl UserLibraryFixture {
    async fn new() -> Self {
        let database = test_database().await;
        let users = UserService::new(database.clone());
        let suffix = Uuid::new_v4().simple().to_string();
        let administrator = users
            .create_initial_administrator(&format!("library-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("library-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token = devices
            .create_session(NewDevice::new(
                administrator.id,
                "User Library Tests",
                "1.0",
                "Test",
                format!("library-admin-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "User Library Tests",
                "1.0",
                "Test",
                format!("library-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let mut media = item("Audio", "Test Song", Some(root.id), false);
        media.media_type = Some("Audio".to_owned());
        media.data = Some(json!({
            "Lyrics": {
                "Metadata": { "Artist": "Test Artist" },
                "Lyrics": [{ "Text": "First line", "Start": 0, "Cues": null }]
            }
        }));
        let media = items.create(media).await.expect("media item");

        let mut intro = item("Video", "Intro", Some(media.id), false);
        intro.data = Some(json!({ "IsIntro": true }));
        let intro = items.create(intro).await.expect("intro item");

        let nested = items
            .create(item("Folder", "Extras", Some(media.id), true))
            .await
            .expect("nested extras folder");
        let mut trailer = item("Video", "Trailer", Some(nested.id), false);
        trailer.data = Some(json!({ "ExtraType": "Trailer" }));
        let trailer = items.create(trailer).await.expect("trailer item");
        let mut feature = item("Video", "Feature", Some(media.id), false);
        feature.data = Some(json!({ "ExtraType": "Featurette" }));
        let feature = items.create(feature).await.expect("feature item");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "User Library Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            user_id: user.id,
            user_token,
            root_id: root.id,
            item_id: media.id,
            intro_id: intro.id,
            trailer_id: trailer.id,
            feature_id: feature.id,
        }
    }

    async fn cleanup(self) {
        BaseItemRepository::new(self.database.clone())
            .delete(self.item_id)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

fn item(item_type: &str, name: &str, parent_id: Option<Uuid>, is_folder: bool) -> NewBaseItem {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    item
}

fn assert_base_item(body: &Value, id: Uuid, item_type: &str, name: &str) {
    assert_eq!(body["Id"], id.simple().to_string());
    assert_eq!(body["Type"], item_type);
    assert_eq!(body["Name"], name);
    assert_eq!(body["ServerId"].as_str().unwrap().len(), 32);
    assert!(body["DateCreated"].is_string());
    assert!(body["Etag"].is_string());
}

async fn request(app: &axum::Router, uri: &str, token: &str) -> axum::response::Response {
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

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = request(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn test_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}
