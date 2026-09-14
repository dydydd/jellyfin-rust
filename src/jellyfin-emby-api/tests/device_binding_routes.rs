use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceOptionsRepository, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Device Binding Tests\", DeviceId=\"device-binding-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_device_binding_";

#[tokio::test]
async fn device_id_and_options_bind_case_insensitively_across_protocol_trees() {
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
        panic!("temporary database task was cancelled: {error}");
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

    for (app, prefix, expected_status, custom_name) in [
        (&fixture.jellyfin, "", StatusCode::NO_CONTENT, "Root Room"),
        (
            &fixture.jellyfin,
            "/api",
            StatusCode::NO_CONTENT,
            "API Room",
        ),
        (&fixture.emby, "/emby", StatusCode::OK, "Emby Room"),
    ] {
        let update_path = format!(
            "{prefix}/Devices/Options?Id=wrong-device&iD={}&Unknown=query",
            fixture.device_id
        );
        let body = format!(
            r#"{{"CustomName":"wrong name","Unknown":{{"Nested":true}},"cUsToMnAmE":"{custom_name}"}}"#
        );
        let response = request(
            app,
            Method::POST,
            &update_path,
            Some(&fixture.admin_token),
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), expected_status, "{update_path}");

        for resource in ["Options", "Info"] {
            let path = format!(
                "{prefix}/Devices/{resource}?ID=wrong-device&iD={}&uNkNoWn=value",
                fixture.device_id
            );
            let response = request(app, Method::GET, &path, Some(&fixture.admin_token), None).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let value = response_json(response).await;
            assert_eq!(value["CustomName"], custom_name, "{path}");
            let id_property = if resource == "Options" {
                "DeviceId"
            } else {
                "Id"
            };
            assert_eq!(value[id_property], fixture.device_id, "{path}");
        }
    }

    let persisted = DeviceOptionsRepository::new(database.clone())
        .get(&fixture.device_id)
        .await
        .expect("device options lookup")
        .expect("device options row");
    assert_eq!(persisted.custom_name.as_deref(), Some("Emby Room"));

    for (app, path) in [
        (&fixture.jellyfin, "/Devices/Options"),
        (&fixture.jellyfin, "/api/Devices/Options"),
        (&fixture.emby, "/emby/Devices/Options"),
    ] {
        let response = request(app, Method::POST, path, None, Some("not-json")).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "authorization must retain precedence at {path}",
        );
    }

    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    device_id: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create_initial_administrator(&format!("device-binding-admin-{suffix}"))
            .await
            .expect("administrator");
        let device_id = format!("device-binding-{suffix}");
        let admin_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Device Binding Tests",
                "1.0",
                "Test Device",
                &device_id,
            ))
            .await
            .expect("administrator session")
            .access_token;
        let state = AppState::new(
            database,
            "Device Binding Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            device_id,
        }
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("response JSON")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
