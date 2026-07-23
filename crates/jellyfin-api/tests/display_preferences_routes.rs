use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Display Preferences Tests\", DeviceId=\"display-preferences-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_display_preferences_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn display_preferences_routes_persist_official_dto_in_postgres() {
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
        exercise_display_preferences_routes(&task_database_name).await;
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

async fn exercise_display_preferences_routes(database_name: &str) {
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
    assert_auth_query_and_target_rules(&fixture).await;
    assert_default_preferences(&fixture).await;
    assert_self_update_round_trips(&fixture).await;
    assert_admin_can_update_another_user_with_aliases(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("display-preferences-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("display-preferences-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("display-preferences-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("display-preferences-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        Self {
            app: jellyfin_api::router(AppState::new(
                database,
                "Display Preferences Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
            api_key_token,
        }
    }
}

async fn assert_auth_query_and_target_rules(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            "/DisplayPreferences/usersettings?client=web",
            None,
            Value::Null,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            "/DisplayPreferences/usersettings",
            Some(&fixture.user_token),
            Value::Null,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            &format!(
                "/DisplayPreferences/usersettings?userId={}&client=web",
                fixture.other_user_id
            ),
            Some(&fixture.user_token),
            Value::Null,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            &format!(
                "/DisplayPreferences/usersettings?userId={}&client=web",
                Uuid::new_v4()
            ),
            Some(&fixture.admin_token),
            Value::Null,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_default_preferences(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            &format!(
                "/DisplayPreferences/usersettings?client=web&api_key={}",
                fixture.api_key_token
            ),
            None,
            Value::Null,
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &fixture.app,
            "GET",
            &format!(
                "/DisplayPreferences/usersettings?userId={}&client=web&api_key={}",
                fixture.user_id, fixture.api_key_token
            ),
            None,
            Value::Null,
        )
        .await
        .status(),
        StatusCode::OK
    );

    let preferences = get_json(
        &fixture.app,
        "/DisplayPreferences/usersettings?client=web",
        &fixture.user_token,
    )
    .await;
    assert_eq!(preferences["Id"], "3ce5b65d-e116-d731-65d1-efc4a30ec35c");
    assert_eq!(preferences["Client"], "web");
    assert_eq!(preferences["SortBy"], "SortName");
    assert_eq!(preferences["SortOrder"], "Ascending");
    assert_eq!(preferences["ScrollDirection"], "Horizontal");
    assert_eq!(preferences["PrimaryImageHeight"], 250);
    assert_eq!(preferences["PrimaryImageWidth"], 250);
    assert_eq!(preferences["ShowBackdrop"], true);
    assert_eq!(preferences["CustomPrefs"]["chromecastVersion"], "stable");
    assert_eq!(preferences["CustomPrefs"]["skipForwardLength"], "15000");
    assert_eq!(
        preferences["CustomPrefs"]["enableNextVideoInfoOverlay"],
        "true"
    );
}

async fn assert_self_update_round_trips(fixture: &Fixture) {
    let item_id = Uuid::new_v4();
    let route = format!("/DisplayPreferences/{item_id}?client=web");
    let body = json!({
        "SortBy": "DateCreated",
        "SortOrder": "Descending",
        "IndexBy": "PremiereDate",
        "RememberIndexing": true,
        "RememberSorting": true,
        "ScrollDirection": "Vertical",
        "ShowBackdrop": false,
        "ShowSidebar": true,
        "PrimaryImageHeight": 320,
        "PrimaryImageWidth": 480,
        "CustomPrefs": {
            "chromecastVersion": "unstable",
            "landing-livetv": "Guide",
            "my-custom": "kept",
            "nullable": null
        }
    });

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &route,
            Some(&fixture.user_token),
            body,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let preferences = get_json(&fixture.app, &route, &fixture.user_token).await;
    assert_eq!(preferences["Id"], item_id.to_string());
    assert_eq!(preferences["Client"], "web");
    assert_eq!(preferences["SortBy"], "DateCreated");
    assert_eq!(preferences["SortOrder"], "Descending");
    assert_eq!(preferences["IndexBy"], "PremiereDate");
    assert_eq!(preferences["RememberIndexing"], true);
    assert_eq!(preferences["RememberSorting"], true);
    assert_eq!(preferences["ScrollDirection"], "Vertical");
    assert_eq!(preferences["ShowBackdrop"], false);
    assert_eq!(preferences["ShowSidebar"], true);
    assert_eq!(preferences["PrimaryImageHeight"], 320);
    assert_eq!(preferences["PrimaryImageWidth"], 480);
    assert_eq!(preferences["CustomPrefs"]["chromecastVersion"], "unstable");
    assert_eq!(preferences["CustomPrefs"]["landing-livetv"], "Guide");
    assert_eq!(preferences["CustomPrefs"]["my-custom"], "kept");
    assert!(preferences["CustomPrefs"]["nullable"].is_null());
    assert_eq!(preferences["CustomPrefs"]["skipBackLength"], "15000");
}

async fn assert_admin_can_update_another_user_with_aliases(fixture: &Fixture) {
    let body = json!({
        "SortOrder": "Descending",
        "CustomPrefs": {
            "dashboardTheme": "dark"
        }
    });

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!(
                "/DisplayPreferences/usersettings?UserId={}&Client=web",
                fixture.other_user_id
            ),
            Some(&fixture.admin_token),
            body,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!(
                "/DisplayPreferences/usersettings?userId={}&client=web",
                fixture.other_user_id
            ),
            Some(&fixture.user_token),
            json!({}),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let preferences = get_json(
        &fixture.app,
        &format!(
            "/DisplayPreferences/usersettings?userId={}&client=web",
            fixture.other_user_id
        ),
        &fixture.admin_token,
    )
    .await;
    assert_eq!(preferences["SortBy"], "SortName");
    assert_eq!(preferences["SortOrder"], "Descending");
    assert_eq!(preferences["CustomPrefs"]["dashboardTheme"], "dark");
}

async fn get_json(app: &axum::Router, uri: &str, token: &str) -> Value {
    let response = request(app, "GET", uri, Some(token), Value::Null).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_null() {
        app.clone()
            .oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("route response")
    } else {
        app.clone()
            .oneshot(
                request
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("route response")
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Display Preferences Tests",
            "1.0",
            "Test",
            format!("display-preferences-tests-{suffix}"),
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
