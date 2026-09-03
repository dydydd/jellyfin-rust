use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{TimeZone, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DeviceQuery, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use jellyfin_server_implementations::DefaultAuthenticationProvider;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, sea_query::Expr};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_AUTHORIZATION: &str = "MediaBrowser Client=\"Schedule%20Tests\", DeviceId=\"schedule-login\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn ignore_parental_control_matches_login_request_and_api_key_semantics() {
    let fixture = Fixture::new().await;
    fixture.assert_blocked_login_does_not_mutate_state().await;
    fixture.assert_existing_device_policy_difference().await;
    fixture.assert_api_key_sources_and_activity().await;
    fixture.assert_damaged_policy_fails_closed().await;
    fixture.cleanup().await;
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    users: UserService,
    devices: DeviceRepository,
    administrator_id: Uuid,
    user_id: Uuid,
    username: String,
    administrator_token: String,
    user_token: String,
    api_key_id: i64,
    api_key_token: String,
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
            .create_initial_administrator(&format!("schedule-admin-{suffix}"))
            .await
            .expect("administrator creation must succeed");
        let mut ordinary = users
            .create(&format!("schedule-user-{suffix}"))
            .await
            .expect("ordinary user creation must succeed");
        DefaultAuthenticationProvider::new().change_password(&mut ordinary, "correct password");
        let ordinary = users
            .set_password_hash(ordinary.id, ordinary.password_hash)
            .await
            .expect("ordinary user password setup must succeed");
        let administrator_token =
            create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let user_token = create_session(&devices, ordinary.id, &format!("ordinary-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("schedule-key-{suffix}"))
            .await
            .expect("API key creation must succeed");

        let blocked = vec![AccessSchedule {
            day_of_week: DynamicDayOfWeek::Everyday,
            start_hour: 18.0,
            end_hour: 6.0,
        }];
        users
            .update_policy(ordinary.id, &policy(false, blocked.clone()))
            .await
            .expect("ordinary blocked policy must persist");
        users
            .update_policy(administrator.id, &policy(true, blocked))
            .await
            .expect("administrator blocked policy must persist");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Schedule Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            users,
            devices,
            administrator_id: administrator.id,
            user_id: ordinary.id,
            username: ordinary.username,
            administrator_token,
            user_token,
            api_key_id: api_key.id,
            api_key_token: api_key.access_token,
        }
    }

    async fn assert_blocked_login_does_not_mutate_state(&self) {
        let before = self.users.get(self.user_id).await.unwrap();
        let device_count = self.device_count().await;

        assert_eq!(
            self.login("wrong password").await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            self.login("correct password").await.status(),
            StatusCode::FORBIDDEN
        );

        let after = self.users.get(self.user_id).await.unwrap();
        assert_eq!(after.last_login_date, before.last_login_date);
        assert_eq!(after.last_activity_date, before.last_activity_date);
        assert_eq!(self.device_count().await, device_count);
    }

    async fn assert_existing_device_policy_difference(&self) {
        let target = format!("/Users/{}", self.administrator_id);
        assert_eq!(
            self.get("/Users", None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            self.get(&target, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            self.get("/Users", Some(&self.user_token)).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            self.get(&target, Some(&self.user_token)).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            self.get("/Users", Some(&self.administrator_token))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            self.get(&target, Some(&self.administrator_token))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            self.get(
                &format!("/Users/{}", Uuid::new_v4()),
                Some(&self.administrator_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );

        self.users
            .update_policy(self.user_id, &policy(false, Vec::new()))
            .await
            .expect("empty schedule policy must persist");
        assert_eq!(
            self.get("/Users", Some(&self.user_token)).await.status(),
            StatusCode::OK
        );
    }

    async fn assert_api_key_sources_and_activity(&self) {
        let target = format!("/Users/{}", self.user_id);
        self.assert_api_key_touch("/Users", Some(&self.api_key_token))
            .await;
        self.assert_api_key_touch(&format!("/Users?api_key={}", self.api_key_token), None)
            .await;
        self.assert_api_key_touch(&target, Some(&self.api_key_token))
            .await;
        self.assert_api_key_touch(&format!("{target}?ApiKey={}", self.api_key_token), None)
            .await;
    }

    async fn assert_api_key_touch(&self, uri: &str, token: Option<&str>) {
        let old = Utc.timestamp_opt(1, 0).unwrap();
        let api_keys = ApiKeyRepository::new(self.database.clone());
        assert_eq!(api_keys.touch(&self.api_key_token, old).await.unwrap(), 1);
        assert_eq!(self.get(uri, token).await.status(), StatusCode::OK);
        let touched = api_keys
            .find_by_token(&self.api_key_token)
            .await
            .unwrap()
            .unwrap();
        assert!(touched.date_last_activity > old);
    }

    async fn assert_damaged_policy_fails_closed(&self) {
        user::Entity::update_many()
            .col_expr(user::Column::Policy, Expr::value(json!("damaged")))
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("damaged policy fixture must persist");
        let before = self.users.get(self.user_id).await.unwrap();
        let device_count = self.device_count().await;
        assert_eq!(
            self.get("/Users", Some(&self.user_token)).await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            self.get(
                &format!("/Users/{}", self.administrator_id),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            self.login("correct password").await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let after = self.users.get(self.user_id).await.unwrap();
        assert_eq!(after.last_login_date, before.last_login_date);
        assert_eq!(after.last_activity_date, before.last_activity_date);
        assert_eq!(self.device_count().await, device_count);

        assert_eq!(
            self.get("/Users", Some(&self.api_key_token)).await.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            self.get(
                &format!("/Users/{}", self.user_id),
                Some(&self.api_key_token),
            )
            .await
            .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            self.get(
                &format!("/Users/{}", self.administrator_id),
                Some(&self.api_key_token),
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    async fn device_count(&self) -> u64 {
        self.devices
            .query(&DeviceQuery {
                user_id: Some(self.user_id),
                ..DeviceQuery::default()
            })
            .await
            .expect("device count must succeed")
            .total_record_count
    }

    async fn login(&self, password: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::post("/Users/AuthenticateByName")
                    .header(header::AUTHORIZATION, CLIENT_AUTHORIZATION)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "Username": self.username, "Pw": password }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Some(token) = token {
            request = request.header("x-emby-token", token);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        api_key::Entity::delete_by_id(self.api_key_id)
            .exec(&self.database)
            .await
            .expect("API key cleanup must succeed");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup must succeed");
    }
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Schedule Tests",
            "1.0",
            "Test Device",
            device_id,
        ))
        .await
        .expect("device session creation must succeed")
        .access_token
}

fn policy(is_administrator: bool, access_schedules: Vec<AccessSchedule>) -> UserPolicy {
    UserPolicy {
        is_administrator,
        access_schedules,
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}
