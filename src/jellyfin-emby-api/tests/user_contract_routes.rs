use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice};
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby User Contract Tests\", DeviceId=\"emby-user-contract-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_user_contract_";
const MAX_RESPONSE_SIZE: usize = 2 * 1024 * 1024;

#[tokio::test]
async fn emby_user_settings_round_trip_without_changing_jellyfin_contracts() {
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
        exercise_user_contract_routes(&task_database_name).await;
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

async fn exercise_user_contract_routes(database_name: &str) {
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
    assert_authorization_and_lookup_precede_json_binding(&fixture).await;
    update_emby_configuration_and_policy(&fixture).await;
    assert_persisted_protocol_documents(&fixture).await;
    assert_user_details_lists_and_authentication(&fixture).await;
    assert_restart_and_jellyfin_isolation(&fixture, database.clone()).await;
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    app: axum::Router,
    users: UserService,
    user_id: Uuid,
    username: String,
    admin_token: String,
    user_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("emby-contract-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let mut user = users
            .create(&format!("emby-contract-user-{suffix}"))
            .await
            .expect("user creation");
        DefaultAuthenticationProvider::new().change_password(&mut user, "correct-password");
        let user = users
            .set_password_hash(user.id, user.password_hash)
            .await
            .expect("password persistence");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        Self {
            app: jellyfin_emby_api::router(AppState::new(
                database,
                "Emby User Contract Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            users,
            user_id: user.id,
            username: user.username,
            admin_token,
            user_token,
        }
    }
}

async fn assert_authorization_and_lookup_precede_json_binding(fixture: &Fixture) {
    let configuration = format!("/emby/Users/{}/Configuration", fixture.user_id);
    assert_eq!(
        request(&fixture.app, "POST", &configuration, None, Body::from("{"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let missing_policy = format!("/emby/Users/{}/Policy", Uuid::new_v4());
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &missing_policy,
            Some(&fixture.admin_token),
            Body::from("{"),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let policy = format!("/emby/Users/{}/Policy", fixture.user_id);
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &policy,
            Some(&fixture.user_token),
            Body::from("{"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            "/emby/uSeRs/not-a-uuid/PoLiCy",
            Some(&fixture.user_token),
            Body::from("{")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "mixed-case elevated policy authorization must precede path/body binding"
    );
}

async fn update_emby_configuration_and_policy(fixture: &Fixture) {
    let view_id = Uuid::new_v4();
    let configuration = format!("/emby/uSeRs/{}/cOnFiGuRaTiOn", fixture.user_id);
    let configuration_body = format!(
        r#"{{
            "subtitlemode":"Always",
            "SubtitleMode":5,
            "introSKIPmode":1,
            "PROFILEpin":"7391",
            "HidePlayedInMoreLikeThis":true,
            "HidePlayedInSuggestions":true,
            "ResumeRewindSeconds":19,
            "OrderedViews":["{}"],
            "EnableLocalPassword":true,
            "UnknownSetting":"ignored"
        }}"#,
        view_id.simple()
    );
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &configuration,
            Some(&fixture.user_token),
            Body::from(configuration_body),
        )
        .await
        .status(),
        StatusCode::OK,
        "Emby's generated operation declares an empty 200 response"
    );

    let policy = format!("/emby/UsErS/{}/PoLiCy", fixture.user_id);
    let policy_body = json!({
        "IsHidden": false,
        "isHIDDENremotely": true,
        "IsHiddenFromUnusedDevices": true,
        "AllowTagOrRating": true,
        "IsTagBlockingModeInclusive": true,
        "IncludeTags": ["family"],
        "BlockUnratedItems": [4, "OTHER"],
        "EnableTranscodingQuality": true,
        "AutoRemoteQuality": 2,
        "RestrictedFeatures": ["feature-a"],
        "EnableSubtitleDownloading": true,
        "ExcludedSubFolders": ["private"],
        "SimultaneousStreamLimit": 3,
        "AllowCameraUpload": true,
        "AllowSharingPersonalItems": true,
        "UnknownPolicyField": "ignored"
    });
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &policy,
            Some(&fixture.admin_token),
            Body::from(policy_body.to_string()),
        )
        .await
        .status(),
        StatusCode::OK
    );
}

async fn assert_persisted_protocol_documents(fixture: &Fixture) {
    let stored = fixture
        .users
        .get(fixture.user_id)
        .await
        .expect("stored user");
    let configuration = &stored.preferences["EmbyUserConfiguration"];
    assert_eq!(configuration["SubtitleMode"], "HearingImpaired");
    assert_eq!(configuration["IntroSkipMode"], "AutoSkip");
    assert_eq!(configuration["ProfilePin"], "7391");
    assert!(configuration.get("UnknownSetting").is_none());
    let policy = &stored.policy["EmbyUserPolicy"];
    assert_eq!(policy["BlockUnratedItems"], json!(["Game", "Other"]));
    assert_eq!(policy["IsHiddenRemotely"], true);
    assert_eq!(policy["SimultaneousStreamLimit"], 3);
    assert!(policy.get("UnknownPolicyField").is_none());
}

