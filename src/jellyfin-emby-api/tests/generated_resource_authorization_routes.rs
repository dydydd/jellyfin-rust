use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Generated Authorization Tests\", DeviceId=\"emby-generated-authorization-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_generated_auth_";

#[tokio::test]
async fn generated_resource_and_system_operations_use_emby_local_authorization() {
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
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");
    let fixture = Fixture::new(database.clone()).await;

    fixture.assert_system_lifecycle_contract().await;
    fixture.assert_generated_resource_contract().await;
    fixture.assert_bitrate_test_contract().await;
    fixture.assert_jellyfin_isolation().await;

    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("generated-auth-admin-{suffix}"))
            .await
            .expect("administrator");
        let user = users
            .create(&format!("generated-auth-user-{suffix}"))
            .await
            .expect("ordinary user");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("generated-auth-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Generated Authorization Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            user_token,
            api_key,
        }
    }

    async fn assert_system_lifecycle_contract(&self) {
        for route in [
            "/emby/System/Restart",
            "/emby/sYsTeM/rEsTaRt",
            "/emby/System/Shutdown",
            "/emby/sYsTeM/sHuTdOwN",
        ] {
            assert_eq!(
                request(&self.emby, Method::POST, route, None).await,
                StatusCode::UNAUTHORIZED,
                "anonymous {route}",
            );
            assert_eq!(
                request(&self.emby, Method::POST, route, Some(&self.user_token)).await,
                StatusCode::FORBIDDEN,
                "ordinary user {route}",
            );
            for token in [&self.admin_token, &self.api_key] {
                assert_eq!(
                    request(&self.emby, Method::POST, route, Some(token)).await,
                    StatusCode::OK,
                    "elevated caller {route}",
                );
            }
        }
    }

    async fn assert_generated_resource_contract(&self) {
        let id = Uuid::nil();
        let mut operations = Vec::new();
        for method in [Method::GET, Method::HEAD] {
            for route in [
                format!("/emby/Items/{id}/Images/Primary"),
                format!("/emby/iTeMs/{id}/iMaGeS/pRiMaRy/0"),
                format!("/emby/Items/{id}/Images/Primary/0/tag/jpg/400/300/0/0"),
                format!("/emby/Users/{id}/Images/Primary"),
                format!("/emby/uSeRs/{id}/iMaGeS/pRiMaRy/0"),
            ] {
                operations.push((method.clone(), route));
            }
            for kind in ["Artists", "Genres", "MusicGenres", "Persons", "Studios"] {
                operations.push((
                    method.clone(),
                    format!("/emby/{kind}/missing/Images/Primary"),
                ));
                operations.push((
                    method.clone(),
                    format!("/emby/{}/missing/iMaGeS/pRiMaRy/0", mixed_case(kind)),
                ));
            }
            operations.push((
                method.clone(),
                format!("/emby/Videos/{id}/{id}/Subtitles/0/Stream.srt"),
            ));
            operations.push((
                method,
                format!("/emby/vIdEoS/{id}/{id}/sUbTiTlEs/0/10000000/sTrEaM.vtt"),
            ));
        }
        operations.extend([
            (
                Method::GET,
                format!("/emby/Audio/{id}/hls/playlist/segment.ts"),
            ),
            (
                Method::GET,
                format!("/emby/vIdEoS/{id}/HlS/playlist/segment.ts"),
            ),
            (
                Method::GET,
                format!("/emby/vIdEoS/{id}/{id}/aTtAcHmEnTs/0/sTrEaM"),
            ),
        ]);

        assert_eq!(operations.len(), 37);
        for (method, route) in operations {
            assert_eq!(
                request(&self.emby, method.clone(), &route, None).await,
                StatusCode::UNAUTHORIZED,
                "anonymous generated operation {method} {route}",
            );
            for token in [&self.user_token, &self.admin_token, &self.api_key] {
                let status = request(&self.emby, method.clone(), &route, Some(token)).await;
                assert!(
                    !matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
                    "authenticated generated operation {method} {route}: {status}",
                );
            }
        }
    }

    async fn assert_bitrate_test_contract(&self) {
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                "/emby/Playback/BitrateTest?Size=1",
                None,
            )
            .await,
            StatusCode::UNAUTHORIZED,
        );
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                "/emby/Playback/BitrateTest",
                Some(&self.user_token),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "the generated Emby operation requires Size",
        );
        for token in [&self.user_token, &self.admin_token, &self.api_key] {
            let response = request_response(
                &self.emby,
                Method::GET,
                "/emby/pLaYbAcK/bItRaTeTeSt?Size=5&sIzE=1",
                Some(token),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[header::CONTENT_LENGTH], "1");
        }

        for route in ["/Playback/BitrateTest", "/api/Playback/BitrateTest"] {
            let response =
                request_response(&self.jellyfin, Method::GET, route, Some(&self.api_key)).await;
            assert_eq!(response.status(), StatusCode::OK, "{route}");
            assert_eq!(response.headers()[header::CONTENT_LENGTH], "102400");
        }
    }

    async fn assert_jellyfin_isolation(&self) {
        for route in ["/System/Restart", "/api/System/Restart"] {
            assert_eq!(
                request(&self.jellyfin, Method::POST, route, None).await,
                StatusCode::NO_CONTENT,
                "Jellyfin local restart must remain anonymous: {route}",
            );
        }
        for route in ["/System/Shutdown", "/api/System/Shutdown"] {
            assert_eq!(
                request(&self.jellyfin, Method::POST, route, None).await,
                StatusCode::UNAUTHORIZED,
                "Jellyfin shutdown must retain its existing elevation: {route}",
            );
        }

        let id = Uuid::nil();
        for route in [
            format!("/Items/{id}/Images/Primary"),
            format!("/api/Items/{id}/Images/Primary"),
            format!("/Videos/{id}/{id}/Subtitles/0/Stream.srt"),
            format!("/api/Videos/{id}/{id}/Subtitles/0/Stream.srt"),
            format!("/Videos/{id}/{id}/Attachments/0/Stream"),
            format!("/api/Videos/{id}/{id}/Attachments/0/Stream"),
            format!("/Audio/{id}/hls/playlist/segment.ts"),
            format!("/api/Audio/{id}/hls/playlist/segment.ts"),
        ] {
            assert_ne!(
                request(&self.jellyfin, Method::GET, &route, None).await,
                StatusCode::UNAUTHORIZED,
                "Emby user policy leaked into Jellyfin route {route}",
            );
        }
    }
}

fn mixed_case(kind: &str) -> &str {
    match kind {
        "Artists" => "aRtIsTs",
        "Genres" => "gEnReS",
        "MusicGenres" => "mUsIcGeNrEs",
        "Persons" => "pErSoNs",
        "Studios" => "sTuDiOs",
        _ => unreachable!("known image kind"),
    }
}

async fn request(app: &Router, method: Method, uri: &str, token: Option<&str>) -> StatusCode {
    request_response(app, method, uri, token).await.status()
}

async fn request_response(
    app: &Router,
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
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Generated Authorization Tests",
            "1.0",
            "Test",
            format!("emby-generated-authorization-tests-{suffix}"),
        ))
        .await
        .expect("session")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
