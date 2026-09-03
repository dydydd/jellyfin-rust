use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DatabaseConfig, DeviceRepository, NamedConfigurationRepository, NewDevice, entities::user,
};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_fallback_font_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn fallback_font_routes_use_encoding_config_and_real_files() {
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
        exercise_fallback_font_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_fallback_font_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/FallbackFont/Fonts", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let no_config = fixture
        .get("/FallbackFont/Fonts", Some(&fixture.user_token))
        .await;
    assert_eq!(no_config.status(), StatusCode::OK);
    assert_eq!(body_json(no_config).await, json!([]));

    let no_config_file = fixture
        .get("/FallbackFont/Fonts/Missing.ttf", Some(&fixture.user_token))
        .await;
    assert_eq!(no_config_file.status(), StatusCode::OK);
    assert_eq!(body_bytes(no_config_file).await.len(), 0);

    let fonts = fixture.temporary.path().join("fonts");
    fs::create_dir_all(&fonts).unwrap();
    fs::write(fonts.join("Bigger.woff2"), b"larger-font").unwrap();
    fs::write(fonts.join("Small.TTF"), b"tiny").unwrap();
    fs::write(fonts.join("ignored.txt"), b"not-a-font").unwrap();
    fs::create_dir_all(fonts.join("nested")).unwrap();
    fs::write(fonts.join("nested/Nested.ttf"), b"ignored nested").unwrap();
    fixture
        .named_configurations
        .save(
            "encoding",
            json!({
                "FallbackFontPath": fonts,
                "EnableFallbackFont": true
            }),
        )
        .await
        .expect("encoding config save");

    let response = fixture
        .get("/FallbackFont/Fonts", Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert_eq!(body[0]["Name"], "Small.TTF");
    assert_eq!(body[0]["Size"], 4);
    assert!(body[0]["DateCreated"].as_str().unwrap().ends_with('Z'));
    assert_eq!(body[1]["Name"], "Bigger.woff2");
    assert_eq!(body[1]["Size"], 11);

    let file = fixture
        .get("/FallbackFont/Fonts/small.ttf", Some(&fixture.user_token))
        .await;
    assert_eq!(file.status(), StatusCode::OK);
    assert_eq!(file.headers()[header::CONTENT_TYPE], "font/ttf");
    assert_eq!(body_bytes(file).await.as_ref(), b"tiny");

    let unsupported = fixture
        .get("/FallbackFont/Fonts/ignored.txt", Some(&fixture.user_token))
        .await;
    assert_eq!(unsupported.status(), StatusCode::OK);
    assert_eq!(body_bytes(unsupported).await.len(), 0);

    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: axum::Router,
    named_configurations: NamedConfigurationRepository,
    temporary: TempDirectory,
    user_id: Uuid,
    user_token: String,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 4,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create(&format!("fallback-font-user-{suffix}"))
            .await
            .expect("user creation");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Fallback Font Tests",
                "1.0",
                "Test",
                format!("fallback-font-{suffix}"),
            ))
            .await
            .expect("session creation")
            .access_token;
        let temporary = TempDirectory::new();
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Fallback Font Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            named_configurations: NamedConfigurationRepository::new(database.clone()),
            database,
            app,
            temporary,
            user_id: user.id,
            user_token,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn body_bytes(response: axum::response::Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap()
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("jellyfin-fallback-font-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
