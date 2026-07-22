use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewUserData, UserDataRepository,
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_generic_user_data_";
const AUTHORIZATION: &str = "MediaBrowser Client=\"UserData Tests\", Device=\"PostgreSQL\", DeviceId=\"user-data\", Version=\"1.0\"";

#[tokio::test]
async fn generic_user_data_routes_match_official_contract() {
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
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    let fixture = Fixture::new(database.clone()).await;

    assert_get_defaults_and_key_resolution(&fixture).await;
    assert_updates_and_field_semantics(&fixture).await;
    assert_authentication_and_preference_access(&fixture).await;
    assert_item_validation_and_visibility(&fixture).await;
    assert_invalid_bodies_are_atomic(&fixture).await;
    assert_concurrent_updates_preserve_unrelated_fields(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool must close");
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    no_preferences_id: Uuid,
    no_preferences_token: String,
    blocked_id: Uuid,
    blocked_token: String,
    api_key: String,
    root_id: Uuid,
    allowed_item_id: Uuid,
    default_item_id: Uuid,
    retired_item_id: Uuid,
    hidden_item_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator("generic-user-data-admin")
            .await
            .expect("administrator creation");
        let user = users
            .create("generic-user-data-user")
            .await
            .expect("ordinary user creation");
        let no_preferences = users
            .create("generic-user-data-no-preferences")
            .await
            .expect("preference-restricted user creation");
        let blocked = users
            .create("generic-user-data-blocked")
            .await
            .expect("parental-schedule user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root creation");
        let allowed_folder =
            create_item(&items, "CollectionFolder", Some(root.id), "Allowed").await;
        let hidden_folder = create_item(&items, "CollectionFolder", Some(root.id), "Hidden").await;
        let mut allowed = NewBaseItem::new(Uuid::new_v4(), "Movie");
        allowed.parent_id = Some(allowed_folder.id);
        allowed.name = Some("Runtime movie".to_owned());
        allowed.presentation_unique_key = Some("generic-presentation-key".to_owned());
        allowed.runtime_ticks = Some(1_000);
        let allowed_item = items.create(allowed).await.expect("allowed item creation");
        let default_item =
            create_item(&items, "Movie", Some(allowed_folder.id), "Default item").await;
        let retired_item =
            create_item(&items, "Movie", Some(allowed_folder.id), "Retired item").await;
        let hidden_item = create_item(&items, "Movie", Some(hidden_folder.id), "Hidden item").await;

        let mut limited = policy();
        limited.enable_all_folders = false;
        limited.enabled_folders = vec![allowed_folder.id];
        users
            .update_policy(user.id, &limited)
            .await
            .expect("limited user policy");
        let mut no_preference_policy = policy();
        no_preference_policy.enable_user_preference_access = false;
        users
            .update_policy(no_preferences.id, &no_preference_policy)
            .await
            .expect("preference-restricted policy");
        users
            .update_policy(blocked.id, &blocked_schedule_policy(false))
            .await
            .expect("blocked schedule policy");
        users
            .update_policy(administrator.id, &blocked_schedule_policy(true))
            .await
            .expect("administrator schedule policy");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session_token(&devices, administrator.id, "admin").await;
        let user_token = session_token(&devices, user.id, "user").await;
        let no_preferences_token =
            session_token(&devices, no_preferences.id, "no-preferences").await;
        let blocked_token = session_token(&devices, blocked.id, "blocked").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create("generic-user-data-api-key")
            .await
            .expect("API key creation")
            .access_token;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Generic UserData Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            administrator_token,
            user_id: user.id,
            user_token,
            no_preferences_id: no_preferences.id,
            no_preferences_token,
            blocked_id: blocked.id,
            blocked_token,
            api_key,
            root_id: root.id,
            allowed_item_id: allowed_item.id,
            default_item_id: default_item.id,
            retired_item_id: retired_item.id,
            hidden_item_id: hidden_item.id,
        }
    }
}

