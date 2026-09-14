use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceOptionsRepository, DeviceRepository, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Device Mutation Tests\", DeviceId=\"emby-device-mutation-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_device_mutation_";

#[tokio::test]
async fn device_mutations_use_generated_status_and_binding_only_below_emby() {
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
        max_connections: 10,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");
    let fixture = Fixture::new(database.clone()).await;

    fixture.assert_emby_delete_contract().await;
    fixture.assert_emby_options_status().await;
    fixture.assert_jellyfin_isolation().await;

    database.close().await.expect("database cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    user_token: String,
    api_key: String,
    delete_targets: [String; 5],
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("device-mutation-admin-{suffix}"))
            .await
            .expect("administrator");
        let user = users
            .create(&format!("device-mutation-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token =
            create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = create_session(&devices, user.id, &format!("user-{suffix}")).await;
        let delete_targets = [
            format!("emby-delete-{suffix}"),
            format!("emby-post-delete-{suffix}"),
            format!("emby-api-key-delete-{suffix}"),
            format!("root-delete-{suffix}"),
            format!("api-delete-{suffix}"),
        ];
        for device_id in &delete_targets {
            create_session(&devices, user.id, device_id).await;
        }
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("device-mutation-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database.clone(),
            "Emby Device Mutation Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            user_token,
            api_key,
            delete_targets,
        }
    }

    async fn assert_emby_delete_contract(&self) {
        for (method, path) in [
            (Method::DELETE, "/emby/Devices"),
            (Method::POST, "/emby/Devices/Delete"),
        ] {
            assert_eq!(
                request(&self.emby, method.clone(), path, None, None)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "authentication must precede Id binding: {method} {path}",
            );
            assert_eq!(
                request(
                    &self.emby,
                    method.clone(),
                    path,
                    Some(&self.user_token),
                    None,
                )
                .await
                .status(),
                StatusCode::FORBIDDEN,
                "ordinary users cannot delete devices: {method} {path}",
            );
            assert_eq!(
                request(
                    &self.emby,
                    method.clone(),
                    path,
                    Some(&self.admin_token),
                    None,
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "generated Id is required: {method} {path}",
            );
        }

        let cases = [
            (
                Method::DELETE,
                format!("/emby/dEvIcEs?Id=missing&iD={}", self.delete_targets[0]),
                &self.admin_token,
                &self.delete_targets[0],
            ),
            (
                Method::POST,
                format!(
                    "/emby/dEvIcEs/dElEtE?ID=missing&id={}",
                    self.delete_targets[1]
                ),
                &self.admin_token,
                &self.delete_targets[1],
            ),
            (
                Method::DELETE,
                format!("/emby/Devices?Id={}", self.delete_targets[2]),
                &self.api_key,
                &self.delete_targets[2],
            ),
        ];
        for (method, path, token, deleted_id) in cases {
            let response = request(&self.emby, method, &path, Some(token), None).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert!(response_bytes(response).await.is_empty(), "{path}");
            assert!(
                DeviceRepository::new(self.database.clone())
                    .latest_by_device_id(deleted_id)
                    .await
                    .expect("deleted device lookup")
                    .is_none(),
                "last Id must be the deleted target: {path}",
            );
        }
    }

    async fn assert_emby_options_status(&self) {
        let device_id = "emby-options-device";
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &format!("/emby/dEvIcEs/oPtIoNs?Id={device_id}"),
                Some(&self.user_token),
                Some(json!({"CustomName": "Denied"})),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
        );
        for (token, name) in [
            (&self.admin_token, "Living Room"),
            (&self.api_key, "API Key Room"),
        ] {
            let response = request(
                &self.emby,
                Method::POST,
                &format!("/emby/dEvIcEs/oPtIoNs?Id={device_id}"),
                Some(token),
                Some(json!({"CustomName": name})),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response_bytes(response).await.is_empty());
        }
        let stored = DeviceOptionsRepository::new(self.database.clone())
            .get(device_id)
            .await
            .expect("device options lookup")
            .expect("device options");
        assert_eq!(stored.custom_name.as_deref(), Some("API Key Room"));
    }

    async fn assert_jellyfin_isolation(&self) {
        for (method, path) in [
            (Method::DELETE, "/Devices"),
            (Method::POST, "/Devices/Delete"),
            (Method::DELETE, "/api/Devices"),
            (Method::POST, "/api/Devices/Delete"),
        ] {
            assert_eq!(
                request(
                    &self.jellyfin,
                    method.clone(),
                    path,
                    Some(&self.admin_token),
                    None,
                )
                .await
                .status(),
                StatusCode::NO_CONTENT,
                "Jellyfin omitted Id/status must stay unchanged: {method} {path}",
            );
        }
        for (prefix, deleted_id) in [
            ("", &self.delete_targets[3]),
            ("/api", &self.delete_targets[4]),
        ] {
            let response = request(
                &self.jellyfin,
                Method::DELETE,
                &format!("{prefix}/Devices?Id={deleted_id}"),
                Some(&self.admin_token),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert!(response_bytes(response).await.is_empty());
        }

        for (prefix, device_id) in [("", "root-options"), ("/api", "api-options")] {
            let response = request(
                &self.jellyfin,
                Method::POST,
                &format!("{prefix}/Devices/Options?Id={device_id}"),
                Some(&self.admin_token),
                Some(json!({"CustomName": "Jellyfin"})),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::NO_CONTENT,
                "Jellyfin Options status must stay unchanged: {prefix}",
            );
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

async fn response_bytes(response: axum::response::Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), 1024)
        .await
        .expect("bounded response")
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Device Mutation Tests",
            "1.0",
            "Test",
            device_id,
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
