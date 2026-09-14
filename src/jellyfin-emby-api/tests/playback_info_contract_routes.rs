use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::ConnectionTrait;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby PlaybackInfo Tests\", DeviceId=\"emby-playback-info\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_playback_info_";

#[tokio::test]
async fn generated_get_playback_info_user_id_contract_is_protocol_local() {
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
    let database = jellyfin_data::connect(&temporary_database_config(database_name))
        .await
        .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let user = UserService::new(database.clone())
        .create_initial_administrator("emby-playback-info-user")
        .await
        .expect("user creation");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "Emby PlaybackInfo Tests",
            "1.0",
            "Test",
            "emby-playback-info",
        ))
        .await
        .expect("device session")
        .access_token;
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.name = Some("Emby PlaybackInfo item".to_owned());
    let item = BaseItemRepository::new(database.clone())
        .create(item)
        .await
        .expect("playback item");

    let state = AppState::new(
        database.clone(),
        "Emby PlaybackInfo Test Server".to_owned(),
        "http://127.0.0.1:18096".to_owned(),
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));
    let emby_route = format!("/emby/Items/{}/PlaybackInfo", item.id);

    assert_eq!(
        request(&app, &emby_route, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &emby_route, Some(&token)).await,
        StatusCode::BAD_REQUEST
    );
    for name in ["UserId", "userId", "userid"] {
        let route = format!("/emby/iTeMs/{}/pLaYbAcKiNfO?{name}={}", item.id, user.id);
        assert_eq!(
            request(&app, &route, Some(&token)).await,
            StatusCode::OK,
            "{route}"
        );
    }

    for route in [
        format!("/Items/{}/PlaybackInfo", item.id),
        format!("/api/Items/{}/PlaybackInfo", item.id),
    ] {
        assert_eq!(
            request(&app, &route, Some(&token)).await,
            StatusCode::OK,
            "{route}"
        );
    }

    assert_eq!(
        request_method(&app, Method::POST, &emby_route, None, None).await,
        StatusCode::UNAUTHORIZED,
        "authentication must precede the generated required body"
    );
    for body in [None, Some(""), Some("null"), Some("{")] {
        assert_eq!(
            request_method(&app, Method::POST, &emby_route, Some(&token), body).await,
            StatusCode::BAD_REQUEST,
            "Emby PlaybackInfo body {body:?}"
        );
    }
    assert_eq!(
        request_method(
            &app,
            Method::POST,
            &format!("/emby/iTeMs/{}/pLaYbAcKiNfO", item.id),
            Some(&token),
            Some("{}"),
        )
        .await,
        StatusCode::OK,
    );

    for route in [
        format!("/Items/{}/PlaybackInfo", item.id),
        format!("/api/Items/{}/PlaybackInfo", item.id),
    ] {
        assert_eq!(
            request_method(&app, Method::POST, &route, Some(&token), None).await,
            StatusCode::OK,
            "Jellyfin optional POST body at {route}"
        );
    }
    database.close().await.expect("database pool cleanup");
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> StatusCode {
    request_method(app, Method::GET, uri, token, None).await
}

async fn request_method(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> StatusCode {
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
                .body(body.map_or_else(Body::empty, |value| Body::from(value.to_owned())))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

fn temporary_database_config(database_name: &str) -> DatabaseConfig {
    let mut config = DatabaseConfig::default();
    let (prefix, _) = config
        .url
        .rsplit_once('/')
        .expect("database URL must include a database name");
    config.url = format!("{prefix}/{database_name}");
    config.max_connections = 8;
    config.min_connections = 1;
    config
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
