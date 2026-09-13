use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NamedConfigurationRepository, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Encoding Tests\", DeviceId=\"emby-encoding-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_encoding_configuration_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn encoding_editors_enforce_sdk_contracts_and_persist_in_postgres() {
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
    administrator.close().await.expect("database pool cleanup");
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
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_authorization_and_binding().await;
    fixture.assert_static_editors().await;
    fixture.assert_codec_parameters().await;
    fixture.assert_restart_and_protocol_isolation().await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: axum::Router,
    jellyfin: axum::Router,
    admin_token: String,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        NamedConfigurationRepository::new(database.clone())
            .save("encoding", json!({"RootOnly": true}))
            .await
            .expect("root Jellyfin encoding configuration seed");

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-encoding-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("emby-encoding-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("emby-encoding-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        Self {
            emby: jellyfin_emby_api::router(AppState::new(
                database.clone(),
                "Emby Encoding Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            jellyfin: jellyfin_api::router(AppState::new(
                database.clone(),
                "Jellyfin Encoding Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            admin_token,
            user_token,
            api_key,
        }
    }

    async fn assert_authorization_and_binding(&self) {
        let static_route = "/emby/Encoding/FullToneMapOptions";
        assert_eq!(
            request(&self.emby, Method::GET, static_route, None, None, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                static_route,
                Some(&self.user_token),
                Some("{"),
                Some("application/json"),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "administrator authorization must precede malformed JSON"
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                static_route,
                Some(&self.admin_token),
                Some("{"),
                Some("application/json"),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        for invalid in ["null", "[]", "true", "\"value\""] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    static_route,
                    Some(&self.admin_token),
                    Some(invalid),
                    Some("application/octet-stream"),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "{invalid}"
            );
        }

        let malformed_codec =
            "/emby/Encoding/CodecParameters?CodecId=h264&ParameterContext=invalid";
        assert_eq!(
            request(&self.emby, Method::GET, malformed_codec, None, None, None,)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede query binding"
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                malformed_codec,
                Some(&self.user_token),
                Some("{"),
                None,
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "administrator authorization must precede query and body binding"
        );
        assert_eq!(
            request(
                &self.emby,
                Method::GET,
                malformed_codec,
                Some(&self.user_token),
                None,
                None,
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_static_editors(&self) {
        let cases = [
            (
                "/emby/Encoding/FullToneMapOptions",
                "/emby/encoding/fulltonemapoptions",
                json!({"EnableToneMapping": true, "ToneMappingAlgorithm": "mobius"}),
            ),
            (
                "/emby/encoding/publictonemapoptions",
                "/emby/EnCoDiNg/PuBlIcToNeMaPoPtIoNs",
                json!({"showAdvanced": false, "OutputRange": "Auto"}),
            ),
            (
                "/emby/EnCoDiNg/SuBtItLeOpTiOnS",
                "/emby/Encoding/SubtitleOptions",
                json!({"AllowEmbeddedSubtitles": "AllowText", "FontSize": 24}),
            ),
            (
                "/emby/Encoding/FfmpegOptions",
                "/emby/encoding/ffmpegoptions",
                json!({"EncoderPreset": "veryfast", "Threads": -1}),
            ),
        ];

        for (post_route, get_route, object) in cases {
            let empty = response_json(
                request(
                    &self.emby,
                    Method::GET,
                    get_route,
                    Some(&self.user_token),
                    None,
                    None,
                )
                .await,
            )
            .await;
            assert_eq!(empty["Object"], json!({}));
            assert_eq!(empty["DefaultObject"], json!({}));
            assert!(empty.get("TypeName").is_none());
            assert!(empty.get("EditorRoot").is_none());

            let response = request(
                &self.emby,
                Method::POST,
                post_route,
                Some(&self.admin_token),
                Some(&object.to_string()),
                Some("application/json"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{post_route}");
            assert!(response_json(response).await.is_null());

            let persisted = response_json(
                request(
                    &self.emby,
                    Method::GET,
                    get_route,
                    Some(&self.user_token),
                    None,
                    None,
                )
                .await,
            )
            .await;
            assert_eq!(persisted["Object"], object, "{get_route}");
            assert_eq!(persisted["DefaultObject"], json!({}));
        }
    }

    async fn assert_codec_parameters(&self) {
        let playback = json!({"Profile": "high", "BFrames": 3});
        let conversion = json!({"Profile": "main", "Crf": 21});
        let playback_route =
            "/emby/Encoding/CodecParameters?CodecId=h264%2Bnvenc&ParameterContext=Playback";
        let conversion_route =
            "/emby/encoding/codecparameters?codecid=h264%2Bnvenc&parametercontext=1";

        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                playback_route,
                Some(&self.admin_token),
                Some(&playback.to_string()),
                Some("application/octet-stream"),
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                conversion_route,
                None,
                Some(&conversion.to_string()),
                Some("application/json"),
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        let api_key_route = format!("{conversion_route}&api_key={}", self.api_key);
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &api_key_route,
                None,
                Some(&conversion.to_string()),
                Some("application/json"),
            )
            .await
            .status(),
            StatusCode::OK
        );

        let mixed_playback = "/emby/eNcOdInG/cOdEcPaRaMeTeRs?CodecId=ignored&CODECID=h264%2Bnvenc&ParameterContext=invalid&PARAMETERCONTEXT=0";
        assert_eq!(
            response_json(
                request(
                    &self.emby,
                    Method::GET,
                    mixed_playback,
                    Some(&self.user_token),
                    None,
                    None,
                )
                .await,
            )
            .await["Object"],
            playback
        );
        assert_eq!(
            response_json(
                request(
                    &self.emby,
                    Method::GET,
                    conversion_route,
                    Some(&self.user_token),
                    None,
                    None,
                )
                .await,
            )
            .await["Object"],
            conversion
        );
    }

    async fn assert_restart_and_protocol_isolation(&self) {
        let restarted = jellyfin_emby_api::router(AppState::new(
            self.database.clone(),
            "Restarted Emby Encoding Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        let value = response_json(
            request(
                &restarted,
                Method::GET,
                "/emby/encoding/fulltonemapoptions",
                Some(&self.user_token),
                None,
                None,
            )
            .await,
        )
        .await;
        assert_eq!(value["Object"]["EnableToneMapping"], true);

        let repository = NamedConfigurationRepository::new(self.database.clone());
        assert_eq!(
            repository
                .load("encoding")
                .await
                .expect("Jellyfin root encoding configuration")
                .configuration,
            json!({"RootOnly": true})
        );
        for key in [
            "emby-encoding-full-tone-map-options",
            "emby-encoding-public-tone-map-options",
            "emby-encoding-subtitle-options",
            "emby-encoding-ffmpeg-options",
            "emby-encoding-codec-parameters-playback-683236342b6e76656e63",
            "emby-encoding-codec-parameters-conversion-683236342b6e76656e63",
        ] {
            assert!(
                repository.load(key).await.is_ok(),
                "missing persisted row {key}"
            );
            assert_eq!(
                request(
                    &self.jellyfin,
                    Method::GET,
                    &format!("/System/Configuration/{key}"),
                    Some(&self.user_token),
                    None,
                    None,
                )
                .await
                .status(),
                StatusCode::NOT_FOUND,
                "Emby protocol state must not leak through Jellyfin's named configuration API"
            );
        }
        assert_eq!(
            request(
                &self.jellyfin,
                Method::GET,
                "/Encoding/FullToneMapOptions",
                Some(&self.user_token),
                None,
                None,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Encoding Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn request(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
    content_type: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    app.clone()
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_owned())))
                .expect("request"),
        )
        .await
        .expect("response")
}

async fn response_json(response: Response) -> Value {
    let status = response.status();
    let body = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .expect("response body");
    if body.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&body)
        .unwrap_or_else(|error| panic!("{status} response must be JSON: {error}; body={body:?}"))
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
}
