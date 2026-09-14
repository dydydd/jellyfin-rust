use std::path::{Path, PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use jellyfin_model::TranscodeReason;
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"emby-swift-compat\", DeviceId=\"emby-swift-compat\", Device=\"Test\", Version=\"1.0\"";
const LOGIN_AUTHORIZATION: &str = "MediaBrowser Client=\"emby-swift-compat\", DeviceId=\"emby-swift-login-compat\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_swift_compat_";
const MAX_RESPONSE_SIZE: usize = 4 * 1024 * 1024;

struct DumpedResponse {
    route: String,
    model: String,
    body: Value,
}

#[tokio::test]
async fn real_emby_responses_cover_generated_swift_bootstrap_contracts() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let dump_dir = std::env::var_os("JELLYFIN_EMBY_SWIFT_DUMP").map(PathBuf::from);
    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name, dump_dir).await }).await;

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

async fn exercise(database_name: &str, dump_dir: Option<PathBuf>) {
    let database = jellyfin_data::connect(&temporary_database_config(database_name))
        .await
        .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let users = UserService::new(database.clone());
    let user = users
        .create_initial_administrator("emby-swift-user")
        .await
        .expect("compatibility user");
    users
        .set_password_hash(
            user.id,
            DefaultAuthenticationProvider::new().password_hash("emby-swift-password"),
        )
        .await
        .expect("compatibility password");
    let user_token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "emby-swift-compat",
            "1.0",
            "Test",
            "emby-swift-compat",
        ))
        .await
        .expect("compatibility session")
        .access_token;
    let items = BaseItemRepository::new(database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.name = Some("Emby Swift Theme Owner".to_owned());
    movie.sort_name = movie.name.clone();
    movie.parent_id = Some(root.id);
    let movie = items.create(movie).await.expect("theme owner item");

    let state = AppState::new(
        database.clone(),
        "Emby Swift Compatibility Server".to_owned(),
        "http://127.0.0.1:18096".to_owned(),
    )
    .with_transcode_job(
        "emby-swift-transcode",
        "emby-swift-compat",
        "emby-swift-play-session",
        TranscodeReason::VIDEO_CODEC_NOT_SUPPORTED
            | TranscodeReason::AUDIO_IS_EXTERNAL
            | TranscodeReason::VIDEO_RANGE_TYPE_NOT_SUPPORTED
            | TranscodeReason::VIDEO_CODEC_TAG_NOT_SUPPORTED
            | TranscodeReason::STREAM_COUNT_EXCEEDS_LIMIT
            | TranscodeReason::VIDEO_ROTATION_NOT_SUPPORTED,
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));
    let user_id = user.id.simple();
    let mut responses = Vec::new();

    let login_route = "/emby/Users/AuthenticateByName";
    let mut authentication = response_json(
        request(
            &app,
            Method::POST,
            login_route,
            None,
            Some(LOGIN_AUTHORIZATION),
            Some(json!({
                "uSeRnAmE": "emby-swift-user",
                "pW": "emby-swift-password"
            })),
        )
        .await,
        login_route,
    )
    .await;
    assert!(authentication["AccessToken"].is_string());
    remove_secret_fields(&mut authentication);
    responses.push(DumpedResponse {
        route: login_route.to_owned(),
        model: "AuthenticationAuthenticationResult".to_owned(),
        body: authentication,
    });

    for (route, model) in [
        ("/emby/System/Info".to_owned(), "SystemInfo"),
        (format!("/emby/Users/{user_id}"), "UserDto"),
        (
            format!("/emby/Items?UserId={user_id}&Limit=1"),
            "QueryResultBaseItemDto",
        ),
        ("/emby/Sessions".to_owned(), "[SessionSessionInfo]"),
        (
            format!("/emby/DisplayPreferences/emby-swift?UserId={user_id}&Client=emby-swift"),
            "DisplayPreferences",
        ),
        (
            format!("/emby/Sync/Targets?UserId={user_id}"),
            "[SyncTarget]",
        ),
        ("/emby/Sync/Jobs".to_owned(), "QueryResultSyncJob"),
        (
            "/emby/Sync/JobItems?TargetId=emby-swift-compat".to_owned(),
            "QueryResultSyncJobItem",
        ),
        (
            "/emby/Sync/Items/Ready?TargetId=emby-swift-compat".to_owned(),
            "[SyncedItem]",
        ),
        (
            format!("/emby/Sync/Options?UserId={user_id}"),
            "SyncDialogOptions",
        ),
        (
            "/emby/Dlna/ProfileInfos".to_owned(),
            "[DlnaProfilesDlnaProfile]",
        ),
        (
            format!("/emby/Items/{}/ThemeSongs", movie.id),
            "ThemeMediaResult",
        ),
        (
            format!("/emby/Items/{}/ThemeVideos", movie.id),
            "ThemeMediaResult",
        ),
        (
            format!("/emby/Items/{}/ThemeMedia", movie.id),
            "AllThemeMediaResult",
        ),
    ] {
        let body = response_json(
            request(&app, Method::GET, &route, Some(&user_token), None, None).await,
            &route,
        )
        .await;
        responses.push(DumpedResponse {
            route,
            model: model.to_owned(),
            body,
        });
    }

    assert_eq!(responses.len(), 15);
    assert!(responses.iter().all(|response| !response.route.is_empty()));
    assert_eq!(responses[2].body["Id"], user_id.to_string());
    assert!(responses[3].body["Items"].is_array());
    assert_eq!(
        session_for_device(&responses[4].body, "emby-swift-compat")["TranscodingInfo"]["TranscodeReasons"],
        json!([
            "VideoCodecNotSupported",
            "ExternalAudioNotSupported",
            "VideoRangeNotSupported"
        ])
    );
    assert!(responses[5].body["CustomPrefs"].is_object());
    assert_eq!(
        responses[7].body,
        json!({"Items": [], "TotalRecordCount": 0})
    );
    assert_eq!(
        responses[8].body,
        json!({"Items": [], "TotalRecordCount": 0})
    );
    assert_eq!(responses[9].body, json!([]));
    for response in &responses[12..14] {
        assert!(
            response.body.get("OwnerId").is_none(),
            "Emby's Int64 OwnerId cannot contain a Jellyfin UUID: {}",
            response.route,
        );
    }
    for result in [
        &responses[14].body["ThemeSongsResult"],
        &responses[14].body["ThemeVideosResult"],
        &responses[14].body["SoundtrackSongsResult"],
    ] {
        assert!(result.get("OwnerId").is_none());
    }

    for route in [
        format!("/Items/{}/ThemeSongs", movie.id),
        format!("/api/Items/{}/ThemeSongs", movie.id),
    ] {
        let jellyfin = response_json(
            request(&app, Method::GET, &route, Some(&user_token), None, None).await,
            &route,
        )
        .await;
        assert_eq!(
            jellyfin["OwnerId"],
            movie.id.to_string(),
            "Emby's numeric OwnerId adaptation must not change Jellyfin {route}",
        );
    }

    let jellyfin_sessions = response_json(
        request(
            &app,
            Method::GET,
            "/Sessions",
            Some(&user_token),
            None,
            None,
        )
        .await,
        "/Sessions",
    )
    .await;
    assert_eq!(
        session_for_device(&jellyfin_sessions, "emby-swift-compat")["TranscodingInfo"]["TranscodeReasons"],
        json!([
            "VideoCodecNotSupported",
            "AudioIsExternal",
            "VideoRangeTypeNotSupported",
            "VideoCodecTagNotSupported",
            "StreamCountExceedsLimit",
            "VideoRotationNotSupported"
        ]),
        "the Emby adapter must not rewrite Jellyfin's root Sessions response"
    );

    if let Some(dump_dir) = dump_dir {
        write_dump(&dump_dir, &responses, &[&user_token]);
    }
    database.close().await.expect("database cleanup");
}

