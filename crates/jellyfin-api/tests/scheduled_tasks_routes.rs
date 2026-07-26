use std::{fs, path::PathBuf};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Scheduled Tasks Tests\", DeviceId=\"scheduled-tasks-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_scheduled_tasks_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn scheduled_tasks_routes_match_official_elevated_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/ScheduledTasks", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks",
                Some(&fixture.user_token),
                None
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks?isHidden=not-bool",
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let tasks = body_json(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks",
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    let tasks = tasks.as_array().expect("tasks");
    assert!(tasks.len() >= 4);
    let names = tasks
        .iter()
        .map(|task| task["Name"].as_str().expect("task name"))
        .collect::<Vec<_>>();
    assert_eq!(names, sorted(names.clone()));
    assert!(tasks.iter().all(|task| task["State"] == "Idle"));
    assert!(tasks.iter().all(|task| task["IsHidden"] == false));
    assert!(
        tasks
            .iter()
            .all(|task| task["Id"].as_str().unwrap().len() == 32)
    );
    let refresh = tasks
        .iter()
        .find(|task| task["Key"] == "RefreshLibrary")
        .expect("refresh library task");
    assert_eq!(refresh["Triggers"][0]["Type"], "IntervalTrigger");
    assert_eq!(refresh["Triggers"][0]["IntervalTicks"], 432_000_000_000_i64);
    let task_id = refresh["Id"].as_str().unwrap();

    let api_key_tasks = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/ScheduledTasks?api_key={}", fixture.api_key_token),
                None,
                None,
            )
            .await,
    )
    .await;
    assert_eq!(api_key_tasks.as_array().unwrap().len(), tasks.len());

    let hidden = body_json(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks?isHidden=true",
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert!(hidden.as_array().unwrap().is_empty());
    let enabled = body_json(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks?isEnabled=true",
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(enabled.as_array().unwrap().len(), tasks.len());

    let task = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/ScheduledTasks/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(task["Key"], "RefreshLibrary");

    assert_eq!(
        fixture
            .request(
                Method::POST,
                &format!("/ScheduledTasks/Running/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let running = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/ScheduledTasks/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(running["State"], "Running");

    assert_eq!(
        fixture
            .request(
                Method::POST,
                &format!("/ScheduledTasks/{task_id}/Triggers"),
                Some(&fixture.admin_token),
                Some(json!([
                    {
                        "Type": "DailyTrigger",
                        "TimeOfDayTicks": 7 * 36_000_000_000_i64,
                        "MaxRuntimeTicks": null
                    }
                ])),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let updated = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/ScheduledTasks/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(updated["Triggers"][0]["Type"], "DailyTrigger");
    assert_eq!(
        updated["Triggers"][0]["TimeOfDayTicks"],
        7 * 36_000_000_000_i64
    );

    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &format!("/ScheduledTasks/Running/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let stopped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/ScheduledTasks/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(stopped["State"], "Idle");

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks/does-not-exist",
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .raw_request(
                Method::POST,
                &format!("/ScheduledTasks/{task_id}/Triggers"),
                Some(&fixture.admin_token),
                b"{not-json".to_vec(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn refresh_library_task_scans_virtual_folder_media_for_playback() {
    let fixture = Fixture::new().await;
    let media_root = temporary_media_root();
    fs::create_dir_all(&media_root).expect("temporary media directory");
    let media_file = media_root.join("Playable Clip.mp4");
    let media_file_path = media_file.to_string_lossy().into_owned();
    let media_bytes = b"direct-play-test-payload";
    fs::write(&media_file, media_bytes).expect("temporary media file");

    let library_name = format!("LocalVideos{}", Uuid::new_v4().simple());
    let create_uri = format!(
        "/Library/VirtualFolders?name={library_name}&collectionType=homevideos&paths={}",
        media_root.to_string_lossy()
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &create_uri,
                Some(&fixture.admin_token),
                Some(json!({ "LibraryOptions": { "Enabled": true } })),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let views = body_json(
        fixture
            .request(Method::GET, "/UserViews", Some(&fixture.user_token), None)
            .await,
    )
    .await;
    let view_id = views["Items"]
        .as_array()
        .expect("user views")
        .iter()
        .find(|view| view["Name"] == library_name)
        .and_then(|view| view["Id"].as_str())
        .expect("scanned library view")
        .to_owned();

    let task_id = refresh_library_task_id(&fixture).await;
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &format!("/ScheduledTasks/Running/{task_id}"),
                Some(&fixture.admin_token),
                None,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let items = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Items?parentId={view_id}&includeItemTypes=Video"),
                Some(&fixture.user_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(items["TotalRecordCount"], 1);
    let item = &items["Items"][0];
    assert_eq!(item["Name"], "Playable Clip");
    assert_eq!(item["Type"], "Video");
    assert_eq!(item["MediaType"], "Video");
    assert_eq!(item["Path"], media_file_path);
    let item_id = item["Id"].as_str().expect("scanned item id");

    let playback = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Items/{item_id}/PlaybackInfo"),
                Some(&fixture.user_token),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(playback["MediaSources"][0]["Path"], media_file_path);
    assert_eq!(playback["MediaSources"][0]["Container"], "mp4");
    assert_eq!(
        playback["MediaSources"][0]["MediaStreams"][0]["Type"],
        1
    );
    assert_eq!(
        playback["MediaSources"][0]["MediaStreams"][0]["Codec"],
        "h264"
    );
    assert_eq!(playback["MediaSources"][0]["MediaStreams"][0]["Index"], 0);

    let response = fixture
        .request(
            Method::GET,
            &format!("/Videos/{item_id}/stream.mp4"),
            Some(&fixture.user_token),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .as_ref(),
        media_bytes
    );

    fs::remove_dir_all(&media_root).expect("temporary media cleanup");
    fixture.cleanup().await;
}

fn sorted(mut values: Vec<&str>) -> Vec<&str> {
    values.sort_unstable();
    values
}

async fn refresh_library_task_id(fixture: &Fixture) -> String {
    let tasks = body_json(
        fixture
            .request(
                Method::GET,
                "/ScheduledTasks",
                Some(&fixture.admin_token),
                None,
            )
            .await,
    )
    .await;
    tasks
        .as_array()
        .expect("tasks")
        .iter()
        .find(|task| task["Key"] == "RefreshLibrary")
        .and_then(|task| task["Id"].as_str())
        .expect("RefreshLibrary task")
        .to_owned()
}

fn temporary_media_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "jellyfin-rust-scheduled-task-scan-{}",
        Uuid::new_v4().simple()
    ))
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        administrator.close().await.unwrap();

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
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("scheduled-tasks-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("scheduled-tasks-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("scheduled-tasks-key-{suffix}"))
            .await
            .unwrap()
            .access_token;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Scheduled Tasks Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            admin_token,
            user_token,
            api_key_token,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let body = body.map(|body| serde_json::to_vec(&body).unwrap());
        self.raw_request(method, uri, token, body.unwrap_or_default())
            .await
    }

    async fn raw_request(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        if !body.is_empty() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Scheduled Tasks Tests",
            "1.0",
            "Test",
            format!("scheduled-tasks-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
