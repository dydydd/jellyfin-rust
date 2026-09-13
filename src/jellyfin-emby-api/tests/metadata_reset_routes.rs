use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Metadata Reset Tests\", DeviceId=\"emby-metadata-reset-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_metadata_reset_";

#[tokio::test]
async fn metadata_reset_is_authorized_atomic_postgres_backed_and_emby_only() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary metadata-reset database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary metadata-reset database cleanup must succeed");
    administrator.close().await.expect("admin pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("metadata-reset test task was cancelled: {error}");
    }
}

async fn exercise_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    assert_authorization_precedes_binding(&fixture).await;
    assert_validation_and_batch_atomicity(&fixture).await;
    assert_success_contract_and_persisted_effect(&fixture).await;
    assert_jellyfin_root_isolation(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    first_item_id: Uuid,
    second_item_id: Uuid,
    administrator_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("metadata-reset-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("metadata-reset-user-{suffix}"))
            .await
            .expect("ordinary user creation");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session(&devices, administrator.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("metadata-reset-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let first_item_id = Uuid::new_v4();
        let mut first = NewBaseItem::new(first_item_id, "Movie");
        first.name = Some("First reset movie".to_owned());
        first.data = Some(json!({
            "isLocked": true,
            "LOCKEDFIELDS": ["Name", "Overview"],
            "ProviderIds": { "Custom": "first-id" },
            "CustomMetadata": { "preserve": true }
        }));
        items.create(first).await.expect("first item creation");

        let second_item_id = Uuid::new_v4();
        let mut second = NewBaseItem::new(second_item_id, "Episode");
        second.data = Some(json!({
            "IsLocked": true,
            "LockedFields": "Genres|Studios",
            "ProviderIds": { "Custom": "second-id", "AnotherProvider": "opaque" },
            "OriginalTitle": "Background Refreshed Episode"
        }));
        items.create(second).await.expect("second item creation");

        let state = AppState::new(
            database.clone(),
            "Emby Metadata Reset Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_omdb_api_key("");
        let emby = jellyfin_emby_api::router(state.clone());
        let jellyfin = jellyfin_api::router(state);
        Self {
            database,
            emby,
            jellyfin,
            first_item_id,
            second_item_id,
            administrator_token,
            user_token,
            api_key,
        }
    }
}

async fn assert_authorization_precedes_binding(fixture: &Fixture) {
    assert_eq!(
        request(&fixture.emby, "/emby/Items/Metadata/Reset", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Metadata/Reset?ItemIds=not-a-uuid",
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "administrator policy must precede ItemIds parsing",
    );
}

async fn assert_validation_and_batch_atomicity(fixture: &Fixture) {
    for suffix in ["", "?ItemIds=", "?ItemIds=not-a-uuid"] {
        assert_eq!(
            request(
                &fixture.emby,
                &format!("/emby/Items/Metadata/Reset{suffix}"),
                Some(&fixture.administrator_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
        );
    }
    assert_eq!(
        request(
            &fixture.emby,
            &format!(
                "/emby/Items/Metadata/Reset?ItemIds={},,{}",
                fixture.first_item_id, fixture.second_item_id
            ),
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    let missing = Uuid::new_v4();
    assert_eq!(
        request(
            &fixture.emby,
            &format!(
                "/emby/Items/Metadata/Reset?ItemIds={},{}",
                fixture.first_item_id, missing
            ),
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    let unchanged = item(fixture, fixture.first_item_id).await;
    let data = unchanged.data.expect("unchanged metadata");
    assert_eq!(metadata_value(&data, "IsLocked"), Some(&json!(true)));
    assert_eq!(
        metadata_value(&data, "LockedFields"),
        Some(&json!(["Name", "Overview"]))
    );
}

async fn assert_success_contract_and_persisted_effect(fixture: &Fixture) {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        request(
            &fixture.emby,
            &format!(
                "/emby/Items/Metadata/Reset?itemids={},{}",
                fixture.first_item_id, fixture.first_item_id
            ),
            Some(&fixture.administrator_token),
        ),
    )
    .await
    .expect("accepted metadata reset must not await remote refresh work");
    assert_ok_empty(response).await;
    assert_reset(item(fixture, fixture.first_item_id).await.data);

    let response = request(
        &fixture.emby,
        &format!(
            "/emby/iTeMs/mEtAdAtA/rEsEt?ItemIds={}&iTeMiDs={}",
            Uuid::new_v4(),
            fixture.second_item_id
        ),
        Some(&fixture.api_key),
    )
    .await;
    assert_ok_empty(response).await;
    let refreshed = wait_for_background_refresh(fixture, fixture.second_item_id).await;
    assert_eq!(
        refreshed.name.as_deref(),
        Some("Background Refreshed Episode"),
        "the assertion must observe state after the queued refresh completed",
    );
    let provider_ids = metadata_value(
        refreshed.data.as_ref().expect("refreshed metadata"),
        "ProviderIds",
    )
    .and_then(Value::as_object)
    .expect("provider ids after queued refresh");
    assert_eq!(provider_ids.get("Custom"), Some(&json!("second-id")));
    assert_eq!(provider_ids.get("AnotherProvider"), Some(&json!("opaque")));
    assert_reset(refreshed.data);
}

async fn assert_jellyfin_root_isolation(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.jellyfin,
            &format!("/Items/Metadata/Reset?ItemIds={}", fixture.first_item_id),
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
}

async fn item(fixture: &Fixture, item_id: Uuid) -> jellyfin_data::entities::base_item::Model {
    BaseItemRepository::new(fixture.database.clone())
        .get(item_id)
        .await
        .expect("item lookup")
        .expect("persisted item")
}

async fn wait_for_background_refresh(
    fixture: &Fixture,
    item_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let item = item(fixture, item_id).await;
            if item.name.as_deref() == Some("Background Refreshed Episode") {
                break item;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued metadata refresh must complete")
}

fn assert_reset(data: Option<Value>) {
    let data = data.expect("reset metadata");
    assert_eq!(metadata_value(&data, "IsLocked"), Some(&json!(false)));
    assert_eq!(metadata_value(&data, "LockedFields"), Some(&json!([])));
    assert!(metadata_value(&data, "ProviderIds").is_some());
}

fn metadata_value<'a>(data: &'a Value, name: &str) -> Option<&'a Value> {
    data.as_object()?
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().method(Method::POST).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn assert_ok_empty(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("response body")
            .is_empty(),
        "generated Java and Swift clients decode Void",
    );
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Metadata Reset Tests",
            "1.0",
            "Test",
            format!("emby-metadata-reset-tests-{suffix}"),
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
