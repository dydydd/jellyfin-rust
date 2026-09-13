use std::path::PathBuf;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamFilter, MediaStreamService, UserService};
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
};
use jellyfin_model::{MediaStream, MediaStreamType};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Subtitle Delete Tests\", DeviceId=\"emby-subtitle-delete-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_subtitle_delete_";

#[tokio::test]
async fn generated_subtitle_delete_alias_removes_external_files_and_stays_emby_only() {
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
        exercise_subtitle_delete_routes(&task_database_name).await;
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

async fn exercise_subtitle_delete_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    assert_authorization_precedes_binding(&fixture).await;
    assert_item_lookup_precedes_legacy_query_validation(&fixture).await;
    assert_signed_index_and_query_contract(&fixture).await;
    assert_external_files_and_rows_are_deleted(&fixture).await;
    assert_embedded_container_is_never_unlinked(&fixture).await;
    assert_jellyfin_root_is_isolated(&fixture, database.clone()).await;

    tokio::fs::remove_dir_all(&fixture.storage_root)
        .await
        .expect("subtitle fixture directory cleanup");
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    item_id: Uuid,
    admin_token: String,
    user_token: String,
    api_key_token: String,
    storage_root: PathBuf,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let storage_root = std::env::temp_dir().join(format!("emby-subtitle-delete-{suffix}"));
        tokio::fs::create_dir_all(&storage_root)
            .await
            .expect("subtitle fixture directory");
        for (name, bytes) in [
            ("primary.eng.srt", b"English subtitle bytes\n".as_slice()),
            ("primary.jpn.ass", b"Japanese subtitle bytes\n".as_slice()),
            ("primary.mkv", b"video container sentinel\n".as_slice()),
        ] {
            tokio::fs::write(storage_root.join(name), bytes)
                .await
                .expect("subtitle fixture file");
        }

        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-subtitle-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-subtitle-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-subtitle-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Emby Subtitle Movie {suffix}"));
        item.media_type = Some("Video".to_owned());
        item.path = Some(
            storage_root
                .join("primary.mkv")
                .to_string_lossy()
                .into_owned(),
        );
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item.id,
                vec![
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        is_external: true,
                        path: Some(
                            storage_root
                                .join("primary.eng.srt")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 3,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("ass".to_owned()),
                        is_external: true,
                        path: Some(
                            storage_root
                                .join("primary.jpn.ass")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 4,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("subrip".to_owned()),
                        is_external: false,
                        path: Some(
                            storage_root
                                .join("primary.mkv")
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("subtitle stream creation");

        Self {
            app: jellyfin_emby_api::router(AppState::new(
                database.clone(),
                "Emby Subtitle Delete Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            item_id: item.id,
            admin_token,
            user_token,
            api_key_token,
            storage_root,
        }
    }

    fn route(&self, index: impl std::fmt::Display, query: &str) -> String {
        format!(
            "/emby/Videos/{}/Subtitles/{index}/Delete{query}",
            self.item_id
        )
    }
}

async fn assert_authorization_precedes_binding(fixture: &Fixture) {
    let malformed = "/emby/Videos/not-a-uuid/Subtitles/2147483648/Delete";
    assert_eq!(
        request(&fixture.app, malformed, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&fixture.app, malformed, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST,
        "the generated Emby operation allows an ordinary authenticated user",
    );
    assert_eq!(
        request(&fixture.app, malformed, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn assert_item_lookup_precedes_legacy_query_validation(fixture: &Fixture) {
    let missing = format!("/emby/Videos/{}/Subtitles/2/Delete", Uuid::new_v4());
    assert_eq!(
        request(&fixture.app, &missing, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "a missing target must remain 404 even when MediaSourceId is absent",
    );
    assert_eq!(
        request(
            &fixture.app,
            &fixture.route(2, ""),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &fixture.app,
            &fixture.route(2, "?MediaSourceId=%20%20"),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn assert_signed_index_and_query_contract(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            &fixture.route(-1, "?MediaSourceId=legacy-source"),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::OK,
        "negative signed Int32 values must bind without deleting another stream",
    );
    assert_eq!(
        request(
            &fixture.app,
            &fixture.route(2_147_483_648_i64, "?MediaSourceId=legacy-source"),
            Some(&fixture.admin_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
        "values outside signed Int32 must fail binding",
    );
}

async fn assert_external_files_and_rows_are_deleted(fixture: &Fixture) {
    assert_eq!(
        request(
            &fixture.app,
            &fixture.route(2, "?MediaSourceId=legacy-source"),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(!fixture.storage_root.join("primary.eng.srt").exists());

    let mixed_case = format!(
        "/emby/vIdEoS/{}/sUbTiTlEs/3/dElEtE?mediaSourceId=legacy-source",
        fixture.item_id
    );
    assert_eq!(
        request(&fixture.app, &mixed_case, Some(&fixture.api_key_token))
            .await
            .status(),
        StatusCode::OK,
        "static path segments and SDK camelCase query names must bind case-insensitively",
    );
    assert!(!fixture.storage_root.join("primary.jpn.ass").exists());

    let remaining = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("remaining media streams after external subtitle deletes");
    assert_eq!(
        remaining
            .iter()
            .map(|stream| (stream.index, stream.stream_type))
            .collect::<Vec<_>>(),
        vec![(4, MediaStreamType::Subtitle)],
    );
}

async fn assert_embedded_container_is_never_unlinked(fixture: &Fixture) {
    let lowercase = format!(
        "/emby/videos/{}/subtitles/4?mediasourceid=legacy-source",
        fixture.item_id
    );
    assert_eq!(
        request_method(
            &fixture.app,
            Method::DELETE,
            &lowercase,
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(
        fixture.storage_root.join("primary.mkv").exists(),
        "an embedded subtitle path may be the video container and must never be unlinked",
    );
    let remaining = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("remaining media streams after embedded subtitle delete");
    assert!(remaining.is_empty(), "the selected stream row is removed");
}

async fn assert_jellyfin_root_is_isolated(fixture: &Fixture, database: DatabaseConnection) {
    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Subtitle Delete Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let route = format!(
        "/Videos/{}/Subtitles/2/Delete?MediaSourceId=legacy-source",
        fixture.item_id
    );
    assert_eq!(
        request(&jellyfin, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "the generated Emby alias must not be registered on Jellyfin's root API",
    );
    let jellyfin_delete = format!(
        "/Videos/{}/Subtitles/2?MediaSourceId=legacy-source",
        fixture.item_id
    );
    assert_eq!(
        request_method(
            &jellyfin,
            Method::DELETE,
            &jellyfin_delete,
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "the unprefixed Jellyfin DELETE must retain RequiresElevation",
    );
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    request_method(app, Method::POST, uri, token).await
}

async fn request_method(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(
            request
                .body(Body::empty())
                .expect("subtitle delete request"),
        )
        .await
        .expect("subtitle delete response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Subtitle Delete Tests",
            "1.0",
            "Test",
            format!("emby-subtitle-delete-tests-{suffix}"),
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