fn session_for_device<'a>(sessions: &'a Value, device_id: &str) -> &'a Value {
    sessions
        .as_array()
        .expect("Sessions response is an array")
        .iter()
        .find(|session| session["DeviceId"] == device_id)
        .unwrap_or_else(|| panic!("Sessions response has no device {device_id}"))
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    authorization: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri).header(
        header::AUTHORIZATION,
        authorization.map_or_else(
            || format!("{AUTHORIZATION}, Token=\"{}\"", token.unwrap_or_default()),
            ToOwned::to_owned,
        ),
    );
    let body = if let Some(value) = body {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&value).expect("request JSON"))
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("route response")
}

async fn response_json(response: axum::response::Response, route: &str) -> Value {
    assert_eq!(response.status(), StatusCode::OK, "{route}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("bounded response body"),
    )
    .unwrap_or_else(|error| panic!("{route} returned invalid JSON: {error}"))
}

fn remove_secret_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| {
                let key = key.to_ascii_lowercase();
                !key.contains("token")
                    && !matches!(key.as_str(), "password" | "pw" | "apikey" | "api_key")
            });
            fields.values_mut().for_each(remove_secret_fields);
        }
        Value::Array(values) => values.iter_mut().for_each(remove_secret_fields),
        _ => {}
    }
}

fn write_dump(directory: &Path, responses: &[DumpedResponse], secrets: &[&str]) {
    std::fs::create_dir_all(directory).expect("create Emby Swift dump directory");
    let cases = responses
        .iter()
        .map(|response| {
            json!({
                "route": response.route,
                "model": response.model,
                "body": response.body,
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec_pretty(&json!({"cases": cases}))
        .expect("serialize Emby Swift dump manifest");
    for secret in secrets {
        assert!(
            secret.is_empty()
                || !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
            "Emby Swift dump must not contain an access token"
        );
    }
    std::fs::write(directory.join("manifest.json"), bytes).expect("write Emby Swift dump manifest");
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
