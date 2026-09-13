use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository,
    NewBaseItem, NewDevice, NewUserData, UserDataRepository, entities::item_value,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Section Items Tests\", DeviceId=\"emby-section-items-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_section_items_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn section_items_reuse_authorized_batched_items_contract_without_leaking_to_jellyfin() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
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
        exercise_section_item_routes(&task_database_name).await;
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

async fn exercise_section_item_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    fixture.persist_sections().await;
    fixture.assert_authorization_and_route_casing().await;
    fixture
        .assert_defaults_overrides_and_emby_projection()
        .await;
    fixture.assert_protocol_isolation(database.clone()).await;

    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
    folder_id: Uuid,
    favorite_zulu_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("section-items-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("section-items-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("section-items-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("section-items-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let folder = create_item(&items, "Folder", "Section folder", root.id, true).await;
        let other_folder = create_item(&items, "Folder", "Other folder", root.id, true).await;
        let favorite_alpha = create_item(&items, "Movie", "Alpha favorite", folder.id, false).await;
        let favorite_zulu = create_item(&items, "Movie", "Zulu favorite", folder.id, false).await;
        let _not_favorite = create_item(&items, "Movie", "Beta ordinary", folder.id, false).await;
        let outside =
            create_item(&items, "Movie", "Outside favorite", other_folder.id, false).await;

        let user_data = UserDataRepository::new(database.clone());
        for item_id in [favorite_alpha.id, favorite_zulu.id, outside.id] {
            let mut data = NewUserData::new(item_id, user.id, "main");
            data.is_favorite = true;
            user_data.upsert(data).await.expect("favorite user data");
        }
        let values = ItemValueRepository::new(database.clone());
        values
            .link(
                favorite_zulu.id,
                item_value::ItemValueType::Studios,
                "Section Studio",
            )
            .await
            .expect("studio relation");
        values
            .link(
                favorite_zulu.id,
                item_value::ItemValueType::Genre,
                "Section Genre",
            )
            .await
            .expect("genre relation");

        let state = AppState::new(
            database,
            "Emby Section Items Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            api_key_token,
            folder_id: folder.id,
            favorite_zulu_id: favorite_zulu.id,
        }
    }

    async fn persist_sections(&self) {
        for (user_id, token) in [
            (self.user_id, self.user_token.as_str()),
            (self.other_user_id, self.admin_token.as_str()),
        ] {
            let response = request_json(
                &self.emby,
                "POST",
                &format!("/emby/Users/{user_id}/HomeSections"),
                Some(token),
                json!({
                    "Id": "Featured",
                    "ParentId": self.folder_id,
                    "SortBy": "SortName",
                    "SortOrder": "Descending",
                    "Query": {"IsFavorite": true}
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    async fn assert_authorization_and_route_casing(&self) {
        let other_route = format!("/emby/Users/{}/Sections/Featured/Items", self.other_user_id);
        assert_eq!(
            request_raw(&self.emby, &other_route, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request_raw(&self.emby, &other_route, Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request_raw(&self.emby, &other_route, Some(&self.admin_token))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            request_raw(
                &self.emby,
                &format!("{other_route}?api_key={}", self.api_key_token),
                None,
            )
            .await
            .status(),
            StatusCode::OK
        );

        for route in [
            format!(
                "/emby/Users/{}/Sections/fEaTuReD/Items?Limit=1",
                self.user_id
            ),
            format!(
                "/emby/users/{}/sections/featured/items?limit=1",
                self.user_id
            ),
            format!(
                "/emby/uSeRs/{}/sEcTiOnS/FEATURED/iTeMs?LiMiT=1",
                self.user_id
            ),
        ] {
            assert_eq!(
                request_raw(&self.emby, &route, Some(&self.user_token))
                    .await
                    .status(),
                StatusCode::OK,
                "{route}"
            );
        }
        assert_eq!(
            request_raw(
                &self.emby,
                &format!("/emby/Users/{}/Sections/missing/Items", self.user_id),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    async fn assert_defaults_overrides_and_emby_projection(&self) {
        let route = format!(
            "/emby/Users/{}/Sections/Featured/Items?StartIndex=-1&Limit=1&Fields=Studios",
            self.user_id
        );
        let page =
            response_json(request_raw(&self.emby, &route, Some(&self.user_token)).await).await;
        assert_query_result_shape(&page);
        assert_eq!(page["StartIndex"], -1);
        assert_eq!(page["TotalRecordCount"], 2);
        assert_eq!(page["Items"].as_array().unwrap().len(), 1);
        assert_eq!(page["Items"][0]["Name"], "Zulu favorite");
        assert_eq!(
            page["Items"][0]["Id"],
            self.favorite_zulu_id.simple().to_string()
        );
        assert!(
            page["Items"][0].get("Studios").is_none(),
            "the shared Emby BaseItem adapter must remove GUID NameLongIdPair relations"
        );
        assert!(page["Items"][0].get("GenreItems").is_none());

        let override_route = format!(
            "/emby/Users/{}/Sections/Featured/Items?parentid={}&ISFAVORITE=false&sortorder=Ascending",
            self.user_id, self.folder_id
        );
        let page =
            response_json(request_raw(&self.emby, &override_route, Some(&self.user_token)).await)
                .await;
        assert_eq!(page["TotalRecordCount"], 1);
        assert_eq!(page["Items"][0]["Name"], "Beta ordinary");

        let zero_limit = format!(
            "/emby/Users/{}/Sections/Featured/Items?limit=0",
            self.user_id
        );
        let page =
            response_json(request_raw(&self.emby, &zero_limit, Some(&self.user_token)).await).await;
        assert_eq!(page["TotalRecordCount"], 2);
        assert_eq!(page["Items"], json!([]));
    }

    async fn assert_protocol_isolation(&self, database: DatabaseConnection) {
        let jellyfin = jellyfin_api::router(AppState::new(
            database,
            "Jellyfin Section Isolation Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        let section_route = format!("/Users/{}/Sections/Featured/Items", self.user_id);
        assert_eq!(
            request_raw(&jellyfin, &section_route, Some(&self.user_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "the Emby-only section route must not leak into Jellyfin"
        );

        let jellyfin_item = format!(
            "/Users/{}/Items?Ids={}&Fields=Studios",
            self.user_id, self.favorite_zulu_id
        );
        let page =
            response_json(request_raw(&jellyfin, &jellyfin_item, Some(&self.user_token)).await)
                .await;
        assert_eq!(page["Items"][0]["Studios"][0]["Name"], "Section Studio");
        assert_eq!(page["Items"][0]["GenreItems"][0]["Name"], "Section Genre");
    }
}

fn assert_query_result_shape(page: &Value) {
    assert!(page["Items"].is_array());
    assert!(page["TotalRecordCount"].is_number());
    assert!(page["StartIndex"].is_number());
}

async fn create_item(
    items: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    is_folder: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = is_folder;
    items.create(item).await.expect("section item creation")
}

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .expect("route response")
}

async fn request_raw(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("route response")
}

async fn response_json(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Section Items Tests",
            "1.0",
            "Test",
            format!("emby-section-items-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
