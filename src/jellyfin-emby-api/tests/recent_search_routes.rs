use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, UserSearchStateRepository,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Search State Tests\", DeviceId=\"emby-search-state-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_search_state_";

#[tokio::test]
async fn recent_search_routes_are_authorized_postgres_backed_and_emby_only() {
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
        exercise_routes(&task_database_name).await;
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
    assert_authorization_and_validation_precedence(&fixture).await;
    assert_reporting_semantics_and_user_isolation(&fixture).await;
    assert_clear_aliases_are_idempotent(&fixture).await;
    assert_elevated_cross_user_and_missing_target_behavior(&fixture).await;
    assert_user_delete_cascades(&fixture).await;
    assert_protocol_isolation(&fixture, database.clone()).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    user_token: String,
    administrator_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("search-state-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("search-state-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("search-state-other-{suffix}"))
            .await
            .expect("other user creation");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session(&devices, administrator.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("search-state-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let app = jellyfin_emby_api::router(AppState::new(
            database.clone(),
            "Emby Search State Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            user_token,
            administrator_token,
            api_key,
        }
    }
}

async fn assert_authorization_and_validation_precedence(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            "/emby/Users/not-a-uuid/SearchedItems/",
            Some("{"),
            None,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "authentication must precede path and body validation",
    );

    let cross_user = report_route(fixture.other_user_id, true);
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &cross_user,
            Some("{"),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "target authorization must precede malformed-body validation",
    );

    let own_route = report_route(fixture.user_id, true);
    for body in [
        None,
        Some("{"),
        Some("[]"),
        Some("true"),
        Some(r#"{"WasSearched":"true"}"#),
    ] {
        assert_eq!(
            request(
                &fixture.app,
                Method::POST,
                &own_route,
                body,
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "invalid ReportItemsSearched body must be rejected: {body:?}",
        );
    }
}

async fn assert_reporting_semantics_and_user_isolation(fixture: &Fixture) {
    let repository = UserSearchStateRepository::new(fixture.database.clone());
    assert!(repository.get(fixture.user_id).await.unwrap().is_none());
    assert!(
        repository
            .get(fixture.other_user_id)
            .await
            .unwrap()
            .is_none()
    );

    let mixed_case = format!("/emby/uSeRs/{}/sEaRcHeDiTeMs/", fixture.user_id);
    let response = request(
        &fixture.app,
        Method::POST,
        &mixed_case,
        Some(r#"{"wAsSeArChEd":true,"Unknown":{"ignored":1}}"#),
        Some(&fixture.user_token),
    )
    .await;
    assert_ok_empty(response).await;
    assert!(stored(fixture, fixture.user_id).await.was_searched);
    assert!(
        repository
            .get(fixture.other_user_id)
            .await
            .unwrap()
            .is_none()
    );

    let lowercase_without_slash = report_route(fixture.user_id, false);
    let response = request(
        &fixture.app,
        Method::POST,
        &lowercase_without_slash,
        Some(r#"{"WasSearched":true,"wassearched":false}"#),
        Some(&fixture.user_token),
    )
    .await;
    assert_ok_empty(response).await;
    assert!(!stored(fixture, fixture.user_id).await.was_searched);

    for body in [r#"{"WasSearched":null}"#, "{}"] {
        assert_ok_empty(
            request(
                &fixture.app,
                Method::POST,
                &report_route(fixture.user_id, true),
                Some(body),
                Some(&fixture.user_token),
            )
            .await,
        )
        .await;
        assert!(
            !stored(fixture, fixture.user_id).await.was_searched,
            "the nullable generated property uses the CLR bool default false",
        );
    }
}

async fn assert_clear_aliases_are_idempotent(fixture: &Fixture) {
    let repository = UserSearchStateRepository::new(fixture.database.clone());
    for (method, path) in [
        (
            Method::DELETE,
            format!("/emby/Users/{}/RecentlySearched", fixture.user_id),
        ),
        (
            Method::POST,
            format!("/emby/users/{}/recentlysearched/delete", fixture.user_id),
        ),
    ] {
        repository
            .set(fixture.user_id, true)
            .await
            .expect("search state seed");
        assert_ok_empty(
            request(
                &fixture.app,
                method.clone(),
                &path,
                None,
                Some(&fixture.user_token),
            )
            .await,
        )
        .await;
        assert!(repository.get(fixture.user_id).await.unwrap().is_none());
        assert_ok_empty(
            request(&fixture.app, method, &path, None, Some(&fixture.user_token)).await,
        )
        .await;
    }

    repository
        .set(fixture.user_id, true)
        .await
        .expect("mixed-case clear seed");
    let mixed_case = format!("/emby/uSeRs/{}/rEcEnTlYsEaRcHeD/dElEtE", fixture.user_id);
    assert_ok_empty(
        request(
            &fixture.app,
            Method::POST,
            &mixed_case,
            None,
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert!(repository.get(fixture.user_id).await.unwrap().is_none());
}

async fn assert_elevated_cross_user_and_missing_target_behavior(fixture: &Fixture) {
    for token in [&fixture.administrator_token, &fixture.api_key] {
        assert_ok_empty(
            request(
                &fixture.app,
                Method::POST,
                &report_route(fixture.other_user_id, true),
                Some(r#"{"WasSearched":true}"#),
                Some(token),
            )
            .await,
        )
        .await;
        assert!(stored(fixture, fixture.other_user_id).await.was_searched);

        let missing = Uuid::new_v4();
        assert_eq!(
            request(
                &fixture.app,
                Method::POST,
                &report_route(missing, true),
                Some("{"),
                Some(token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "target lookup must precede malformed-body validation",
        );
        assert_eq!(
            request(
                &fixture.app,
                Method::DELETE,
                &format!("/emby/Users/{missing}/RecentlySearched"),
                None,
                Some(token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
        );
    }
}

async fn assert_user_delete_cascades(fixture: &Fixture) {
    let users = UserService::new(fixture.database.clone());
    let user = users
        .create(&format!("search-state-cascade-{}", Uuid::new_v4().simple()))
        .await
        .expect("cascade user creation");
    let repository = UserSearchStateRepository::new(fixture.database.clone());
    repository
        .set(user.id, true)
        .await
        .expect("cascade state seed");
    users.delete(user.id).await.expect("cascade user deletion");
    assert!(repository.get(user.id).await.unwrap().is_none());
}

async fn assert_protocol_isolation(fixture: &Fixture, database: DatabaseConnection) {
    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    for (method, path) in [
        (
            Method::DELETE,
            format!("/Users/{}/RecentlySearched", fixture.user_id),
        ),
        (
            Method::POST,
            format!("/Users/{}/RecentlySearched/Delete", fixture.user_id),
        ),
        (
            Method::POST,
            format!("/Users/{}/SearchedItems/", fixture.user_id),
        ),
    ] {
        assert_eq!(
            request(
                &jellyfin,
                method,
                &path,
                Some(r#"{"WasSearched":true}"#),
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
        );
    }
}

async fn stored(
    fixture: &Fixture,
    user_id: Uuid,
) -> jellyfin_data::entities::user_search_state::Model {
    UserSearchStateRepository::new(fixture.database.clone())
        .get(user_id)
        .await
        .expect("search state lookup")
        .expect("persisted search state")
}

fn report_route(user_id: Uuid, trailing_slash: bool) -> String {
    format!(
        "/emby/{}/{user_id}/{}{}",
        if trailing_slash { "Users" } else { "users" },
        if trailing_slash {
            "SearchedItems"
        } else {
            "searcheditems"
        },
        if trailing_slash { "/" } else { "" },
    )
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    token: Option<&str>,
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

async fn assert_ok_empty(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("response body")
            .is_empty(),
        "the generated operations declare an empty response",
    );
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Search State Tests",
            "1.0",
            "Test",
            format!("emby-search-state-tests-{suffix}"),
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
