use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository};
use jellyfin_model::UserPolicy;
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_public_user_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const PERSISTENT_DEVICE: &str = "public-persistent-device";
const LEGACY_DEVICE: &str = "public-legacy-device";
const DEFAULT_CAPABILITIES_DEVICE: &str = "public-default-capabilities-device";

#[tokio::test]
async fn public_users_follow_startup_device_and_network_filters() {
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
        exercise_public_user_filters(&task_database_name).await;
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

async fn exercise_public_user_filters(database_name: &str) {
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

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("public-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let mut administrator_policy = policy(false, true, []);
    administrator_policy.is_administrator = true;
    let administrator = users
        .update_policy(administrator.id, &administrator_policy)
        .await
        .expect("public administrator policy")
        .0;
    let all_devices = create_public_user(
        &users,
        &format!("public-all-{suffix}"),
        policy(true, true, []),
    )
    .await;
    let allowed_device = create_public_user(
        &users,
        &format!("public-allowed-{suffix}"),
        policy(false, true, [PERSISTENT_DEVICE.to_ascii_uppercase()]),
    )
    .await;
    let denied_device = create_public_user(
        &users,
        &format!("public-denied-{suffix}"),
        policy(false, true, []),
    )
    .await;
    let local_only = create_public_user(
        &users,
        &format!("public-local-{suffix}"),
        policy(true, false, []),
    )
    .await;

    let devices = DeviceRepository::new(database.clone());
    create_device(&devices, administrator.id, PERSISTENT_DEVICE, true).await;
    create_device(&devices, administrator.id, LEGACY_DEVICE, false).await;
    devices
        .create_session(NewDevice::new(
            administrator.id,
            "Public User Tests",
            "1.0",
            "Test Device",
            DEFAULT_CAPABILITIES_DEVICE,
        ))
        .await
        .expect("default-capabilities device creation");

    let configuration = ServerConfigurationRepository::new(database.clone());
    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Public User Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_persistent_startup(configuration.clone()),
    );

    let all_names = names([
        &administrator.username,
        &all_devices.username,
        &allowed_device.username,
        &denied_device.username,
        &local_only.username,
    ]);
    let local_persistent_names = names([
        &administrator.username,
        &all_devices.username,
        &allowed_device.username,
        &local_only.username,
    ]);
    let remote_names = names([
        &administrator.username,
        &all_devices.username,
        &allowed_device.username,
        &denied_device.username,
    ]);
    let remote_persistent_names = names([
        &administrator.username,
        &all_devices.username,
        &allowed_device.username,
    ]);
    let unknown_persistent_names = names([
        &administrator.username,
        &all_devices.username,
        &local_only.username,
    ]);

    // Before startup completion Jellyfin only applies the visible/enabled
    // predicates, even when the request is remote and supplies a DeviceId.
    assert_eq!(
        public_user_names(&app, Some(PERSISTENT_DEVICE), remote_ip()).await,
        all_names
    );

    configuration
        .complete_startup()
        .await
        .expect("startup completion");

    assert_eq!(
        public_user_names(&app, Some(PERSISTENT_DEVICE), local_ip()).await,
        local_persistent_names
    );
    assert_eq!(
        public_user_names(&app, Some(DEFAULT_CAPABILITIES_DEVICE), local_ip()).await,
        unknown_persistent_names
    );
    assert_eq!(
        public_user_names(&app, None, remote_ip()).await,
        remote_names
    );
    assert_eq!(
        public_user_names(&app, Some(PERSISTENT_DEVICE), remote_ip()).await,
        remote_persistent_names
    );
    assert_eq!(
        public_user_names(&app, Some(LEGACY_DEVICE), local_ip()).await,
        all_names
    );
    assert_eq!(
        public_user_names(&app, Some("unknown-persistent-device"), local_ip()).await,
        unknown_persistent_names
    );

    database.close().await.expect("database pool cleanup");
}

async fn create_public_user(
    users: &UserService,
    name: &str,
    policy: UserPolicy,
) -> jellyfin_data::entities::user::Model {
    let user = users.create(name).await.expect("public user creation");
    users
        .update_policy(user.id, &policy)
        .await
        .expect("public user policy")
        .0
}

fn policy<const N: usize>(
    enable_all_devices: bool,
    enable_remote_access: bool,
    enabled_devices: [String; N],
) -> UserPolicy {
    UserPolicy {
        is_hidden: false,
        is_disabled: false,
        enable_all_devices,
        enable_remote_access,
        enabled_devices: enabled_devices.into(),
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

async fn create_device(
    devices: &DeviceRepository,
    user_id: Uuid,
    device_id: &str,
    supports_persistent_identifier: bool,
) {
    let device = devices
        .create_session(NewDevice::new(
            user_id,
            "Public User Tests",
            "1.0",
            "Test Device",
            device_id,
        ))
        .await
        .expect("device creation");
    assert_eq!(
        devices
            .update_capabilities_by_token(
                &device.access_token,
                json!({ "SupportsPersistentIdentifier": supports_persistent_identifier }),
            )
            .await
            .expect("device capability update"),
        1
    );
}

async fn public_user_names(
    app: &axum::Router,
    device_id: Option<&str>,
    remote_ip: IpAddr,
) -> BTreeSet<String> {
    let mut request =
        Request::get("/Users/Public").extension(ConnectInfo(SocketAddr::new(remote_ip, 12345)));
    if let Some(device_id) = device_id {
        request = request.header(
            header::AUTHORIZATION,
            format!(
                "MediaBrowser Client=\"Public User Tests\", DeviceId=\"{device_id}\", Device=\"Test\", Version=\"1.0\""
            ),
        );
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("public user request"))
        .await
        .expect("public user response");
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("public user response body"),
    )
    .expect("public user response JSON");
    body.as_array()
        .expect("public user array")
        .iter()
        .map(|user| user["Name"].as_str().expect("public user name").to_owned())
        .collect()
}

fn names<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.into_iter().map(str::to_owned).collect()
}

const fn local_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

const fn remote_ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10))
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
