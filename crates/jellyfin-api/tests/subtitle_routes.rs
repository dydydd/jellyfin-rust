use std::path::PathBuf;

use axum::{
    body::{Body, Bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamFilter, MediaStreamService, UserService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    entities::{base_item, user},
};
use jellyfin_model::{MediaStream, MediaStreamType, UserPolicy};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Subtitle Tests\", Device=\"Test\", DeviceId=\"subtitle-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_subtitle_routes_";

#[tokio::test]
async fn delete_subtitle_route_requires_elevation_and_deletes_only_target_subtitle_stream() {
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
        exercise_delete_subtitle_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_delete_subtitle_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = Fixture::subtitle_route(fixture.item_id, 2);

    assert_eq!(
        fixture.send(Method::DELETE, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::subtitle_route(Uuid::new_v4(), 2),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("media streams after delete");
    let remaining = streams
        .iter()
        .map(|stream| (stream.index, stream.stream_type))
        .collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec![
            (0, MediaStreamType::Video),
            (1, MediaStreamType::Audio),
            (3, MediaStreamType::Subtitle),
        ]
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn remote_subtitle_routes_match_management_policy_and_empty_provider_contract() {
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
        exercise_remote_subtitle_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn upload_subtitle_route_decodes_base64_file_and_persists_external_stream() {
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
        exercise_upload_subtitle_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_remote_subtitle_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let search_route = Fixture::search_route(fixture.item_id, "eng");

    assert_eq!(
        fixture
            .send(Method::GET, &search_route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::GET, &search_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let search = fixture
        .send(Method::GET, &search_route, Some(&fixture.manager_token))
        .await;
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(body_json(search).await, Value::Array(Vec::new()));

    let missing_search = fixture
        .send(
            Method::GET,
            &Fixture::search_route(Uuid::new_v4(), "eng"),
            Some(&fixture.manager_token),
        )
        .await;
    assert_eq!(missing_search.status(), StatusCode::NOT_FOUND);

    let non_video_search = fixture
        .send(
            Method::GET,
            &Fixture::search_route(fixture.folder_id, "eng"),
            Some(&fixture.manager_token),
        )
        .await;
    assert_eq!(non_video_search.status(), StatusCode::NOT_FOUND);

    let download_route = Fixture::download_route(fixture.item_id, "provider-subtitle-id");
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(Method::POST, &download_route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(
                Method::POST,
                &Fixture::download_route(Uuid::new_v4(), "provider-subtitle-id"),
                Some(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let provider_route = "/Providers/Subtitles/Subtitles/provider-subtitle-id";
    assert_eq!(
        fixture
            .send(Method::GET, provider_route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(Method::GET, provider_route, Some(&fixture.manager_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

async fn exercise_upload_subtitle_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = Fixture::upload_route(fixture.item_id);
    let body = json!({
        "Language": "Eng",
        "Format": "SRT",
        "IsForced": true,
        "IsHearingImpaired": false,
        "Data": "MSAwMDowMDowMSwwMDAgLS0+IDAwOjAwOjAyLDAwMApIZWxsbyBmcm9tIHVwbG9hZAo="
    });

    assert_eq!(
        fixture
            .send_json(Method::POST, &route, None, &body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send_json(Method::POST, &route, Some(&fixture.user_token), &body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &route,
                Some(&fixture.manager_token),
                &json!({
                    "Language": "eng",
                    "Format": "srt",
                    "IsForced": false,
                    "IsHearingImpaired": false,
                    "Data": "not-base64"
                }),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &Fixture::upload_route(Uuid::new_v4()),
                Some(&fixture.manager_token),
                &body,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send_json(
                Method::POST,
                &Fixture::upload_route(fixture.folder_id),
                Some(&fixture.manager_token),
                &body,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send_json(Method::POST, &route, Some(&fixture.manager_token), &body)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("streams after upload");
    let uploaded = streams
        .iter()
        .find(|stream| stream.index == 4)
        .expect("uploaded subtitle stream");
    assert_eq!(uploaded.stream_type, MediaStreamType::Subtitle);
    assert_eq!(uploaded.codec.as_deref(), Some("srt"));
    assert_eq!(uploaded.language.as_deref(), Some("eng"));
    assert!(uploaded.is_external);
    assert!(uploaded.is_forced);
    assert!(!uploaded.is_hearing_impaired);
    let path = uploaded.path.as_deref().expect("uploaded subtitle path");
    assert!(path.contains("/subtitles/"));
    assert!(path.ends_with("/4.eng.srt"));
    let bytes = tokio::fs::read(path).await.expect("uploaded subtitle file");
    assert_eq!(
        Bytes::from(bytes),
        Bytes::from_static(b"1 00:00:01,000 --> 00:00:02,000\nHello from upload\n")
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    manager_id: Uuid,
    manager_token: String,
    item_id: Uuid,
    folder_id: Uuid,
    storage_root: PathBuf,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
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
        let storage_root = std::env::temp_dir().join(format!("jellyfin-subtitle-routes-{suffix}"));
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("subtitle-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("subtitle-user-{suffix}"))
            .await
            .expect("user creation");
        let manager = users
            .create(&format!("subtitle-manager-{suffix}"))
            .await
            .expect("manager creation");
        users
            .update_policy(manager.id, &subtitle_manager_policy())
            .await
            .expect("subtitle manager policy");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = devices
            .create_session(NewDevice::new(
                admin.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-admin-{suffix}"),
            ))
            .await
            .expect("admin session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;
        let manager_token = devices
            .create_session(NewDevice::new(
                manager.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-manager-{suffix}"),
            ))
            .await
            .expect("manager session")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Subtitle Movie {suffix}"));
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Subtitle Movie {suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie item creation");
        let mut folder = NewBaseItem::new(Uuid::new_v4(), "Folder");
        folder.name = Some(format!("Subtitle Folder {suffix}"));
        folder.is_folder = true;
        let folder = BaseItemRepository::new(database.clone())
            .create(folder)
            .await
            .expect("folder item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item.id,
                &[
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some("h264".to_owned()),
                        path: Some(format!("/media/Subtitle Movie {suffix}.mkv")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        language: Some("eng".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.eng.srt")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 3,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("ass".to_owned()),
                        language: Some("jpn".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.jpn.ass")),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("media stream creation");

        let app_state = AppState::new(
            database.clone(),
            "Subtitle Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_storage_paths(
            storage_root.join("programdata"),
            storage_root.join("web"),
            storage_root.join("cache").join("images"),
            storage_root.join("cache"),
            storage_root.join("metadata"),
        );
        let app = jellyfin_api::router(app_state);
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            manager_id: manager.id,
            manager_token,
            item_id: item.id,
            folder_id: folder.id,
            storage_root,
        }
    }

    fn subtitle_route(item_id: Uuid, index: i32) -> String {
        format!("/Videos/{item_id}/Subtitles/{index}")
    }

    fn search_route(item_id: Uuid, language: &str) -> String {
        format!("/Items/{item_id}/RemoteSearch/Subtitles/{language}?isPerfectMatch=true")
    }

    fn download_route(item_id: Uuid, subtitle_id: &str) -> String {
        format!("/Items/{item_id}/RemoteSearch/Subtitles/{subtitle_id}")
    }

    fn upload_route(item_id: Uuid) -> String {
        format!("/Videos/{item_id}/Subtitles")
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn send_json(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: &Value,
    ) -> axum::response::Response {
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
        self.app
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(body).expect("request JSON")))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.is_in([self.item_id, self.folder_id]))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id, self.manager_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        let _ = tokio::fs::remove_dir_all(&self.storage_root).await;
        self.database.close().await.unwrap();
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn subtitle_manager_policy() -> UserPolicy {
    UserPolicy {
        enable_subtitle_management: true,
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
