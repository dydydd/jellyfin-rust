use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use jellyfin_data::{
    DatabaseConfig, DeviceQuery, DeviceRepository, QuickConnectRepository, QuickConnectStoreError,
    entities::{device, quick_connect, user},
};
use jellyfin_server_implementations::{
    AuthorizationInfo, QuickConnectCapability, QuickConnectError, QuickConnectManager,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug)]
struct FixtureState {
    enabled: AtomicBool,
    now: Mutex<DateTime<Utc>>,
    codes: Mutex<VecDeque<String>>,
    secrets: Mutex<VecDeque<String>>,
    sequence: AtomicU64,
}

#[derive(Clone, Debug)]
struct FixtureCapability {
    state: Arc<FixtureState>,
}

impl FixtureCapability {
    fn new(enabled: bool) -> Self {
        let (seed, _) = Uuid::new_v4().as_u64_pair();
        Self {
            state: Arc::new(FixtureState {
                enabled: AtomicBool::new(enabled),
                now: Mutex::new(Utc::now()),
                codes: Mutex::new(VecDeque::new()),
                secrets: Mutex::new(VecDeque::new()),
                sequence: AtomicU64::new(seed),
            }),
        }
    }

    fn set_enabled(&self, enabled: bool) {
        self.state.enabled.store(enabled, Ordering::Release);
    }

    fn advance(&self, duration: Duration) {
        let mut now = self.state.now.lock().unwrap();
        *now += duration;
    }

    fn queue_code(&self, code: impl Into<String>) {
        self.state.codes.lock().unwrap().push_back(code.into());
    }

    fn queue_secret(&self, secret: impl Into<String>) {
        self.state.secrets.lock().unwrap().push_back(secret.into());
    }

    fn next_value(&self) -> u64 {
        self.state.sequence.fetch_add(1, Ordering::Relaxed)
    }
}

impl QuickConnectCapability for FixtureCapability {
    fn is_enabled(&self) -> bool {
        self.state.enabled.load(Ordering::Acquire)
    }

    fn now(&self) -> DateTime<Utc> {
        *self.state.now.lock().unwrap()
    }

    fn generate_code(&self) -> String {
        self.state
            .codes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| format!("{:06}", 100_000 + self.next_value() % 900_000))
    }

    fn generate_secret(&self) -> String {
        self.state
            .secrets
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| format!("{:064X}", self.next_value()))
    }
}

struct TestContext {
    database: DatabaseConnection,
    manager: QuickConnectManager<FixtureCapability>,
    capability: FixtureCapability,
    user_id: Uuid,
    device_prefix: String,
}

impl TestContext {
    async fn new(enabled: bool) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let user_id = Uuid::new_v4();
        insert_user(&database, user_id).await;
        let device_prefix = format!("QuickConnect-{}-", Uuid::new_v4().simple());
        let capability = FixtureCapability::new(enabled);
        let manager = QuickConnectManager::new(
            QuickConnectRepository::new(database.clone()),
            capability.clone(),
        );
        Self {
            database,
            manager,
            capability,
            user_id,
            device_prefix,
        }
    }

    fn authorization(&self) -> AuthorizationInfo {
        AuthorizationInfo {
            device_name: "Device".to_owned(),
            device_id: format!("{}{}", self.device_prefix, Uuid::new_v4().simple()),
            app_name: "Client".to_owned(),
            app_version: "1.0.0".to_owned(),
        }
    }

    async fn cleanup(&self) {
        quick_connect::Entity::delete_many()
            .filter(quick_connect::Column::DeviceId.starts_with(&self.device_prefix))
            .exec(&self.database)
            .await
            .expect("Quick Connect test requests must clean up");
        let deleted_user = user::Entity::delete_by_id(self.user_id)
            .exec(&self.database)
            .await
            .expect("Quick Connect test user and devices must clean up");
        assert_eq!(deleted_user.rows_affected, 1);
        assert!(
            quick_connect::Entity::find()
                .filter(quick_connect::Column::DeviceId.starts_with(&self.device_prefix))
                .one(&self.database)
                .await
                .expect("Quick Connect cleanup verification must succeed")
                .is_none()
        );
        assert!(
            device::Entity::find()
                .filter(device::Column::UserId.eq(self.user_id))
                .one(&self.database)
                .await
                .expect("Quick Connect device cleanup verification must succeed")
                .is_none()
        );
    }
}

