use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Sync Mutation Tests\", DeviceId=\"emby-sync-mutations\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_sync_mutations_";

#[tokio::test]
async fn retired_sync_mutations_are_bound_authenticated_and_emby_only() {
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

    let malformed = request(
        &fixture.emby,
        Method::POST,
        "/emby/sYnC/dAtA",
        None,
        Some("{}"),
    )
    .await;
    assert_eq!(
        malformed.status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede Sync/Data query binding",
    );
    for (path, body) in [
        ("/emby/sYnC/dAtA", Some("{}")),
        ("/emby/sYnC/jObS", None),
        ("/emby/sYnC/jObS/not-an-int", Some("{}")),
    ] {
        let response = request(
            &fixture.emby,
            Method::POST,
            path,
            Some(&fixture.user_token),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
    }

    let unavailable = [
        (Method::POST, "/emby/sYnC/jObS", Some("{}")),
        (Method::POST, "/emby/sYnC/oFfLiNeAcTiOnS", Some("[]")),
        (Method::POST, "/emby/sYnC/item/sTaTuS", Some("{}")),
        (Method::POST, "/emby/sYnC/jObS/1", Some("{}")),
        (Method::DELETE, "/emby/sYnC/jObS/job", None),
        (Method::POST, "/emby/sYnC/iTeMs/cAnCeL", None),
        (Method::DELETE, "/emby/sYnC/target/iTeMs", None),
        (Method::DELETE, "/emby/sYnC/jObItEmS/item", None),
        (Method::POST, "/emby/sYnC/jObS/job/dElEtE", None),
        (Method::POST, "/emby/sYnC/target/iTeMs/dElEtE", None),
        (Method::POST, "/emby/sYnC/jObItEmS/item/tRaNsFeRrEd", None),
        (Method::POST, "/emby/sYnC/jObItEmS/item/eNaBlE", None),
        (Method::POST, "/emby/sYnC/jObItEmS/item/dElEtE", None),
        (
            Method::POST,
            "/emby/sYnC/jObItEmS/item/mArKfOrReMoVaL",
            None,
        ),
        (
            Method::POST,
            "/emby/sYnC/jObItEmS/item/uNmArKfOrReMoVaL",
            None,
        ),
    ];
    for (method, path, body) in unavailable {
        for token in [&fixture.user_token, &fixture.api_key] {
            let response = request(&fixture.emby, method.clone(), path, Some(token), body).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        }
    }

    for token in [&fixture.user_token, &fixture.api_key] {
        let response = request(
            &fixture.emby,
            Method::POST,
            "/emby/sYnC/dAtA?tArGeTiD=first&TARGETID=second",
            Some(token),
            Some(r#"{"LocalItemIds":[],"InternalTargetIds":[]}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            serde_json::json!({"ItemIdsToRemove": []})
        );
    }

    let unprefixed = [
        (Method::POST, "/Sync/Jobs", Some("{}")),
        (Method::POST, "/Sync/OfflineActions", Some("[]")),
        (Method::POST, "/Sync/Data?TargetId=target", Some("{}")),
        (Method::POST, "/Sync/item/Status", Some("{}")),
        (Method::POST, "/Sync/Jobs/1", Some("{}")),
        (Method::DELETE, "/Sync/Jobs/job", None),
        (Method::POST, "/Sync/Items/Cancel", None),
        (Method::DELETE, "/Sync/target/Items", None),
        (Method::DELETE, "/Sync/JobItems/item", None),
        (Method::POST, "/Sync/Jobs/job/Delete", None),
        (Method::POST, "/Sync/target/Items/Delete", None),
        (Method::POST, "/Sync/JobItems/item/Transferred", None),
        (Method::POST, "/Sync/JobItems/item/Enable", None),
        (Method::POST, "/Sync/JobItems/item/Delete", None),
        (Method::POST, "/Sync/JobItems/item/MarkForRemoval", None),
        (Method::POST, "/Sync/JobItems/item/UnmarkForRemoval", None),
    ];
    for (method, path, body) in unprefixed {
        for prefix in ["", "/api"] {
            let path = format!("{prefix}{path}");
            let response = request(
                &fixture.jellyfin,
                method.clone(),
                &path,
                Some(&fixture.user_token),
                body,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "Emby-only Sync mutation leaked at {method} {path}",
            );
        }
    }

    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let user = users
            .create_initial_administrator(&format!("sync-mutation-user-{suffix}"))
            .await
            .expect("test user");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Sync Mutation Tests",
                "1.0",
                "Test",
                format!("emby-sync-mutations-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("sync-mutation-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Sync Mutation Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
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
