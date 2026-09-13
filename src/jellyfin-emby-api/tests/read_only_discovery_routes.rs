use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Discovery Tests\", DeviceId=\"emby-discovery-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_read_discovery_";

#[tokio::test]
async fn read_only_discovery_routes_match_auth_wire_and_protocol_boundaries() {
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
        exercise(&task_database_name).await;
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

async fn exercise(database_name: &str) {
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
    assert_public_swagger_alias(&fixture).await;
    assert_protocol_specific_authentication(&fixture).await;
    assert_sync_discovery(&fixture).await;
    assert_dlna_administrator_boundary(&fixture).await;
    assert_protocol_isolation(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-discovery-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-discovery-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-discovery-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Discovery Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            admin_token,
            user_token,
            api_key_token,
        }
    }
}

async fn assert_public_swagger_alias(fixture: &Fixture) {
    for path in ["/emby/swagger", "/emby/sWaGgEr"] {
        let response = request(&fixture.emby, path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8",
            "{path}"
        );
        let document = response_json(response).await;
        assert_eq!(document["openapi"], "3.1.0", "{path}");
        assert!(document["paths"].is_object(), "{path}");
    }
}

async fn assert_protocol_specific_authentication(fixture: &Fixture) {
    for path in [
        "/emby/System/Info/Public",
        "/emby/sYsTeM/iNfO/pUbLiC",
        "/emby/Branding/Configuration",
        "/emby/bRaNdInG/cOnFiGuRaTiOn",
        "/emby/Branding/Css",
        "/emby/bRaNdInG/cSs.CsS",
    ] {
        assert_eq!(
            request_method(&fixture.emby, Method::GET, path, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "Emby contract requires authentication for {path}",
        );
        assert_eq!(
            request_method(&fixture.emby, Method::GET, path, Some(&fixture.user_token),)
                .await
                .status(),
            StatusCode::OK,
            "authenticated Emby route {path}",
        );
    }

    for method in [Method::GET, Method::POST, Method::HEAD] {
        let path = "/emby/sYsTeM/pInG";
        assert_eq!(
            request_method(&fixture.emby, method.clone(), path, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "Emby ping requires authentication for {method}",
        );
        assert_eq!(
            request_method(
                &fixture.emby,
                method.clone(),
                path,
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::OK,
            "authenticated Emby ping {method}",
        );
    }

    let path = "/emby/fEaTuReS";
    assert_eq!(
        request(&fixture.emby, path, None).await.status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        request(&fixture.emby, path, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    for token in [&fixture.admin_token, &fixture.api_key_token] {
        assert_eq!(
            request(&fixture.emby, path, Some(token)).await.status(),
            StatusCode::OK,
        );
    }
}

async fn assert_sync_discovery(fixture: &Fixture) {
    let user_id = fixture.user_id.simple();
    let routes = [
        (format!("/emby/sYnC/tArGeTs?uSeRiD={user_id}"), json!([])),
        (
            "/emby/sYnC/jObS".to_owned(),
            json!({"Items": [], "TotalRecordCount": 0}),
        ),
        (
            "/emby/sYnC/jObItEmS?tArGeTiD=device".to_owned(),
            json!({"Items": [], "TotalRecordCount": 0}),
        ),
        (
            "/emby/sYnC/iTeMs/rEaDy?tArGeTiD=device".to_owned(),
            json!([]),
        ),
    ];
    for (path, expected) in routes {
        assert_eq!(
            request(&fixture.emby, &path, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "authorization must precede query binding for {path}"
        );
        for token in [&fixture.user_token, &fixture.api_key_token] {
            let response = request(&fixture.emby, &path, Some(token)).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response_json(response).await, expected, "{path}");
        }
    }

    for path in [
        "/emby/Sync/Targets",
        "/emby/Sync/JobItems",
        "/emby/Sync/Items/Ready",
    ] {
        assert_eq!(
            request(&fixture.emby, path, Some(&fixture.user_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "missing generated-client required query must fail for {path}"
        );
    }
}

async fn assert_dlna_administrator_boundary(fixture: &Fixture) {
    let path = "/emby/dLnA/pRoFiLeInFoS";
    assert_eq!(
        request(&fixture.emby, path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&fixture.emby, path, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for token in [&fixture.admin_token, &fixture.api_key_token] {
        let response = request(&fixture.emby, path, Some(token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await, json!([]));
    }
}

async fn assert_protocol_isolation(fixture: &Fixture) {
    for path in [
        "/swagger",
        "/api/swagger",
        "/Sync/Targets?UserId=user",
        "/api/Sync/Targets?UserId=user",
        "/Sync/Jobs",
        "/api/Sync/Jobs",
        "/Sync/JobItems?TargetId=device",
        "/api/Sync/JobItems?TargetId=device",
        "/Sync/Items/Ready?TargetId=device",
        "/api/Sync/Items/Ready?TargetId=device",
        "/Dlna/ProfileInfos",
        "/api/Dlna/ProfileInfos",
    ] {
        assert_eq!(
            request(&fixture.jellyfin, path, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "Emby-only route leaked into Jellyfin at {path}"
        );
    }
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    request_method(app, Method::GET, uri, token).await
}

async fn request_method(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("discovery request"))
        .await
        .expect("discovery route response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .expect("bounded discovery response"),
    )
    .expect("JSON discovery response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Discovery Tests",
            "1.0",
            "Test",
            format!("emby-discovery-tests-{suffix}"),
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
