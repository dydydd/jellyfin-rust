use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby DLNA Profile Tests\", DeviceId=\"emby-dlna-profile-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_dlna_profile_";

#[tokio::test]
async fn dlna_profiles_are_persistent_elevated_sdk_safe_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary PostgreSQL migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_authorization_and_default().await;
    fixture.assert_create_and_read().await;
    fixture.assert_persistence_and_update().await;
    fixture.assert_delete().await;
    fixture.assert_protocol_isolation().await;

    drop(fixture);
    database.close().await.expect("database close");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    admin_token: String,
    user_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("dlna-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("dlna-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("dlna-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database.clone(),
            "Emby DLNA Profile Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            database,
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            admin_token,
            user_token,
            api_key_token,
        }
    }

    async fn assert_authorization_and_default(&self) {
        let default_path = "/emby/dLnA/pRoFiLeS/dEfAuLt";
        assert_eq!(
            request(&self.emby, Method::GET, default_path, None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                default_path,
                Some(&self.user_token),
                None,
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        for token in [&self.admin_token, &self.api_key_token] {
            let response = request(&self.emby, Method::GET, default_path, Some(token), None).await;
            assert_eq!(response.status(), StatusCode::OK);
            let profile = response_json(response).await;
            assert_eq!(profile["Name"], "Generic Device");
            assert_eq!(profile["Type"], "System");
            assert_eq!(profile["MaxStreamingBitrate"], 140_000_000_i64);
            assert_eq!(profile["DirectPlayProfiles"].as_array().unwrap().len(), 2);
            assert_eq!(profile["TranscodingProfiles"].as_array().unwrap().len(), 3);
            assert_eq!(profile["SubtitleProfiles"].as_array().unwrap().len(), 12);
            assert_eq!(profile["ProtocolInfoDetection"]["EnabledForVideo"], true);
        }

        let malformed = Some(b"{".as_slice());
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Dlna/Profiles",
                None,
                malformed,
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED,
            "authorization must precede body binding",
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Dlna/Profiles",
                Some(&self.user_token),
                malformed,
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
        );
    }

    async fn assert_create_and_read(&self) {
        let body = serde_json::to_vec(&json!({
            "name": "first name",
            "NAME": "Living Room",
            "id": "wrong-id",
            "ID": "Profile-One",
            "type": "System",
            "unknown": "ignored",
            "MAXSTREAMINGBITRATE": "123456789",
            "protocolinfodetection": {
                "enabledforvideo": true,
                "ENABLEDFORAUDIO": "false"
            },
            "directplayprofiles": [{
                "container": "mkv",
                "type": "1"
            }],
            "subtitleprofiles": [{
                "format": "srt",
                "method": 2,
                "allowchunkedresponse": "true"
            }],
            "codecprofiles": [{
                "type": 2,
                "conditions": [{
                    "condition": "2",
                    "property": "22",
                    "value": "48000",
                    "isrequired": "false"
                }]
            }]
        }))
        .unwrap();
        let response = request(
            &self.emby,
            Method::POST,
            "/emby/dLnA/pRoFiLeS",
            Some(&self.admin_token),
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_bytes(response).await.is_empty());

        let response = request(
            &self.emby,
            Method::GET,
            "/emby/DLNA/PROFILES/profile-one",
            Some(&self.admin_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let profile = response_json(response).await;
        assert_eq!(profile["Name"], "Living Room");
        assert_eq!(profile["Id"], "Profile-One");
        assert_eq!(profile["Type"], "User");
        assert_eq!(profile["MaxStreamingBitrate"], 123_456_789_i64);
        assert_eq!(profile["ProtocolInfoDetection"]["EnabledForVideo"], false);
        assert_eq!(profile["ProtocolInfoDetection"]["EnabledForAudio"], false);
        assert_eq!(profile["DirectPlayProfiles"][0]["Type"], "Video");
        assert_eq!(profile["SubtitleProfiles"][0]["Method"], "External");
        assert_eq!(
            profile["CodecProfiles"][0]["Conditions"][0]["Property"],
            "AudioSampleRate"
        );
        assert!(profile.get("Path").is_none());
        assert!(profile.get("unknown").is_none());

        for invalid in [
            json!({"Id":"missing-name"}),
            json!({"Name":"bad-enum", "DirectPlayProfiles":[{"Type":99}]}),
        ] {
            let body = serde_json::to_vec(&invalid).unwrap();
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    "/emby/Dlna/Profiles",
                    Some(&self.admin_token),
                    Some(&body),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
            );
        }
    }

    async fn assert_persistence_and_update(&self) {
        let replacement_state = AppState::new(
            self.database.clone(),
            "Replacement State".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let replacement = jellyfin_emby_api::router(replacement_state);
        let response = request(
            &replacement,
            Method::GET,
            "/emby/dlna/profiles/PROFILE-ONE",
            Some(&self.admin_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["Name"], "Living Room");

        let update = serde_json::to_vec(&json!({
            "Name": "Bedroom",
            "Id": "must-not-redirect",
            "ProtocolInfoDetection": {"EnabledForVideo": true},
            "DirectPlayProfiles": [{"Type": "Photo", "Container": "jpeg"}]
        }))
        .unwrap();
        let response = request(
            &replacement,
            Method::POST,
            "/emby/Dlna/Profiles/profile-one",
            Some(&self.api_key_token),
            Some(&update),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let updated = response_json(
            request(
                &replacement,
                Method::GET,
                "/emby/Dlna/Profiles/Profile-One",
                Some(&self.admin_token),
                None,
            )
            .await,
        )
        .await;
        assert_eq!(updated["Id"], "profile-one");
        assert_eq!(updated["Name"], "Bedroom");
        assert_eq!(updated["ProtocolInfoDetection"]["EnabledForVideo"], false);

        let infos = response_json(
            request(
                &replacement,
                Method::GET,
                "/emby/Dlna/ProfileInfos",
                Some(&self.admin_token),
                None,
            )
            .await,
        )
        .await;
        assert_eq!(infos.as_array().unwrap().len(), 1);
        assert_eq!(infos[0]["Name"], "Bedroom");

        assert_eq!(
            request(
                &replacement,
                Method::POST,
                "/emby/Dlna/Profiles/not-found",
                Some(&self.admin_token),
                Some(&update),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    async fn assert_delete(&self) {
        let path = "/emby/dlna/profiles/PROFILE-ONE";
        let response = request(
            &self.emby,
            Method::DELETE,
            path,
            Some(&self.api_key_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_bytes(response).await.is_empty());
        assert_eq!(
            request(
                &self.emby,
                Method::DELETE,
                path,
                Some(&self.admin_token),
                None,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in [
            "/Dlna/Profiles",
            "/api/Dlna/Profiles",
            "/Dlna/Profiles/Default",
            "/api/Dlna/Profiles/Default",
            "/Dlna/Profiles/example",
            "/api/Dlna/Profiles/example",
        ] {
            assert_eq!(
                request(
                    &self.jellyfin,
                    Method::GET,
                    path,
                    Some(&self.admin_token),
                    None,
                )
                .await
                .status(),
                StatusCode::NOT_FOUND,
                "Emby DLNA profile route leaked into Jellyfin at {path}",
            );
        }
    }
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
) -> Response {
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
                .body(Body::from(body.unwrap_or_default().to_vec()))
                .expect("DLNA profile request"),
        )
        .await
        .expect("DLNA profile response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&response_bytes(response).await).expect("DLNA profile JSON response")
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("bounded DLNA profile response")
        .to_vec()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby DLNA Profile Tests",
            "1.0",
            "Test",
            format!("emby-dlna-profile-tests-{suffix}"),
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
