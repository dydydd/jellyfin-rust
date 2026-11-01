use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use jellyfin_model::UserPolicy;
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn api_keys_create_and_delete_users_with_the_official_admin_role() {
    let fixture = Fixture::new().await;
    for (route, lowercase) in [("/Users/New", false), ("/users/new", true)] {
        let name = format!("key-created-{}", Uuid::new_v4().simple());
        let body = if lowercase {
            json!({"name": name, "password": "initial password"})
        } else {
            json!({"Name": name, "Password": "initial password"})
        };
        assert_eq!(
            request(&fixture.app, "POST", route, None, body.clone())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                route,
                Some(&fixture.user_token),
                body.clone()
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let created = request(&fixture.app, "POST", route, Some(&fixture.key_token), body).await;
        assert_eq!(created.status(), StatusCode::OK);
        let created = response_json(created).await;
        assert_eq!(created["Name"], name);
        assert_eq!(created["HasPassword"], true);
        assert_eq!(created["Policy"]["IsAdministrator"], false);
        let id = Uuid::parse_str(created["Id"].as_str().unwrap()).unwrap();
        let route = format!("/users/{id}");
        assert_eq!(
            request(
                &fixture.app,
                "DELETE",
                &route,
                Some(&fixture.user_token),
                Value::Null
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &fixture.app,
                "DELETE",
                &format!("{route}?api_key={}", fixture.key_token),
                None,
                Value::Null,
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        assert!(matches!(
            fixture.users.get(id).await,
            Err(jellyfin_controller::UserError::NotFound)
        ));
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn api_key_profile_and_configuration_updates_preserve_target_authorization() {
    let fixture = Fixture::new().await;
    let stored_user = fixture.users.get(fixture.user_id).await.unwrap();
    let mut policy: UserPolicy =
        serde_json::from_value(stored_user.policy).expect("persisted user policy");
    policy.enable_user_preference_access = false;
    fixture
        .users
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("disable ordinary preference access");
    assert_omitted_and_missing_targets(&fixture).await;
    assert_ordinary_user_permissions(&fixture).await;

    for route in [
        format!("/Users/Configuration?UserId={}", fixture.user_id),
        format!("/users/configuration?userid={}", fixture.user_id),
        format!("/Users/{}/Configuration", fixture.user_id),
        format!("/users/{}/configuration", fixture.user_id),
    ] {
        let language = Uuid::new_v4().simple().to_string();
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                &route,
                Some(&fixture.key_token),
                json!({"AudioLanguagePreference": language, "HidePlayedInLatest": false}),
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        let user = fixture.users.get(fixture.user_id).await.unwrap();
        assert_eq!(user.preferences["AudioLanguagePreference"], language);
        assert_eq!(user.preferences["HidePlayedInLatest"], false);
    }
    for route in [
        format!("/Users?userId={}", fixture.user_id),
        format!("/users/{}", fixture.user_id),
    ] {
        let name = format!("key-renamed-{}", Uuid::new_v4().simple());
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                &format!(
                    "{route}{}api_key={}",
                    if route.contains('?') { '&' } else { '?' },
                    fixture.key_token
                ),
                None,
                json!({"Name": name, "Configuration": {"RememberAudioSelections": false}}),
            )
            .await
            .status(),
            StatusCode::NO_CONTENT
        );
        let user = fixture.users.get(fixture.user_id).await.unwrap();
        assert_eq!(user.username, name);
        assert_eq!(user.preferences["RememberAudioSelections"], false);
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn api_key_password_changes_revoke_user_sessions_but_resets_preserve_them() {
    let fixture = Fixture::new().await;
    let provider = DefaultAuthenticationProvider::new();
    fixture
        .users
        .set_password_hash(fixture.user_id, provider.password_hash("old password"))
        .await
        .unwrap();
    let second_token = create_session(&fixture.devices, fixture.user_id).await;
    let reset = request(
        &fixture.app,
        "POST",
        &format!("/users/{}/password", fixture.user_id),
        Some(&fixture.key_token),
        json!({"resetpassword": true}),
    )
    .await;
    assert_eq!(reset.status(), StatusCode::NO_CONTENT);
    assert!(
        fixture
            .users
            .get(fixture.user_id)
            .await
            .unwrap()
            .password_hash
            .is_none()
    );
    for token in [&fixture.user_token, &second_token] {
        assert!(
            fixture
                .devices
                .find_by_token(token)
                .await
                .unwrap()
                .is_some()
        );
    }

    let new_password = Uuid::new_v4().simple().to_string();
    let changed = request(
        &fixture.app,
        "POST",
        &format!(
            "/users/password?userid={}&api_key={}",
            fixture.user_id, fixture.key_token
        ),
        None,
        json!({"newPw": new_password}),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT);
    let mut user = fixture.users.get(fixture.user_id).await.unwrap();
    let username = user.username.clone();
    assert!(
        provider
            .authenticate(&username, &new_password, Some(&mut user))
            .is_ok()
    );
    for token in [&fixture.user_token, &second_token] {
        assert!(
            fixture
                .devices
                .find_by_token(token)
                .await
                .unwrap()
                .is_none()
        );
    }
    let still_authorized = request(
        &fixture.app,
        "GET",
        &format!("/Users/{}", fixture.user_id),
        Some(&fixture.key_token),
        Value::Null,
    )
    .await;
    assert_eq!(still_authorized.status(), StatusCode::OK);
    fixture.cleanup().await;
}

async fn assert_omitted_and_missing_targets(fixture: &Fixture) {
    for (route, body) in [
        ("/Users".to_owned(), json!({"Name": "not-renamed"})),
        ("/users/configuration".to_owned(), json!({})),
        (
            "/users/password".to_owned(),
            json!({"NewPw": "not-written"}),
        ),
        (
            format!("/Users?userId={}", Uuid::nil()),
            json!({"Name": "not-renamed"}),
        ),
        (
            format!("/Users/{}/Configuration", Uuid::new_v4()),
            json!({}),
        ),
    ] {
        assert_eq!(
            request(&fixture.app, "POST", &route, Some(&fixture.key_token), body)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

async fn assert_ordinary_user_permissions(fixture: &Fixture) {
    for (route, body) in [
        ("/Users".to_owned(), json!({"Name": "not-renamed"})),
        ("/users/configuration".to_owned(), json!({})),
        ("/users/password".to_owned(), json!({"ResetPassword": true})),
        (
            format!("/users/{}/policy", fixture.user_id),
            json!({"IsAdministrator": true}),
        ),
        (
            format!("/Users/{}/Configuration", fixture.administrator_id),
            json!({}),
        ),
    ] {
        assert_eq!(
            request(
                &fixture.app,
                "POST",
                &route,
                Some(&fixture.user_token),
                body
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &fixture.app,
            "POST",
            &format!("/users/{}/configuration", Uuid::new_v4()),
            Some(&fixture.user_token),
            json!({}),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

struct Fixture {
    app: Router,
    database: sea_orm::DatabaseConnection,
    users: UserService,
    devices: DeviceRepository,
    administrator_id: Uuid,
    user_id: Uuid,
    user_token: String,
    key_id: i64,
    key_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("isolated PostgreSQL test database must be available");
        jellyfin_data::migrate(&database).await.unwrap();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!(
                "key-management-admin-{}",
                Uuid::new_v4().simple()
            ))
            .await
            .unwrap();
        let user = users
            .create(&format!("key-management-user-{}", Uuid::new_v4().simple()))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let user_token = create_session(&devices, user.id).await;
        let key = ApiKeyRepository::new(database.clone())
            .create(&format!("key-management-{}", Uuid::new_v4().simple()))
            .await
            .unwrap();
        Self {
            app: jellyfin_api::router(AppState::new(
                database.clone(),
                "API Key User Management Tests".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            database,
            users,
            devices,
            administrator_id: administrator.id,
            user_id: user.id,
            user_token,
            key_id: key.id,
            key_token: key.access_token,
        }
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
            .exec(&self.database)
            .await
            .unwrap();
        api_key::Entity::delete_by_id(self.key_id)
            .exec(&self.database)
            .await
            .unwrap();
    }
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "User Management Tests",
            "1.0",
            "Test Device",
            Uuid::new_v4().simple().to_string(),
        ))
        .await
        .unwrap()
        .access_token
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