// Official IsEnabled_QuickConnectUnavailable_False.
#[tokio::test]
async fn is_enabled_quick_connect_unavailable_false() {
    let context = TestContext::new(false).await;
    assert!(!context.manager.is_enabled());
    context.cleanup().await;
}

// Official TryConnect_InvalidAuthorizationInfo_ThrowsArgumentException: four rows.
#[tokio::test]
async fn try_connect_invalid_authorization_info_throws_argument_error() {
    let context = TestContext::new(true).await;
    for (field, mut authorization) in [
        ("device name", context.authorization()),
        ("device id", context.authorization()),
        ("app name", context.authorization()),
        ("app version", context.authorization()),
    ] {
        match field {
            "device name" => authorization.device_name.clear(),
            "device id" => authorization.device_id.clear(),
            "app name" => authorization.app_name.clear(),
            "app version" => authorization.app_version.clear(),
            _ => unreachable!(),
        }
        assert!(matches!(
            context.manager.try_connect(&authorization).await,
            Err(QuickConnectError::InvalidAuthorization(actual)) if actual == field
        ));
    }
    context.cleanup().await;
}

// Official TryConnect_QuickConnectUnavailable_ThrowsAuthenticationException.
#[tokio::test]
async fn try_connect_quick_connect_unavailable_throws_authentication_error() {
    let context = TestContext::new(false).await;
    assert!(matches!(
        context.manager.try_connect(&context.authorization()).await,
        Err(QuickConnectError::Disabled)
    ));
    context.cleanup().await;
}

// Official CheckRequestStatus_QuickConnectUnavailable_ThrowsAuthenticationException.
#[tokio::test]
async fn check_request_status_quick_connect_unavailable_throws_authentication_error() {
    let context = TestContext::new(false).await;
    assert!(matches!(
        context.manager.check_request_status("").await,
        Err(QuickConnectError::Disabled)
    ));
    context.cleanup().await;
}

// Official AuthorizeRequest_QuickConnectUnavailable_ThrowsAuthenticationException.
#[tokio::test]
async fn authorize_request_quick_connect_unavailable_throws_authentication_error() {
    let context = TestContext::new(false).await;
    assert!(matches!(
        context.manager.authorize_request(Uuid::nil(), "").await,
        Err(QuickConnectError::Disabled)
    ));
    context.cleanup().await;
}

// Official GetAuthorizedRequest_QuickConnectUnavailable_ThrowsAuthenticationException.
#[tokio::test]
async fn get_authorized_request_quick_connect_unavailable_throws_authentication_error() {
    let context = TestContext::new(false).await;
    assert!(matches!(
        context.manager.get_authorized_request("").await,
        Err(QuickConnectError::Disabled)
    ));
    context.cleanup().await;
}

// Official IsEnabled_QuickConnectAvailable_True.
#[tokio::test]
async fn is_enabled_quick_connect_available_true() {
    let context = TestContext::new(true).await;
    assert!(context.manager.is_enabled());
    context.capability.set_enabled(false);
    assert!(!context.manager.is_enabled());
    context.cleanup().await;
}

// Official CheckRequestStatus_QuickConnectAvailable_Success.
#[tokio::test]
async fn check_request_status_quick_connect_available_success() {
    let context = TestContext::new(true).await;
    let request = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    assert_eq!(
        context
            .manager
            .check_request_status(&request.secret)
            .await
            .unwrap(),
        request
    );
    context.cleanup().await;
}

