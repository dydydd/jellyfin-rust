use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Home Sections Tests\", DeviceId=\"emby-home-sections-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_home_sections_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn home_sections_enforce_target_authorization_and_persist_in_postgres() {
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
        exercise_home_sections_routes(&task_database_name).await;
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

async fn exercise_home_sections_routes(database_name: &str) {
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
    assert_target_authorization(&fixture).await;
    assert_mutations_and_ordering(&fixture).await;
    assert_persistence_and_protocol_isolation(&fixture, database.clone()).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    other_user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-home-sections-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-home-sections-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("emby-home-sections-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let other_user_token = session(&devices, other_user.id, &format!("other-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-home-sections-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        Self {
            app: jellyfin_emby_api::router(AppState::new(
                database,
                "Emby Home Sections Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            other_user_token,
            api_key_token,
        }
    }
}

async fn assert_target_authorization(fixture: &Fixture) {
    let other_route = format!("/emby/Users/{}/HomeSections", fixture.other_user_id);
    assert_eq!(
        request_raw(&fixture.app, "GET", &other_route, None, Body::empty())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request_raw(
            &fixture.app,
            "POST",
            &other_route,
            Some(&fixture.user_token),
            Body::from("{")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "target authorization must precede malformed JSON binding"
    );
    assert_eq!(
        request_json(
            &fixture.app,
            "POST",
            &other_route,
            Some(&fixture.admin_token),
            json!({"ID": "admin", "nAmE": "Administrator section"}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        get_json(
            &fixture.app,
            &format!("{other_route}?api_key={}", fixture.api_key_token),
            None,
        )
        .await,
        json!([{"Id": "admin", "Name": "Administrator section"}])
    );
}

async fn assert_mutations_and_ordering(fixture: &Fixture) {
    let canonical = format!("/emby/Users/{}/HomeSections", fixture.user_id);
    let mixed_case = format!("/emby/uSeRs/{}/hOmEsEcTiOnS", fixture.user_id);
    assert_eq!(
        get_json(&fixture.app, &canonical, Some(&fixture.user_token)).await,
        json!([])
    );

    for body in [
        json!({"id": "one", "NAME": "One", "unknown": "ignored"}),
        json!({"Id": "two", "Name": "Two", "scrollDirection": 1}),
        json!({"ID": "three", "nAmE": "Three"}),
    ] {
        assert_eq!(
            request_json(
                &fixture.app,
                "POST",
                &mixed_case,
                Some(&fixture.user_token),
                body,
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        request_json(
            &fixture.app,
            "POST",
            &canonical,
            Some(&fixture.user_token),
            json!({"Id": "ONE", "Name": "One updated"}),
        )
        .await
        .status(),
        StatusCode::OK
    );

    let sections = get_json(&fixture.app, &canonical, Some(&fixture.user_token)).await;
    assert_eq!(ids(&sections), ["ONE", "two", "three"]);
    assert_eq!(sections[0]["Name"], "One updated");
    assert_eq!(sections[1]["ScrollDirection"], "Vertical");
    assert!(sections[0].get("unknown").is_none());

    assert_eq!(
        request_json(
            &fixture.app,
            "POST",
            &format!("{mixed_case}/MoVe"),
            Some(&fixture.user_token),
            json!({"iDs": ["THREE", "one"], "newINDEX": 1}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ids(&get_json(&fixture.app, &canonical, Some(&fixture.user_token)).await),
        ["two", "ONE", "three"]
    );

    assert_eq!(
        request_json(
            &fixture.app,
            "POST",
            &format!("{canonical}/Move"),
            Some(&fixture.user_token),
            json!({"Ids": ["ONE"], "NewIndex": 3}),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        ids(&get_json(&fixture.app, &canonical, Some(&fixture.user_token)).await),
        ["two", "ONE", "three"],
        "an invalid move must not persist a partial reorder"
    );

    assert_eq!(
        request_json(
            &fixture.app,
            "POST",
            &format!("{mixed_case}/dElEtE"),
            Some(&fixture.user_token),
            json!({"ids": ["oNe"]}),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        ids(&get_json(&fixture.app, &canonical, Some(&fixture.user_token)).await),
        ["two", "three"]
    );
}

async fn assert_persistence_and_protocol_isolation(
    fixture: &Fixture,
    database: DatabaseConnection,
) {
    let emby_route = format!("/emby/Users/{}/HomeSections", fixture.user_id);
    let restarted = jellyfin_emby_api::router(AppState::new(
        database.clone(),
        "Emby Home Sections Restarted Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        ids(&get_json(&restarted, &emby_route, Some(&fixture.user_token)).await),
        ["two", "three"]
    );

    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Protocol Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        request_raw(
            &jellyfin,
            "GET",
            &format!("/Users/{}/HomeSections", fixture.user_id),
            Some(&fixture.other_user_token),
            Body::empty(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "Emby HomeSections must not be registered on the unprefixed Jellyfin tree"
    );
}

fn ids(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("home sections response must be an array")
        .iter()
        .map(|section| section["Id"].as_str().expect("section Id"))
        .collect()
}

async fn get_json(app: &axum::Router, uri: &str, token: Option<&str>) -> Value {
    let response = request_raw(app, "GET", uri, token, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    request_raw(app, method, uri, token, Body::from(body.to_string())).await
}

async fn request_raw(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Body,
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
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Home Sections Tests",
            "1.0",
            "Test",
            format!("emby-home-sections-tests-{suffix}"),
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
