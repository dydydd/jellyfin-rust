use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewUserData, UserDataRepository,
};
use jellyfin_model::{UserConfiguration, UserPolicy};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Copy Data Tests\", DeviceId=\"emby-copy-data-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_copy_data_";

#[tokio::test]
async fn copy_data_is_authorized_atomic_and_postgres_backed() {
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
    fixture.assert_options_and_authorization().await;
    fixture.assert_validation_is_atomic().await;
    fixture.assert_copy_and_protocol_isolation().await;
    fixture.assert_create_with_copy().await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: axum::Router,
    jellyfin: axum::Router,
    admin_token: String,
    administrator_user_id: Uuid,
    user_token: String,
    api_key: String,
    source_user_id: Uuid,
    target_user_id: Uuid,
    second_target_user_id: Uuid,
    item_id: Uuid,
    unrelated_item_id: Uuid,
    missing_user_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("copy-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let source = users
            .create(&format!("copy-source-{suffix}"))
            .await
            .expect("source user creation");
        let target = users
            .create(&format!("copy-target-{suffix}"))
            .await
            .expect("target user creation");
        let second_target = users
            .create(&format!("copy-target-two-{suffix}"))
            .await
            .expect("second target user creation");
        let mut source_policy = UserPolicy {
            is_hidden: false,
            enable_remote_access: false,
            remote_client_bitrate_limit: 4_000_000,
            ..UserPolicy::default()
        };
        source_policy.authentication_provider_id =
            Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned());
        source_policy.password_reset_provider_id =
            Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned());
        users
            .update_emby_policy(
                source.id,
                &source_policy,
                json!({
                    "IsHidden": false,
                    "EnableRemoteAccess": false,
                    "RemoteClientBitrateLimit": 4000000,
                    "IsHiddenRemotely": true,
                    "BlockUnratedItems": ["Game", "Other"]
                }),
            )
            .await
            .expect("source Emby policy");
        let source_configuration = UserConfiguration {
            audio_language_preference: Some("deu".to_owned()),
            remember_audio_selections: false,
            ..UserConfiguration::default()
        };
        users
            .update_emby_configuration(
                source.id,
                &source_configuration,
                json!({
                    "AudioLanguagePreference": "deu",
                    "RememberAudioSelections": false,
                    "IntroSkipMode": "AutoSkip"
                }),
            )
            .await
            .expect("source Emby configuration");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, source.id, &format!("source-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("copy-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let item_id = create_item(&items, root.id, "Copied item").await;
        let unrelated_item_id = create_item(&items, root.id, "Unrelated target item").await;
        let data = UserDataRepository::new(database.clone());

        let mut source_data = NewUserData::new(item_id, source.id, "copy-key");
        source_data.rating = Some(8.5);
        source_data.playback_position_ticks = 12_345;
        source_data.play_count = 4;
        source_data.is_favorite = true;
        source_data.last_played_date = Some(
            Utc.with_ymd_and_hms(2026, 9, 13, 8, 30, 0)
                .single()
                .expect("valid date"),
        );
        source_data.played = true;
        source_data.audio_stream_index = Some(2);
        source_data.subtitle_stream_index = Some(3);
        source_data.likes = Some(true);
        source_data.retention_date = Some(
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0)
                .single()
                .expect("valid date"),
        );
        source_data.is_hidden_from_resume = true;
        data.upsert(source_data).await.expect("source user data");

        let mut old_target_data = NewUserData::new(item_id, target.id, "copy-key");
        old_target_data.rating = Some(1.0);
        old_target_data.playback_position_ticks = 1;
        data.upsert(old_target_data)
            .await
            .expect("old conflicting target data");
        let mut unrelated = NewUserData::new(unrelated_item_id, target.id, "target-only");
        unrelated.is_favorite = true;
        data.upsert(unrelated).await.expect("unrelated target data");

        Self {
            emby: jellyfin_emby_api::router(AppState::new(
                database.clone(),
                "Emby Copy Data Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            jellyfin: jellyfin_api::router(AppState::new(
                database.clone(),
                "Jellyfin Copy Data Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            admin_token,
            administrator_user_id: administrator.id,
            user_token,
            api_key,
            source_user_id: source.id,
            target_user_id: target.id,
            second_target_user_id: second_target.id,
            item_id,
            unrelated_item_id,
            missing_user_id: Uuid::new_v4(),
        }
    }

    async fn assert_options_and_authorization(&self) {
        for route in [
            "/emby/Users/CopyDataOptions",
            "/emby/users/copydataoptions",
            "/emby/uSeRs/cOpYdAtAoPtIoNs",
        ] {
            assert_eq!(
                request(&self.emby, Method::GET, route, None, None)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED,
                "{route}"
            );
            assert_eq!(
                request(&self.emby, Method::GET, route, Some(&self.user_token), None,)
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "{route}"
            );
            let response = request(
                &self.emby,
                Method::GET,
                route,
                Some(&self.admin_token),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "{route}");
            assert_eq!(
                response_json(response).await,
                json!({"DataOptions":[
                    {"Name":"User Policy","Id":"UserPolicy"},
                    {"Name":"User Configuration","Id":"UserConfiguration"},
                    {"Name":"User Data","Id":"UserData"}
                ]})
            );
        }
    }

    async fn assert_validation_is_atomic(&self) {
        let route = format!("/emby/Users/{}/CopyData", self.source_user_id);
        assert_eq!(
            request(&self.emby, Method::POST, &route, None, Some("{"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &route,
                Some(&self.user_token),
                Some("{"),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "administrator authorization must precede malformed JSON"
        );
        let missing_source = format!("/emby/Users/{}/CopyData", self.missing_user_id);
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &missing_source,
                Some(&self.admin_token),
                Some("{"),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "source resolution must precede malformed JSON"
        );
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Users/not-a-uuid/CopyData",
                Some(&self.admin_token),
                Some("{}"),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );

        for body in [
            json!({}),
            json!({"ToUserIds":[],"CopyOptions":["UserData"]}),
            json!({"ToUserIds":[self.target_user_id],"CopyOptions":["Unknown"]}),
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &route,
                    Some(&self.admin_token),
                    Some(&body.to_string()),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        self.assert_target_still_old().await;

        let sole_admin_target = json!({
            "ToUserIds":[self.administrator_user_id],
            "CopyOptions":["UserPolicy"]
        });
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &route,
                Some(&self.admin_token),
                Some(&sole_admin_target.to_string()),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "copying a non-administrator policy cannot remove the sole administrator"
        );
        assert!(
            UserService::new(self.database.clone())
                .get(self.administrator_user_id)
                .await
                .expect("administrator after rejected copy")
                .is_administrator
        );

        for body in [
            json!({"ToUserIds":[self.target_user_id]}),
            json!({"ToUserIds":[self.target_user_id],"CopyOptions":[]}),
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    Method::POST,
                    &route,
                    Some(&self.admin_token),
                    Some(&body.to_string()),
                )
                .await
                .status(),
                StatusCode::OK,
                "omitted or empty nullable options are a validated no-op"
            );
        }
        self.assert_target_still_old().await;

        let missing_target = json!({
            "ToUserIds":[self.target_user_id,self.missing_user_id],
            "CopyOptions":["UserData"]
        });
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &route,
                Some(&self.admin_token),
                Some(&missing_target.to_string()),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        self.assert_target_still_old().await;
    }

    async fn assert_copy_and_protocol_isolation(&self) {
        let route = format!("/emby/uSeRs/{}/cOpYdAtA", self.source_user_id);
        let body = json!({
            // The route value remains authoritative when an SDK repeats a
            // different nullable UserId inside the request body.
            "uSeRiD": self.missing_user_id,
            "tOuSeRiDs": [
                self.target_user_id,
                self.target_user_id,
                self.second_target_user_id
            ],
            "cOpYoPtIoNs": ["userpolicy", "UserConfiguration", "userdata", "UserData"]
        });
        let response = request(
            &self.emby,
            Method::POST,
            &route,
            Some(&self.admin_token),
            Some(&body.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json(response).await.is_null());

        for target_user_id in [self.target_user_id, self.second_target_user_id] {
            self.assert_target_matches_source(target_user_id).await;
            self.assert_target_contract_matches_source(target_user_id)
                .await;
        }
        assert!(
            UserDataRepository::new(self.database.clone())
                .get(self.unrelated_item_id, self.target_user_id, "target-only")
                .await
                .expect("unrelated row lookup")
                .is_some(),
            "copying user data must not erase target-only rows"
        );

        // API keys are administrator equivalents and a second copy overwrites
        // the matching target row from the current source values.
        let repository = UserDataRepository::new(self.database.clone());
        let mut source = repository
            .get(self.item_id, self.source_user_id, "copy-key")
            .await
            .expect("source lookup")
            .expect("source row");
        source.playback_position_ticks = 99_999;
        repository
            .upsert(NewUserData {
                item_id: source.item_id,
                user_id: source.user_id,
                custom_data_key: source.custom_data_key,
                rating: source.rating,
                playback_position_ticks: source.playback_position_ticks,
                play_count: source.play_count,
                is_favorite: source.is_favorite,
                last_played_date: source.last_played_date,
                played: source.played,
                audio_stream_index: source.audio_stream_index,
                subtitle_stream_index: source.subtitle_stream_index,
                likes: source.likes,
                retention_date: source.retention_date,
                is_hidden_from_resume: source.is_hidden_from_resume,
            })
            .await
            .expect("source update");
        let api_key_route = format!(
            "/emby/users/{}/copydata?api_key={}",
            self.source_user_id, self.api_key
        );
        let api_key_body = json!({
            "ToUserIds":[self.target_user_id],
            "CopyOptions":["UserData"]
        });
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                &api_key_route,
                None,
                Some(&api_key_body.to_string()),
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            repository
                .get(self.item_id, self.target_user_id, "copy-key")
                .await
                .expect("target lookup")
                .expect("target row")
                .playback_position_ticks,
            99_999
        );

        assert_eq!(
            request(
                &self.jellyfin,
                Method::POST,
                &format!("/Users/{}/CopyData", self.source_user_id),
                Some(&self.admin_token),
                Some(&api_key_body.to_string()),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
            "the Emby mutation must not appear on Jellyfin's root route tree"
        );
    }

    async fn assert_create_with_copy(&self) {
        let copied_name = format!("created-copy-{}", Uuid::new_v4().simple());
        let body = json!({
            "nAmE": copied_name,
            "COPYFROMUSERID": self.source_user_id,
            "usercopyoptions": ["UserPolicy", "UserConfiguration", "UserData"]
        });
        let response = request(
            &self.emby,
            Method::POST,
            "/emby/uSeRs/nEw",
            Some(&self.admin_token),
            Some(&body.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let created_id = response_json(response).await["Id"]
            .as_str()
            .expect("created user id")
            .parse::<Uuid>()
            .expect("created user uuid");
        self.assert_target_matches_source(created_id).await;
        self.assert_target_contract_matches_source(created_id).await;

        // The dashboard always submits its checkbox array. The nullable SDK
        // field has no proven server-side default, so omission copies none.
        let default_name = format!("created-default-{}", Uuid::new_v4().simple());
        let default_body = json!({
            "Name": default_name,
            "CopyFromUserId": self.source_user_id
        });
        let response = request(
            &self.emby,
            Method::POST,
            "/emby/Users/New",
            Some(&self.admin_token),
            Some(&default_body.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let default_id = response_json(response).await["Id"]
            .as_str()
            .expect("default-copy user id")
            .parse::<Uuid>()
            .expect("default-copy user uuid");
        let default_user = UserService::new(self.database.clone())
            .get(default_id)
            .await
            .expect("default-copy user");
        let source = self.source_user().await;
        assert_ne!(default_user.policy, source.policy);
        assert_ne!(default_user.preferences, source.preferences);
        assert!(
            UserDataRepository::new(self.database.clone())
                .get(self.item_id, default_id, "copy-key")
                .await
                .expect("default-copy data lookup")
                .is_none()
        );

        let rejected_name = format!("created-rejected-{}", Uuid::new_v4().simple());
        let rejected = json!({
            "Name": rejected_name,
            "CopyFromUserId": self.source_user_id,
            "UserCopyOptions": ["Unknown"]
        });
        assert_eq!(
            request(
                &self.emby,
                Method::POST,
                "/emby/Users/New",
                Some(&self.admin_token),
                Some(&rejected.to_string()),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert!(
            UserService::new(self.database.clone())
                .list()
                .await
                .expect("user list")
                .iter()
                .all(|user| user.username != rejected_name),
            "invalid copy options must be rejected before creating a user"
        );

        // Jellyfin's unprefixed route continues to ignore Emby-only fields.
        let jellyfin_name = format!("jellyfin-no-copy-{}", Uuid::new_v4().simple());
        let jellyfin_body = json!({
            "Name": jellyfin_name,
            "CopyFromUserId": self.source_user_id,
            "UserCopyOptions": ["UserPolicy", "UserConfiguration", "UserData"]
        });
        let response = request(
            &self.jellyfin,
            Method::POST,
            "/Users/New",
            Some(&self.admin_token),
            Some(&jellyfin_body.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let jellyfin_id = response_json(response).await["Id"]
            .as_str()
            .expect("Jellyfin-created user id")
            .parse::<Uuid>()
            .expect("Jellyfin-created user uuid");
        assert!(
            UserDataRepository::new(self.database.clone())
                .get(self.item_id, jellyfin_id, "copy-key")
                .await
                .expect("Jellyfin-created user data lookup")
                .is_none()
        );
        let jellyfin_user = UserService::new(self.database.clone())
            .get(jellyfin_id)
            .await
            .expect("Jellyfin-created user");
        assert_ne!(jellyfin_user.policy, self.source_user().await.policy);
    }

    async fn source_user(&self) -> jellyfin_data::entities::user::Model {
        UserService::new(self.database.clone())
            .get(self.source_user_id)
            .await
            .expect("source user")
    }

    async fn assert_target_contract_matches_source(&self, target_user_id: Uuid) {
        let users = UserService::new(self.database.clone());
        let source = self.source_user().await;
        let target = users.get(target_user_id).await.expect("target user");
        assert_eq!(target.policy, source.policy);
        assert_eq!(target.preferences, source.preferences);
        assert_eq!(target.is_administrator, source.is_administrator);
        assert_eq!(target.is_hidden, source.is_hidden);
        assert_eq!(target.is_disabled, source.is_disabled);
        assert_eq!(
            target.invalid_login_attempt_count,
            source.invalid_login_attempt_count
        );
        assert_eq!(
            target.login_attempts_before_lockout,
            source.login_attempts_before_lockout
        );
        assert_eq!(
            target.authentication_provider_id,
            source.authentication_provider_id
        );
        assert_eq!(
            target.password_reset_provider_id,
            source.password_reset_provider_id
        );
        assert_eq!(target.enable_local_password, source.enable_local_password);
    }

    async fn assert_target_still_old(&self) {
        let target = UserDataRepository::new(self.database.clone())
            .get(self.item_id, self.target_user_id, "copy-key")
            .await
            .expect("target lookup")
            .expect("target row");
        assert_eq!(target.rating, Some(1.0));
        assert_eq!(target.playback_position_ticks, 1);
    }

    async fn assert_target_matches_source(&self, target_user_id: Uuid) {
        let repository = UserDataRepository::new(self.database.clone());
        let source = repository
            .get(self.item_id, self.source_user_id, "copy-key")
            .await
            .expect("source lookup")
            .expect("source row");
        let target = repository
            .get(self.item_id, target_user_id, "copy-key")
            .await
            .expect("target lookup")
            .expect("target row");
        assert_eq!(target.item_id, source.item_id);
        assert_eq!(target.custom_data_key, source.custom_data_key);
        assert_eq!(target.rating, source.rating);
        assert_eq!(
            target.playback_position_ticks,
            source.playback_position_ticks
        );
        assert_eq!(target.play_count, source.play_count);
        assert_eq!(target.is_favorite, source.is_favorite);
        assert_eq!(target.last_played_date, source.last_played_date);
        assert_eq!(target.played, source.played);
        assert_eq!(target.audio_stream_index, source.audio_stream_index);
        assert_eq!(target.subtitle_stream_index, source.subtitle_stream_index);
        assert_eq!(target.likes, source.likes);
        assert_eq!(target.retention_date, source.retention_date);
        assert_eq!(target.is_hidden_from_resume, source.is_hidden_from_resume);
    }
}

async fn create_item(items: &BaseItemRepository, parent_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    let mut item = NewBaseItem::new(id, "Video");
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    items.create(item).await.expect("item creation");
    id
}

async fn session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Copy Data Tests",
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
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
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
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body");
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON response")
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
}