// Official CheckRequestStatus_UnknownSecret_ThrowsResourceNotFoundException.
#[tokio::test]
async fn check_request_status_unknown_secret_throws_not_found() {
    let context = TestContext::new(true).await;
    assert!(matches!(
        context.manager.check_request_status("Unknown secret").await,
        Err(QuickConnectError::NotFound)
    ));
    context.cleanup().await;
}

// Official GetAuthorizedRequest_UnknownSecret_ThrowsResourceNotFoundException.
#[tokio::test]
async fn get_authorized_request_unknown_secret_throws_not_found() {
    let context = TestContext::new(true).await;
    assert!(matches!(
        context
            .manager
            .get_authorized_request("Unknown secret")
            .await,
        Err(QuickConnectError::NotFound)
    ));
    context.cleanup().await;
}

// Official AuthorizeRequest_QuickConnectAvailable_Success.
#[tokio::test]
async fn authorize_request_quick_connect_available_success() {
    let context = TestContext::new(true).await;
    let authorization = context.authorization();
    let request = context.manager.try_connect(&authorization).await.unwrap();
    assert!(
        context
            .manager
            .authorize_request(context.user_id, &request.code)
            .await
            .unwrap()
    );

    let status = context
        .manager
        .check_request_status(&request.secret)
        .await
        .unwrap();
    assert!(status.authenticated);
    let authenticated = context
        .manager
        .get_authorized_request(&request.secret)
        .await
        .unwrap();
    assert_eq!(authenticated.user_id, context.user_id);
    assert_eq!(authenticated.device_id, authorization.device_id);
    assert_eq!(authenticated.device_name, authorization.device_name);
    assert_eq!(authenticated.app_name, authorization.app_name);
    assert_eq!(authenticated.app_version, authorization.app_version);
    assert!(!authenticated.access_token.is_empty());
    context.cleanup().await;
}

#[tokio::test]
async fn expired_request_is_unknown_to_every_lifecycle_operation() {
    let context = TestContext::new(true).await;
    let request = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    context.capability.advance(Duration::minutes(11));

    assert!(matches!(
        context.manager.check_request_status(&request.secret).await,
        Err(QuickConnectError::NotFound)
    ));
    context.cleanup().await;
    assert!(matches!(
        context
            .manager
            .authorize_request(context.user_id, &request.code)
            .await,
        Err(QuickConnectError::NotFound)
    ));
    assert!(matches!(
        context
            .manager
            .get_authorized_request(&request.secret)
            .await,
        Err(QuickConnectError::NotFound)
    ));
}

#[tokio::test]
async fn unique_collision_retries_with_fresh_code_and_secret() {
    let context = TestContext::new(true).await;
    let first = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    let fresh_code = unused_code(&context.database, &[]).await;
    let fresh_secret = next_secret();
    context.capability.queue_code(first.code.clone());
    context.capability.queue_secret(next_secret());
    context.capability.queue_code(fresh_code.clone());
    context.capability.queue_secret(fresh_secret.clone());

    let second = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();

    assert_eq!(second.code, fresh_code);
    assert_eq!(second.secret, fresh_secret);

    let third_code = unused_code(&context.database, &[]).await;
    let final_code = unused_code(&context.database, std::slice::from_ref(&third_code)).await;
    let final_secret = next_secret();
    context.capability.queue_code(third_code);
    context.capability.queue_secret(first.secret);
    context.capability.queue_code(final_code.clone());
    context.capability.queue_secret(final_secret.clone());
    let third = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    assert_eq!(third.code, final_code);
    assert_eq!(third.secret, final_secret);
    context.cleanup().await;
}

