use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use jellyfin_model::UserPolicy;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const CLIENT_AUTHORIZATION: &str = "MediaBrowser Client=\"Policy%20Tests\", DeviceId=\"policy-login\", Device=\"Test\", Version=\"1.0\"";
static POLICY_ROUTE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn update_user_policy_matches_the_official_controller_contract() {
    let _guard = POLICY_ROUTE_LOCK.lock().await;
    let fixture = Fixture::new().await;
    fixture.assert_rejections().await;
    fixture.assert_persistence().await;
    fixture.assert_provider_authentication().await;
    fixture.assert_disable_behavior().await;
    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    users: UserService,
    devices: DeviceRepository,
    app: Router,
    administrator_id: Uuid,
    target_id: Uuid,
    target_username: String,
    admin_token: String,
    target_token: String,
    second_target_token: String,
    api_key_id: i64,
    api_key_token: String,
    route: String,
}

impl Fixture {
    async fn new() -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let devices = DeviceRepository::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("policy-route-admin-{suffix}"))
            .await
            .expect("administrator creation must succeed");
        let target = users
            .create(&format!("policy-route-user-{suffix}"))
            .await
            .expect("target user creation must succeed");
        let admin_token =
            create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let target_token = create_session(&devices, target.id, &format!("target-{suffix}")).await;
        let second_target_token =
            create_session(&devices, target.id, &format!("target-second-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("policy-route-key-{suffix}"))
            .await
            .expect("API key creation must succeed");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Policy Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            users,
            devices,
            app,
            administrator_id: administrator.id,
            target_id: target.id,
            target_username: target.username,
            admin_token,
            target_token,
            second_target_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
            route: format!("/Users/{}/Policy", target.id),
        }
    }

    async fn assert_rejections(&self) {
        let response = post_policy(&self.app, &self.route, None, valid_policy()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = post_policy(
            &self.app,
            &self.route,
            Some(&self.target_token),
            valid_policy(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let missing_route = format!("/Users/{}/Policy", Uuid::new_v4());
        let response = post_policy(
            &self.app,
            &missing_route,
            Some(&self.admin_token),
            valid_policy(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let mut invalid_missing = valid_policy();
        invalid_missing["AuthenticationProviderId"] = Value::Null;
        self.assert_bad_request(&missing_route, invalid_missing)
            .await;

        for invalid in [Value::Null, json!(""), json!(" \t")] {
            let mut policy = valid_policy();
            policy["PasswordResetProviderId"] = invalid.clone();
            self.assert_bad_request(&self.route, policy).await;
            let mut policy = valid_policy();
            policy["AuthenticationProviderId"] = invalid;
            self.assert_bad_request(&self.route, policy).await;
        }
    }

    async fn assert_bad_request(&self, route: &str, policy: Value) {
        let response = post_policy(&self.app, route, Some(&self.admin_token), policy).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    async fn assert_persistence(&self) {
        let arbitrary_policy = json!({
            "IsHidden": false,
            "EnableCollectionManagement": true,
            "AuthenticationProviderId": "Example.Authentication, Assembly",
            "PasswordResetProviderId": "Example.PasswordReset, Assembly"
        });
        let response = post_policy(
            &self.app,
            &self.route,
            Some(&self.admin_token),
            arbitrary_policy.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let stored = self.users.get(self.target_id).await.unwrap();
        assert_eq!(
            stored.authentication_provider_id,
            "Example.Authentication, Assembly"
        );
        assert_eq!(
            stored.password_reset_provider_id,
            "Example.PasswordReset, Assembly"
        );
        assert_eq!(
            stored.policy["AuthenticationProviderId"],
            arbitrary_policy["AuthenticationProviderId"]
        );
        assert_eq!(
            stored.policy["PasswordResetProviderId"],
            arbitrary_policy["PasswordResetProviderId"]
        );
        assert!(!stored.is_hidden);

        let mut stale_json = stored.into_active_model();
        stale_json.policy = Set(json!({
            "AuthenticationProviderId": "Stale.Authentication",
            "PasswordResetProviderId": "Stale.PasswordReset"
        }));
        stale_json.update(&self.database).await.unwrap();
        let response = get_authenticated(
            &self.app,
            &format!("/Users/{}", self.target_id),
            &self.admin_token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let dto = response_json(response).await;
        assert_eq!(
            dto["Policy"]["AuthenticationProviderId"],
            "Example.Authentication, Assembly"
        );
        assert_eq!(
            dto["Policy"]["PasswordResetProviderId"],
            "Example.PasswordReset, Assembly"
        );
    }

    async fn assert_provider_authentication(&self) {
        let response = authenticate(&self.app, &self.target_username).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let case_insensitive_policy = json!({
            "AuthenticationProviderId":
                UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_lowercase(),
            "PasswordResetProviderId":
                UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_uppercase()
        });
        let response = post_policy(
            &self.app,
            &format!("{}?api_key={}", self.route, self.api_key_token),
            None,
            case_insensitive_policy,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = authenticate(&self.app, &self.target_username).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn assert_disable_behavior(&self) {
        let mut disabled = valid_policy();
        disabled["IsDisabled"] = json!(true);
        let response = post_policy(&self.app, &self.route, Some(&self.admin_token), disabled).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        for token in [&self.target_token, &self.second_target_token] {
            assert!(self.devices.find_by_token(token).await.unwrap().is_none());
        }

        let mut disabled_admin = valid_policy();
        disabled_admin["IsAdministrator"] = json!(false);
        disabled_admin["IsDisabled"] = json!(true);
        let response = post_policy(
            &self.app,
            &format!("/Users/{}/Policy", self.administrator_id),
            Some(&self.admin_token),
            disabled_admin,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("test API key cleanup must succeed");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.target_id]))
            .exec(&self.database)
            .await
            .expect("test user cleanup must succeed");
    }
}

fn valid_policy() -> Value {
    json!({
        "AuthenticationProviderId": UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID,
        "PasswordResetProviderId": UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID
    })
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Policy Tests",
            "1.0",
            "Test Device",
            device_id,
        ))
        .await
        .expect("device session creation must succeed")
        .access_token
}

async fn post_policy(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    policy: Value,
) -> axum::response::Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::from(policy.to_string())).unwrap())
        .await
        .unwrap()
}

async fn get_authenticated(app: &Router, uri: &str, token: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(uri)
                .header("x-emby-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn authenticate(app: &Router, username: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post("/Users/AuthenticateByName")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, CLIENT_AUTHORIZATION)
                .body(Body::from(
                    json!({ "Username": username, "Pw": "" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body must be readable"),
    )
    .expect("response body must be JSON")
}
