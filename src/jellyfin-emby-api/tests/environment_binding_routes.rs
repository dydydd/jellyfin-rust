use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Environment Tests\", DeviceId=\"emby-environment-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_environment_";

#[tokio::test]
async fn environment_routes_bind_mobile_and_mixed_case_contracts() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");
    let fixture = Fixture::new(database.clone()).await;

    let missing_path = "/emby/eNvIrOnMeNt/dIrEcToRyCoNtEnTs";
    assert_eq!(
        request(&fixture.emby, Method::GET, missing_path, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
        "authorization must precede required Path binding",
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::GET,
            missing_path,
            Some(&fixture.user_token),
            None,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        request(
            &fixture.emby,
            Method::GET,
            missing_path,
            Some(&fixture.admin_token),
            None,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    for token in [&fixture.admin_token, &fixture.api_key] {
        let path = "/emby/eNvIrOnMeNt/dIrEcToRyCoNtEnTs?pAtH=%2Ftmp&iNcLuDeFiLeS=true&INCLUDEFILES=false&iNcLuDeDiReCtOrIeS=false";
        let response = request(&fixture.emby, Method::GET, path, Some(token), None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response_json(response).await, json!([]), "{path}");

        let path = "/emby/eNvIrOnMeNt/nEtWoRkShArEs?pAtH=ignored";
        let response = request(&fixture.emby, Method::GET, path, Some(token), None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response_json(response).await, json!([]), "{path}");

        let path = "/emby/eNvIrOnMeNt/vAlIdAtEpAtH?pAtH=%2Ftmp";
        let response = request(
            &fixture.emby,
            Method::POST,
            path,
            Some(token),
            Some(json!({
                "IsFile": true,
                "iSfIlE": false,
                "ValidateWriteable": false,
                "vAlIdAtEwRiTaBlE": true,
                "Unknown": "ignored"
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }

    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: Router,
    admin_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("environment-admin-{suffix}"))
            .await
            .expect("administrator");
        let user = users
            .create(&format!("environment-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("environment-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Environment Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state),
            admin_token,
            user_token,
            api_key,
        }
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = if let Some(value) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).expect("JSON body"))
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("route response")
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("response body");
    serde_json::from_slice(&body).expect("JSON response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Environment Tests",
            "1.0",
            "Test",
            format!("emby-environment-tests-{suffix}"),
        ))
        .await
        .expect("session")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