async fn assert_user_details_lists_and_authentication(fixture: &Fixture) {
    let detail = get_json(
        &fixture.app,
        &format!("/emby/Users/{}", fixture.user_id),
        Some(&fixture.admin_token),
    )
    .await;
    assert_emby_user_contract(&detail);

    let query = get_json(
        &fixture.app,
        "/emby/Users/Query",
        Some(&fixture.admin_token),
    )
    .await;
    let listed = query["Items"]
        .as_array()
        .expect("query result items")
        .iter()
        .find(|user| user["Id"] == fixture.user_id.simple().to_string())
        .expect("updated user in query result");
    assert_emby_user_contract(listed);

    let password_route = format!("/emby/uSeRs/{}/pAsSwOrD", fixture.user_id);
    let response = request(
        &fixture.app,
        "POST",
        &password_route,
        Some(&fixture.user_token),
        Body::from(format!(
            r#"{{"Id":"{}","NewPw":"wrong-password","nEwPw":"updated-password","ResetPassword":true,"rEsEtPaSsWoRd":false}}"#,
            fixture.user_id.simple()
        )),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "{password_route}");

    let response = request(
        &fixture.app,
        "POST",
        "/emby/Users/AuthenticateByName",
        None,
        Body::from(json!({ "uSeRnAmE": fixture.username, "pW": "updated-password" }).to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let authentication = response_json(response).await;
    assert!(authentication["AccessToken"].is_string());
    assert_emby_user_contract(&authentication["User"]);

    let direct_route = format!(
        "/emby/uSeRs/{}/aUtHeNtIcAtE?pW=updated-password",
        fixture.user_id
    );
    let response = request(&fixture.app, "POST", &direct_route, None, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK, "{direct_route}");
    let authentication = response_json(response).await;
    assert_emby_user_contract(&authentication["User"]);
}

fn assert_emby_user_contract(user: &Value) {
    let configuration = &user["Configuration"];
    assert_eq!(configuration["SubtitleMode"], "HearingImpaired");
    assert_eq!(configuration["IntroSkipMode"], "AutoSkip");
    assert_eq!(configuration["ProfilePin"], "7391");
    assert_eq!(configuration["ResumeRewindSeconds"], 19);
    assert!(configuration.get("GroupedFolders").is_none());
    assert!(configuration.get("DisplayCollectionsView").is_none());

    let policy = &user["Policy"];
    assert_eq!(policy["IsHiddenRemotely"], true);
    assert_eq!(policy["BlockUnratedItems"], json!(["Game", "Other"]));
    assert_eq!(policy["SimultaneousStreamLimit"], 3);
    assert_eq!(policy["AllowCameraUpload"], true);
    assert!(policy.get("EnableCollectionManagement").is_none());
    assert!(policy.get("SyncPlayAccess").is_none());
}

async fn assert_restart_and_jellyfin_isolation(fixture: &Fixture, database: DatabaseConnection) {
    let restarted = jellyfin_emby_api::router(AppState::new(
        database.clone(),
        "Restarted Emby User Contract Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let restarted_detail = get_json(
        &restarted,
        &format!("/emby/users/{}", fixture.user_id),
        Some(&fixture.admin_token),
    )
    .await;
    assert_emby_user_contract(&restarted_detail);

    let jellyfin = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin User Contract Isolation Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let response = request(
        &jellyfin,
        "POST",
        &format!("/Users/{}/Password", fixture.user_id),
        Some(&fixture.user_token),
        Body::from(r#"{"NewPw":"must-not-change"}"#),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "Jellyfin self-service still requires CurrentPw"
    );
    let mut shared = get_json(
        &jellyfin,
        &format!("/Users/{}", fixture.user_id),
        Some(&fixture.admin_token),
    )
    .await;
    assert!(shared["Configuration"].get("ProfilePin").is_none());
    assert!(shared["Configuration"].get("ResumeRewindSeconds").is_none());
    assert!(shared["Policy"].get("IsHiddenRemotely").is_none());
    assert!(shared["Policy"].get("AllowCameraUpload").is_none());
    assert_eq!(shared["Policy"]["AllowedTags"], json!(["family"]));
    assert_eq!(shared["Policy"]["MaxActiveSessions"], 3);

    shared["Configuration"]["PlayDefaultAudioTrack"] = json!(false);
    shared["Configuration"]["EnableLocalPassword"] = json!(false);
    let response = request(
        &jellyfin,
        "POST",
        &format!("/Users/{}/Configuration", fixture.user_id),
        Some(&fixture.admin_token),
        Body::from(shared["Configuration"].to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    shared["Policy"]["EnableMediaPlayback"] = json!(false);
    shared["Policy"]["EnableRemoteAccess"] = json!(false);
    let response = request(
        &jellyfin,
        "POST",
        &format!("/Users/{}/Policy", fixture.user_id),
        Some(&fixture.admin_token),
        Body::from(shared["Policy"].to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let overlaid = get_json(
        &restarted,
        &format!("/emby/Users/{}", fixture.user_id),
        Some(&fixture.admin_token),
    )
    .await;
    assert_eq!(overlaid["Configuration"]["PlayDefaultAudioTrack"], false);
    assert_eq!(overlaid["Configuration"]["EnableLocalPassword"], false);
    assert_eq!(overlaid["Configuration"]["ProfilePin"], "7391");
    assert_eq!(overlaid["Configuration"]["SubtitleMode"], "HearingImpaired");
    assert_eq!(overlaid["Policy"]["EnableMediaPlayback"], false);
    assert_eq!(overlaid["Policy"]["EnableRemoteAccess"], false);
    assert_eq!(overlaid["Policy"]["IsHiddenRemotely"], true);
    assert_eq!(
        overlaid["Policy"]["BlockUnratedItems"],
        json!(["Game", "Other"])
    );
}

async fn get_json(app: &axum::Router, uri: &str, token: Option<&str>) -> Value {
    let response = request(app, "GET", uri, token, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    response_json(response).await
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> Response {
    let authorization = token.map_or_else(
        || AUTHORIZATION.to_owned(),
        |token| format!("{AUTHORIZATION}, Token=\"{token}\""),
    );
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, authorization);
    app.clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby User Contract Tests",
            "1.0",
            "Test",
            format!("emby-user-contract-tests-{suffix}"),
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