#[tokio::test]
async fn authorized_request_uses_a_fresh_ttl_then_expires() {
    let context = TestContext::new(true).await;
    let request = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    context.capability.advance(Duration::minutes(9));
    context
        .manager
        .authorize_request(context.user_id, &request.code)
        .await
        .unwrap();
    context.capability.advance(Duration::minutes(9));
    assert!(
        context
            .manager
            .get_authorized_request(&request.secret)
            .await
            .is_ok()
    );
    context.capability.advance(Duration::minutes(2));
    assert!(matches!(
        context
            .manager
            .get_authorized_request(&request.secret)
            .await,
        Err(QuickConnectError::NotFound)
    ));
    context.cleanup().await;
}

#[tokio::test]
async fn concurrent_authorize_has_exactly_one_success_and_one_device_session() {
    let context = TestContext::new(true).await;
    let authorization = context.authorization();
    let request = context.manager.try_connect(&authorization).await.unwrap();
    let (first, second) = tokio::join!(
        context
            .manager
            .authorize_request(context.user_id, &request.code),
        context
            .manager
            .authorize_request(context.user_id, &request.code)
    );
    let results = [first, second];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(QuickConnectError::AlreadyAuthorized)))
            .count(),
        1
    );

    let devices = DeviceRepository::new(context.database.clone())
        .query(&DeviceQuery {
            user_id: Some(context.user_id),
            device_id: Some(authorization.device_id),
            ..DeviceQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(devices.total_record_count, 1);
    assert_eq!(devices.items.len(), 1);
    assert!(devices.items[0].is_active);
    context.cleanup().await;
}

#[tokio::test]
async fn failed_device_session_rolls_back_authorization_state() {
    let context = TestContext::new(true).await;
    let request = context
        .manager
        .try_connect(&context.authorization())
        .await
        .unwrap();
    let missing_user = Uuid::new_v4();

    assert!(matches!(
        context
            .manager
            .authorize_request(missing_user, &request.code)
            .await,
        Err(QuickConnectError::Store(QuickConnectStoreError::Device(_)))
    ));
    let status = context
        .manager
        .check_request_status(&request.secret)
        .await
        .unwrap();
    assert!(!status.authenticated);
    assert!(matches!(
        context
            .manager
            .get_authorized_request(&request.secret)
            .await,
        Err(QuickConnectError::NotFound)
    ));
    context.cleanup().await;
}

async fn insert_user(database: &DatabaseConnection, user_id: Uuid) {
    let now = Utc::now();
    user::ActiveModel {
        id: Set(user_id),
        username: Set(format!("QuickConnect-{user_id}")),
        normalized_username: Set(format!("QUICKCONNECT-{user_id}")),
        password_hash: Set(None),
        must_update_password: Set(false),
        enable_local_password: Set(false),
        invalid_login_attempt_count: Set(0),
        login_attempts_before_lockout: Set(-1),
        is_administrator: Set(false),
        is_hidden: Set(false),
        is_disabled: Set(false),
        enable_auto_login: Set(false),
        last_login_date: Set(None),
        last_activity_date: Set(None),
        authentication_provider_id: Set(
            "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider".to_owned(),
        ),
        password_reset_provider_id: Set(
            "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider".to_owned(),
        ),
        policy: Set(json!({})),
        preferences: Set(json!({})),
        row_version: Set(1),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(database)
    .await
    .expect("Quick Connect test user must insert");
}

async fn unused_code(database: &DatabaseConnection, reserved: &[String]) -> String {
    loop {
        let candidate = (100_000 + Uuid::new_v4().as_u128() % 900_000).to_string();
        if !reserved.contains(&candidate)
            && quick_connect::Entity::find()
                .filter(quick_connect::Column::Code.eq(&candidate))
                .one(database)
                .await
                .expect("unused Quick Connect code lookup must succeed")
                .is_none()
        {
            return candidate;
        }
    }
}

fn next_secret() -> String {
    let first = Uuid::new_v4().simple().to_string();
    let second = Uuid::new_v4().simple().to_string();
    format!("{first}{second}").to_uppercase()
}