async fn assert_get_defaults_and_key_resolution(fixture: &Fixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let modern = modern_route(fixture.default_item_id);
    let response = request(&fixture.app, "GET", &modern, &fixture.user_token, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_default_body(&body, fixture.default_item_id);
    assert_eq!(
        repository
            .get_for_item(fixture.default_item_id, fixture.user_id)
            .await
            .expect("default rows lookup")
            .len(),
        0,
        "GET must not insert a default row"
    );

    let mut retired = NewUserData::new(fixture.allowed_item_id, fixture.user_id, "a-retired-key");
    retired.rating = Some(2.0);
    repository.upsert(retired).await.expect("retired seed");
    let mut current = NewUserData::new(
        fixture.allowed_item_id,
        fixture.user_id,
        "generic-presentation-key",
    );
    current.rating = Some(8.0);
    current.likes = Some(true);
    current.playback_position_ticks = 250;
    current.play_count = 3;
    current.is_favorite = true;
    current.played = true;
    current.audio_stream_index = Some(2);
    current.subtitle_stream_index = Some(3);
    repository.upsert(current).await.expect("current seed");

    for route in [
        modern_route(fixture.allowed_item_id),
        legacy_route(fixture.user_id, fixture.allowed_item_id),
    ] {
        let response = request(&fixture.app, "GET", &route, &fixture.user_token, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["Key"], "generic-presentation-key");
        assert_eq!(body["Rating"], 8.0);
        assert_eq!(body["Likes"], true);
        assert_eq!(body["PlayedPercentage"], 25.0);
        assert_eq!(body["PlayCount"], 3);
    }

    let mut lexical_later = NewUserData::new(fixture.retired_item_id, fixture.user_id, "z-retired");
    lexical_later.rating = Some(9.0);
    repository
        .upsert(lexical_later)
        .await
        .expect("later retired seed");
    let mut lexical_first = NewUserData::new(fixture.retired_item_id, fixture.user_id, "a-retired");
    lexical_first.rating = Some(4.0);
    repository
        .upsert(lexical_first)
        .await
        .expect("first retired seed");
    let response = request(
        &fixture.app,
        "GET",
        &modern_route(fixture.retired_item_id),
        &fixture.user_token,
        None,
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["Key"], "a-retired");
    assert_eq!(body["Rating"], 4.0);
    assert_eq!(body["Likes"], false, "Likes derives from a retired rating");
}

#[allow(
    clippy::too_many_lines,
    reason = "the assertions follow the official field-application order as one stateful scenario"
)]
async fn assert_updates_and_field_semantics(fixture: &Fixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let legacy = legacy_route(fixture.user_id, fixture.default_item_id);
    let response = request(
        &fixture.app,
        "POST",
        &legacy,
        &fixture.user_token,
        Some("{}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_default_body(&body_json(response).await, fixture.default_item_id);
    assert_eq!(
        repository
            .get_for_item(fixture.default_item_id, fixture.user_id)
            .await
            .expect("inserted default lookup")
            .len(),
        1,
        "empty POST must persist a default row"
    );

    let update = json!({
        "PlaybackPositionTicks": 500,
        "PlayCount": 7,
        "IsFavorite": true,
        "Likes": false,
        "Played": true,
        "LastPlayedDate": "2026-07-22T09:10:11Z",
        "Rating": 7.0,
        "PlayedPercentage": 99.0,
        "UnplayedItemCount": 99,
        "Key": "body-key-must-be-ignored",
        "ItemId": Uuid::new_v4().simple().to_string()
    });
    let response = request(
        &fixture.app,
        "POST",
        &modern_route(fixture.allowed_item_id),
        &fixture.user_token,
        Some(&update.to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["Key"], "generic-presentation-key");
    assert_eq!(body["ItemId"], fixture.allowed_item_id.simple().to_string());
    assert_eq!(body["PlaybackPositionTicks"], 500);
    assert_eq!(body["PlayedPercentage"], 50.0);
    assert_eq!(body["PlayCount"], 7);
    assert_eq!(body["IsFavorite"], true);
    assert_eq!(body["Played"], true);
    assert_eq!(body["LastPlayedDate"], "2026-07-22T09:10:11Z");
    assert_eq!(body["Rating"], 7.0, "Rating must win over Likes");
    assert_eq!(body["Likes"], true, "Likes must derive from final Rating");
    assert!(body.get("UnplayedItemCount").is_none());

    let response = request(
        &fixture.app,
        "POST",
        &modern_route(fixture.allowed_item_id),
        &fixture.user_token,
        Some(r#"{"rating":null,"likes":null,"playCount":8}"#),
    )
    .await;
    let body = body_json(response).await;
    assert_eq!(body["Rating"], 7.0);
    assert_eq!(body["Likes"], true);
    assert_eq!(body["PlayCount"], 8);
    assert_eq!(body["PlaybackPositionTicks"], 500);

    for (payload, rating, likes) in [
        (json!({ "Likes": true }), 10.0, true),
        (json!({ "Likes": false }), 1.0, false),
        (json!({ "Rating": 6.5 }), 6.5, true),
        (json!({ "Rating": 6.49 }), 6.49, false),
    ] {
        let response = request(
            &fixture.app,
            "POST",
            &modern_route(fixture.allowed_item_id),
            &fixture.user_token,
            Some(&payload.to_string()),
        )
        .await;
        let body = body_json(response).await;
        assert_eq!(body["Rating"], rating);
        assert_eq!(body["Likes"], likes);
    }

    let response = request(
        &fixture.app,
        "POST",
        &modern_route(fixture.retired_item_id),
        &fixture.user_token,
        Some(r#"{"IsFavorite":true}"#),
    )
    .await;
    assert_eq!(body_json(response).await["Key"], "a-retired");
    assert_eq!(
        repository
            .get_for_item(fixture.retired_item_id, fixture.user_id)
            .await
            .expect("retired rows after update")
            .len(),
        2,
        "retired fallback must be reused"
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the assertions cover the official modern and legacy authorization matrix"
)]
async fn assert_authentication_and_preference_access(fixture: &Fixture) {
    let modern = modern_route(fixture.allowed_item_id);
    for (method, body) in [("GET", None), ("POST", Some("{}"))] {
        assert_eq!(
            request_without_header(&fixture.app, method, &modern, body)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    for route in [
        format!("{modern}?userId={}", fixture.no_preferences_id),
        format!("{modern}?userId={}", Uuid::new_v4()),
    ] {
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request(&fixture.app, method, &route, &fixture.user_token, body)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }

    let no_preferences_modern = format!("{modern}?userId={}", fixture.no_preferences_id);
    let no_preferences_legacy = legacy_route(fixture.no_preferences_id, fixture.allowed_item_id);
    for route in [&no_preferences_modern, &no_preferences_legacy] {
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request(
                    &fixture.app,
                    method,
                    route,
                    &fixture.no_preferences_token,
                    body,
                )
                .await
                .status(),
                StatusCode::FORBIDDEN,
                "ordinary self access requires EnableUserPreferenceAccess"
            );
        }
    }

    for route in [&no_preferences_modern, &no_preferences_legacy] {
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request(
                    &fixture.app,
                    method,
                    route,
                    &fixture.administrator_token,
                    body,
                )
                .await
                .status(),
                StatusCode::OK
            );
        }
    }
    for route in [
        format!(
            "{modern}?userId={}&api_key={}",
            fixture.no_preferences_id, fixture.api_key
        ),
        format!("{no_preferences_legacy}?api_key={}", fixture.api_key),
    ] {
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request_without_header(&fixture.app, method, &route, body)
                    .await
                    .status(),
                StatusCode::OK
            );
        }
    }

    let blocked = format!("{modern}?userId={}", fixture.blocked_id);
    for (method, body) in [("GET", None), ("POST", Some("{}"))] {
        assert_eq!(
            request(&fixture.app, method, &blocked, &fixture.blocked_token, body)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    for route in [
        format!("{modern}?api_key={}", fixture.api_key),
        format!(
            "{modern}?userId={}&api_key={}",
            Uuid::nil(),
            fixture.api_key
        ),
    ] {
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request_without_header(&fixture.app, method, &route, body)
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
    }

    let legacy_nil = legacy_route(Uuid::nil(), fixture.allowed_item_id);
    assert_eq!(
        request(&fixture.app, "GET", &legacy_nil, &fixture.user_token, None)
            .await
            .status(),
        StatusCode::OK
    );
    let legacy_nil_api_key = format!("{legacy_nil}?api_key={}", fixture.api_key);
    for (method, body) in [("GET", None), ("POST", Some("{}"))] {
        assert_eq!(
            request_without_header(&fixture.app, method, &legacy_nil_api_key, body)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

async fn assert_item_validation_and_visibility(fixture: &Fixture) {
    assert_ne!(fixture.root_id, Uuid::nil());
    for item_id in [Uuid::nil(), Uuid::new_v4(), fixture.hidden_item_id] {
        let route = modern_route(item_id);
        for (method, body) in [("GET", None), ("POST", Some("{}"))] {
            assert_eq!(
                request(&fixture.app, method, &route, &fixture.user_token, body)
                    .await
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    for method in ["GET", "POST"] {
        let body = (method == "POST").then_some("{}");
        assert_eq!(
            request(
                &fixture.app,
                method,
                &modern_route(fixture.allowed_item_id),
                &fixture.user_token,
                body,
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
}

async fn assert_invalid_bodies_are_atomic(fixture: &Fixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let route = modern_route(fixture.allowed_item_id);
    let before = repository
        .get(
            fixture.allowed_item_id,
            fixture.user_id,
            "generic-presentation-key",
        )
        .await
        .expect("before-invalid lookup")
        .expect("before-invalid row");
    for payload in [
        r#"{"Rating":-0.1,"PlaybackPositionTicks":999}"#,
        r#"{"Rating":10.1}"#,
        r#"{"PlaybackPositionTicks":-1}"#,
        r#"{"PlayCount":-1}"#,
        r#"{"LastPlayedDate":"not-a-date"}"#,
        "{",
        "",
    ] {
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                &route,
                &fixture.user_token,
                Some(payload),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "payload must be rejected: {payload:?}"
        );
    }
    for content_type in [None, Some("text/plain")] {
        assert_eq!(
            request_with_content_type(
                &fixture.app,
                "POST",
                &route,
                &fixture.user_token,
                "{}",
                content_type,
            )
            .await
            .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    let after = repository
        .get(
            fixture.allowed_item_id,
            fixture.user_id,
            "generic-presentation-key",
        )
        .await
        .expect("after-invalid lookup")
        .expect("after-invalid row");
    assert_eq!(
        after, before,
        "validation must happen before the atomic write"
    );
}

async fn assert_concurrent_updates_preserve_unrelated_fields(fixture: &Fixture) {
    let repository = UserDataRepository::new(fixture.database.clone());
    let item_id = fixture.allowed_item_id;
    let key = "generic-presentation-key";
    let keys = vec![key.to_owned(), item_id.to_string()];
    let mut seed = NewUserData::new(item_id, fixture.user_id, key);
    seed.rating = Some(8.0);
    seed.likes = Some(false);
    seed.playback_position_ticks = 321;
    seed.play_count = 2;
    seed.audio_stream_index = Some(4);
    seed.subtitle_stream_index = Some(5);
    seed.retention_date = Some(Utc::now() + Duration::days(10));
    repository.upsert(seed).await.expect("concurrency seed");

    let route = modern_route(item_id);
    let route_update = request(
        &fixture.app,
        "POST",
        &route,
        &fixture.user_token,
        Some(r#"{"PlayCount":9}"#),
    );
    let favorite_update = repository.set_favorite(item_id, fixture.user_id, &keys, true);
    let (response, favorite) = tokio::join!(route_update, favorite_update);
    assert_eq!(response.status(), StatusCode::OK);
    favorite.expect("concurrent favorite update");

    let persisted = repository
        .get(item_id, fixture.user_id, key)
        .await
        .expect("concurrent row lookup")
        .expect("concurrent row");
    assert_eq!(persisted.play_count, 9);
    assert!(persisted.is_favorite);
    assert_eq!(persisted.rating, Some(8.0));
    assert_eq!(
        persisted.likes,
        Some(true),
        "unrelated generic patches normalize historical Likes from Rating"
    );
    assert_eq!(persisted.playback_position_ticks, 321);
    assert_eq!(persisted.audio_stream_index, Some(4));
    assert_eq!(persisted.subtitle_stream_index, Some(5));
    assert!(persisted.retention_date.is_some());
}

fn assert_default_body(body: &Value, item_id: Uuid) {
    assert_eq!(body["ItemId"], item_id.simple().to_string());
    assert_eq!(body["Key"], item_id.to_string());
    assert_eq!(body["PlaybackPositionTicks"], 0);
    assert_eq!(body["PlayCount"], 0);
    assert_eq!(body["IsFavorite"], false);
    assert_eq!(body["Played"], false);
    for absent in [
        "Rating",
        "PlayedPercentage",
        "UnplayedItemCount",
        "Likes",
        "LastPlayedDate",
    ] {
        assert!(body.get(absent).is_none(), "{absent} must be absent");
    }
}

async fn create_item(
    items: &BaseItemRepository,
    item_type: &str,
    parent_id: Option<Uuid>,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.parent_id = parent_id;
    item.name = Some(name.to_owned());
    items.create(item).await.expect("base item creation")
}

async fn session_token(devices: &DeviceRepository, user_id: Uuid, label: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "UserData Tests",
            "1.0",
            "PostgreSQL",
            format!("user-data-{label}"),
        ))
        .await
        .expect("device session creation")
        .access_token
}

fn policy() -> UserPolicy {
    UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

fn blocked_schedule_policy(is_administrator: bool) -> UserPolicy {
    UserPolicy {
        is_administrator,
        access_schedules: vec![AccessSchedule {
            day_of_week: DynamicDayOfWeek::Everyday,
            start_hour: 18.0,
            end_hour: 6.0,
        }],
        ..policy()
    }
}

fn modern_route(item_id: Uuid) -> String {
    format!("/UserItems/{item_id}/UserData")
}

fn legacy_route(user_id: Uuid, item_id: Uuid) -> String {
    format!("/Users/{user_id}/Items/{item_id}/UserData")
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri).header(
        header::AUTHORIZATION,
        format!("{AUTHORIZATION}, Token=\"{token}\""),
    );
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            builder
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn request_without_header(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            builder
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn request_with_content_type(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: &str,
    content_type: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri).header(
        header::AUTHORIZATION,
        format!("{AUTHORIZATION}, Token=\"{token}\""),
    );
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("route response")
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .expect("content type")
            .starts_with("application/json")
    );
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
