use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
    entities::server_configuration,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_startup_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn startup_routes_persist_across_app_states_and_fail_closed() {
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
        exercise_startup_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_startup_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 6,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let users = UserService::new(database.clone());
    let initial_name = format!("startup-admin-{}", Uuid::new_v4().simple());
    let administrator = users
        .create_initial_administrator(&initial_name)
        .await
        .expect("startup administrator creation");
    let app_a = persistent_app(database.clone(), administrator.id).await;
    let app_b = persistent_app(database.clone(), administrator.id).await;

    assert_official_user_rows(&database, &app_a, &app_b, administrator.id, &initial_name).await;
    assert_configuration_roundtrip(&app_a, &app_b).await;

    let session = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            administrator.id,
            "Startup Persistence Tests",
            "1.0",
            "PostgreSQL",
            Uuid::new_v4().simple().to_string(),
        ))
        .await
        .expect("administrator session");

    let response = send(
        &app_a,
        Request::post("/Startup/Complete")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    for uri in ["/Startup/User", "/Startup/Configuration"] {
        let response = get(&app_b, uri, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let app_after_restart = persistent_app(database.clone(), administrator.id).await;
    let response = get(
        &app_after_restart,
        "/Startup/Configuration",
        Some(&session.access_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, configured_startup());

    let response = get(&app_after_restart, "/System/Info/Public", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let public_info = body_json(response).await;
    assert_eq!(public_info["ServerName"], "Persisted Server");
    assert_eq!(public_info["StartupWizardCompleted"], true);

    server_configuration::Entity::delete_by_id(1_i16)
        .exec(&database)
        .await
        .expect("missing-singleton fixture");
    for uri in ["/Startup/Configuration", "/System/Info/Public"] {
        let response = get(&app_after_restart, uri, None).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body_json(response).await["Message"],
            "Startup configuration persistence failed"
        );
    }
    assert!(
        server_configuration::Entity::find_by_id(1_i16)
            .one(&database)
            .await
            .expect("singleton lookup")
            .is_none(),
        "a request must never recreate a missing singleton as incomplete"
    );

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_official_user_rows(
    database: &DatabaseConnection,
    app_a: &axum::Router,
    app_b: &axum::Router,
    user_id: Uuid,
    initial_name: &str,
) {
    let response = get(app_a, "/Startup/User", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let initial = body_json(response).await;
    assert_eq!(initial["Name"], initial_name);
    assert_eq!(initial["Password"], Value::Null);

    let missing_user_app = persistent_app(database.clone(), Uuid::new_v4()).await;
    let response = post_json(
        &missing_user_app,
        "/Startup/User",
        &json!({ "Name": "missing", "Password": "first password" }),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = post_json(
        app_a,
        "/Startup/User",
        &json!({ "Name": "Persisted Admin", "Password": "first password" }),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = get(app_b, "/Startup/User", None).await;
    assert_eq!(body_json(response).await["Name"], "Persisted Admin");

    let response = post_json(
        app_a,
        "/Startup/User",
        &json!({ "Name": "attacker", "Password": "replacement" }),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        UserService::new(database.clone())
            .get(user_id)
            .await
            .expect("persisted startup user")
            .username,
        "Persisted Admin"
    );
}

async fn assert_configuration_roundtrip(app_a: &axum::Router, app_b: &axum::Router) {
    let response = post_json(
        app_a,
        "/Startup/Configuration",
        &json!({
            "ServerName": null,
            "UICulture": null,
            "MetadataCountryCode": null,
            "PreferredMetadataLanguage": null
        }),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = get(app_b, "/Startup/Configuration", None).await;
    assert_eq!(
        body_json(response).await,
        json!({
            "ServerName": "",
            "UICulture": "",
            "MetadataCountryCode": "",
            "PreferredMetadataLanguage": ""
        })
    );

    let response = post_json(app_a, "/Startup/Configuration", &configured_startup(), None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = get(app_b, "/Startup/Configuration", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, configured_startup());
}

fn configured_startup() -> Value {
    json!({
        "ServerName": "Persisted Server",
        "UICulture": "nl-BE",
        "MetadataCountryCode": "be",
        "PreferredMetadataLanguage": "nl"
    })
}

async fn persistent_app(database: DatabaseConnection, user_id: Uuid) -> axum::Router {
    let repository = ServerConfigurationRepository::new(database.clone());
    let configuration = repository.load().await.expect("server configuration load");
    jellyfin_api::router(
        AppState::new(
            database,
            configuration.server_name,
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_startup_user(user_id)
        .with_persistent_startup(repository),
    )
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    send(app, request.body(Body::empty()).unwrap()).await
}

async fn post_json(
    app: &axum::Router,
    uri: &str,
    body: &Value,
    token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    send(
        app,
        request
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap(),
    )
    .await
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
