use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
use jellyfin_model::UserConfiguration;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"User Configuration Tests\", DeviceId=\"user-configuration-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_user_configuration_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn user_configuration_routes_follow_official_contract_with_postgres_state() {
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
        exercise_user_configuration_routes(&task_database_name).await;
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

async fn exercise_user_configuration_routes(database_name: &str) {
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
    assert_auth_and_target_user_rules(&fixture).await;
    assert_self_update(&fixture).await;
    assert_admin_legacy_update(&fixture).await;
    assert_pascal_case_user_id_alias(&fixture).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    database: DatabaseConnection,
    user_id: Uuid,
    other_user_id: Uuid,
    admin_token: String,
    user_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("user-configuration-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("user-configuration-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("user-configuration-other-{suffix}"))
            .await
            .expect("other user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "User Configuration Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            admin_token,
            user_token,
        }
    }
}

async fn assert_auth_and_target_user_rules(fixture: &Fixture) {
    let body = configuration_body(|configuration| {
        configuration.display_missing_episodes = true;
    });
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            "/Users/Configuration",
            None,
            body.clone()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("/Users/Configuration?userId={}", fixture.other_user_id),
            Some(&fixture.user_token),
            body.clone(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("/Users/{}/Configuration", Uuid::new_v4()),
            Some(&fixture.admin_token),
            body,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_self_update(fixture: &Fixture) {
    let grouped_folder = Uuid::new_v4();
    let body = configuration_body(|configuration| {
        configuration.audio_language_preference = Some("eng".to_owned());
        configuration.display_missing_episodes = true;
        configuration.grouped_folders = vec![grouped_folder];
        configuration.hide_played_in_latest = false;
        configuration.cast_receiver_id = Some("living-room".to_owned());
    });

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            "/Users/Configuration",
            Some(&fixture.user_token),
            body,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let user = get_user(&fixture.app, fixture.user_id, &fixture.admin_token).await;
    let configuration = &user["Configuration"];
    assert_eq!(configuration["AudioLanguagePreference"], "eng");
    assert_eq!(configuration["DisplayMissingEpisodes"], true);
    assert_eq!(
        configuration["GroupedFolders"],
        json!([grouped_folder.simple().to_string()])
    );
    assert_eq!(configuration["HidePlayedInLatest"], false);
    assert_eq!(configuration["RememberAudioSelections"], true);
    assert_eq!(configuration["CastReceiverId"], "living-room");

    // Jellyfin denies self-service mutations when the user's policy disables
    // EnableUserPreferenceAccess, while administrators retain access.
    let users = UserService::new(fixture.database.clone());
    let mut policy = jellyfin_model::UserPolicy::default();
    policy.enable_user_preference_access = false;
    users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("disable preference access");
    let denied_configuration = request(
        &fixture.app,
        "POST",
        "/Users/Configuration",
        Some(&fixture.user_token),
        configuration_body(|configuration| configuration.display_missing_episodes = false),
    )
    .await;
    assert_eq!(denied_configuration.status(), StatusCode::FORBIDDEN);
    let denied_profile = request(
        &fixture.app,
        "POST",
        "/Users",
        Some(&fixture.user_token),
        serde_json::json!({ "Name": "renamed-while-denied", "Configuration": {} }),
    )
    .await;
    assert_eq!(denied_profile.status(), StatusCode::FORBIDDEN);
    let denied_password = request(
        &fixture.app,
        "POST",
        "/Users/Password",
        Some(&fixture.user_token),
        serde_json::json!({ "ResetPassword": true }),
    )
    .await;
    assert_eq!(denied_password.status(), StatusCode::FORBIDDEN);
}

async fn assert_admin_legacy_update(fixture: &Fixture) {
    let ordered_view = Uuid::new_v4();
    let latest_exclude = Uuid::new_v4();
    let body = configuration_body(|configuration| {
        configuration.display_collections_view = true;
        configuration.ordered_views = vec![ordered_view];
        configuration.latest_items_excludes = vec![latest_exclude];
        configuration.remember_subtitle_selections = false;
    });

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("/Users/{}/Configuration", fixture.other_user_id),
            Some(&fixture.admin_token),
            body,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let other = get_user(&fixture.app, fixture.other_user_id, &fixture.admin_token).await;
    let configuration = &other["Configuration"];
    assert_eq!(configuration["DisplayCollectionsView"], true);
    assert_eq!(
        configuration["OrderedViews"],
        json!([ordered_view.simple().to_string()])
    );
    assert_eq!(
        configuration["LatestItemsExcludes"],
        json!([latest_exclude.simple().to_string()])
    );
    assert_eq!(configuration["RememberSubtitleSelections"], false);
}

async fn assert_pascal_case_user_id_alias(fixture: &Fixture) {
    let body = configuration_body(|configuration| {
        configuration.my_media_excludes = vec![Uuid::new_v4()];
        configuration.enable_next_episode_auto_play = false;
    });

    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("/Users/Configuration?UserId={}", fixture.other_user_id),
            Some(&fixture.admin_token),
            body,
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );

    let other = get_user(&fixture.app, fixture.other_user_id, &fixture.admin_token).await;
    let configuration = &other["Configuration"];
    assert_eq!(
        configuration["MyMediaExcludes"].as_array().unwrap().len(),
        1
    );
    assert_eq!(configuration["EnableNextEpisodeAutoPlay"], false);
}

fn configuration_body(mutate: impl FnOnce(&mut UserConfiguration)) -> Value {
    let mut configuration = UserConfiguration::default();
    mutate(&mut configuration);
    serde_json::to_value(configuration).expect("configuration JSON")
}

async fn get_user(app: &axum::Router, id: Uuid, token: &str) -> Value {
    let response = request(
        app,
        "GET",
        &format!("/Users/{id}"),
        Some(token),
        Value::Null,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
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
            "User Configuration Tests",
            "1.0",
            "Test",
            format!("user-configuration-tests-{suffix}"),
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
