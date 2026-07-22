use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use jellyfin_api::AppState;
use jellyfin_controller::{InstalledPlugin, UserService};
use jellyfin_data::{
    ApiKeyRepository, DeviceRepository, NewDevice,
    entities::{api_key, user},
};
use jellyfin_model::{PluginInfo, PluginStatus};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Plugin Tests\", DeviceId=\"plugin-tests\", Device=\"Test\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn official_plugins_contract_requires_authentication_and_defaults_to_empty() {
    let fixture = Fixture::new(Vec::new()).await;

    assert_eq!(fixture.get(None).await.status(), StatusCode::UNAUTHORIZED);

    let response = fixture.get(Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert_eq!(body_json(response).await, json!([]));
    fixture.cleanup().await;
}

#[tokio::test]
async fn authenticated_users_receive_complete_name_ordered_plugin_metadata() {
    let first_id = Uuid::from_u128(0x2d35_0a13_0bf7_4b61_859c_d5e6_01b5_facf);
    let second_id = Uuid::from_u128(0x930f_1b2e_f0d9_4bc8_b98f_ea08_5274_3e4c);
    let fixture = Fixture::new(vec![
        plugin("Zulu", second_id, None, PluginStatus::Disabled),
        plugin("Alpha", first_id, Some("config.xml"), PluginStatus::Active),
    ])
    .await;

    let response = fixture.get(Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "Name": "Alpha",
                "Version": "1.2.3.4",
                "ConfigurationFileName": "config.xml",
                "Description": "Alpha 描述",
                "Id": first_id.simple().to_string(),
                "CanUninstall": true,
                "HasImage": true,
                "Status": "Active"
            },
            {
                "Name": "Zulu",
                "Version": "1.2.3.4",
                "Description": "Zulu 描述",
                "Id": second_id.simple().to_string(),
                "CanUninstall": true,
                "HasImage": true,
                "Status": "Disabled"
            }
        ])
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn anonymous_plugin_images_match_official_file_and_path_security_contract() {
    let PluginImageCases {
        _temporary_directory,
        plugin_root,
        installed_plugins,
        mut not_found_ids,
        valid_id,
        nested_id,
        normalized_id,
    } = plugin_image_cases();

    let fixture = Fixture::new_installed(installed_plugins).await;

    let response = fixture
        .request(&format!("/Plugins/{valid_id}/1.0/Image"), &[])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );
    assert_eq!(body_bytes(response).await, b"official-plugin-image");

    let nested_response = fixture
        .request(&format!("/Plugins/{nested_id}/1.0/Image"), &[])
        .await;
    assert_eq!(nested_response.status(), StatusCode::OK);
    assert_eq!(
        nested_response.headers()[header::CONTENT_TYPE],
        "image/jpeg"
    );
    assert_eq!(body_bytes(nested_response).await, b"nested-image");

    let normalized_response = fixture
        .request(&format!("/Plugins/{normalized_id}/1.0/Image"), &[])
        .await;
    assert_eq!(normalized_response.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(normalized_response).await,
        b"official-plugin-image"
    );

    assert_eq!(
        fixture
            .request(&format!("/Plugins/{valid_id}/2.0/Image"), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    not_found_ids.push(Uuid::from_u128(0xffff));
    for plugin_id in not_found_ids {
        assert_eq!(
            fixture
                .request(&format!("/Plugins/{plugin_id}/1.0/Image"), &[])
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "plugin image {plugin_id} should not resolve"
        );
    }

    let listed_plugins = body_json(fixture.get(Some(&fixture.user_token)).await).await;
    let listed_json = serde_json::to_string(&listed_plugins).unwrap();
    assert!(listed_json.contains(&valid_id.simple().to_string()));
    assert!(!listed_json.contains(plugin_root.to_string_lossy().as_ref()));
    assert!(!listed_json.contains("logo.png"));

    fixture.cleanup().await;
}

#[tokio::test]
async fn api_key_sources_authenticate_touch_activity_and_never_create_a_user_session() {
    let fixture = Fixture::new(Vec::new()).await;

    let response = fixture
        .request(
            "/Plugins",
            &[(
                header::AUTHORIZATION.as_str(),
                &format!("MediaBrowser Token=\"{}\"", fixture.api_key_token),
            )],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let touched = ApiKeyRepository::new(fixture.database.clone())
        .find_by_token(&fixture.api_key_token)
        .await
        .unwrap()
        .unwrap();
    assert!(touched.date_last_activity > fixture.api_key_last_activity);

    assert_eq!(
        fixture
            .request(
                "/Plugins",
                &[("x-emby-token", fixture.api_key_token.as_str())],
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(&format!("/Plugins?ApiKey={}", fixture.api_key_token), &[],)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(&format!("/Plugins?api_key={}", fixture.api_key_token), &[],)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(
                "/Users/Me",
                &[("x-emby-token", fixture.api_key_token.as_str())],
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn unknown_revoked_empty_and_lower_priority_keys_are_rejected() {
    let fixture = Fixture::new(Vec::new()).await;

    for headers in [
        vec![("x-emby-token", "unknown")],
        vec![(header::AUTHORIZATION.as_str(), "MediaBrowser Token=\"\"")],
        vec![
            ("x-emby-token", "unknown"),
            ("x-mediabrowser-token", fixture.api_key_token.as_str()),
        ],
    ] {
        assert_eq!(
            fixture.request("/Plugins", &headers).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        fixture
            .request(
                &format!("/Plugins?ApiKey=unknown&api_key={}", fixture.api_key_token),
                &[],
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    ApiKeyRepository::new(fixture.database.clone())
        .revoke(&fixture.api_key_token)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .request(
                "/Plugins",
                &[("x-emby-token", fixture.api_key_token.as_str())],
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn active_device_wins_over_same_token_api_key_before_admin_policy() {
    let fixture = Fixture::new(Vec::new()).await;
    let api_keys = ApiKeyRepository::new(fixture.database.clone());
    let mut key = api_keys
        .find_by_token(&fixture.api_key_token)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    key.access_token = Set(fixture.user_token.clone());
    key.update(&fixture.database).await.unwrap();

    assert_eq!(
        fixture
            .request(
                "/System/ActivityLog/Entries",
                &[("x-emby-token", fixture.user_token.as_str())],
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let devices = DeviceRepository::new(fixture.database.clone());
    let mut device = devices
        .find_by_token(&fixture.user_token)
        .await
        .unwrap()
        .unwrap();
    device.is_active = false;
    devices.update(device).await.unwrap();
    assert_eq!(
        fixture
            .request(
                "/System/ActivityLog/Entries",
                &[("x-emby-token", fixture.user_token.as_str())],
            )
            .await
            .status(),
        StatusCode::OK
    );

    fixture.cleanup().await;
}

fn plugin(
    name: &str,
    id: Uuid,
    configuration_file_name: Option<&str>,
    status: PluginStatus,
) -> PluginInfo {
    PluginInfo {
        name: name.to_owned(),
        version: "1.2.3.4".to_owned(),
        configuration_file_name: configuration_file_name.map(str::to_owned),
        description: format!("{name} 描述"),
        id,
        can_uninstall: true,
        has_image: true,
        status,
    }
}

fn installed_plugin(id: Uuid, directory: &Path, image_path: Option<&str>) -> InstalledPlugin {
    let mut info = plugin(
        &format!("Image Plugin {id}"),
        id,
        None,
        PluginStatus::Active,
    );
    "1.0".clone_into(&mut info.version);
    InstalledPlugin::new(info, directory, image_path.map(str::to_owned))
}

struct PluginImageCases {
    _temporary_directory: TempDirectory,
    plugin_root: PathBuf,
    installed_plugins: Vec<InstalledPlugin>,
    not_found_ids: Vec<Uuid>,
    valid_id: Uuid,
    nested_id: Uuid,
    normalized_id: Uuid,
}

fn plugin_image_cases() -> PluginImageCases {
    let temporary_directory = TempDirectory::new();
    let plugin_root = temporary_directory.path().join("plugin");
    let sibling_root = temporary_directory.path().join("plugin-evil");
    let outside_file = temporary_directory.path().join("outside.png");
    fs::create_dir_all(plugin_root.join("nested")).unwrap();
    fs::create_dir_all(&sibling_root).unwrap();
    fs::write(plugin_root.join("logo.png"), b"official-plugin-image").unwrap();
    fs::write(plugin_root.join("nested/cover.jpg"), b"nested-image").unwrap();
    fs::write(sibling_root.join("logo.png"), b"sibling-image").unwrap();
    fs::write(&outside_file, b"outside-image").unwrap();

    let valid_id = Uuid::from_u128(0x100);
    let nested_id = Uuid::from_u128(0x101);
    let normalized_id = Uuid::from_u128(0x102);
    let missing_id = Uuid::from_u128(0x103);
    let traversal_id = Uuid::from_u128(0x104);
    let nested_traversal_id = Uuid::from_u128(0x105);
    let sibling_id = Uuid::from_u128(0x106);
    let absolute_id = Uuid::from_u128(0x107);
    let null_id = Uuid::from_u128(0x108);
    let empty_id = Uuid::from_u128(0x109);
    let whitespace_id = Uuid::from_u128(0x10a);
    let mut installed_plugins = vec![
        installed_plugin(valid_id, &plugin_root, Some("logo.png")),
        installed_plugin(nested_id, &plugin_root, Some("nested/cover.jpg")),
        installed_plugin(normalized_id, &plugin_root, Some("unused/../logo.png")),
        installed_plugin(missing_id, &plugin_root, Some("does-not-exist.png")),
        installed_plugin(traversal_id, &plugin_root, Some("../../../../etc/passwd")),
        installed_plugin(
            nested_traversal_id,
            &plugin_root,
            Some("subdir/../../../../etc/passwd"),
        ),
        installed_plugin(sibling_id, &plugin_root, Some("../plugin-evil/logo.png")),
        installed_plugin(
            absolute_id,
            &plugin_root,
            Some(outside_file.to_string_lossy().as_ref()),
        ),
        installed_plugin(null_id, &plugin_root, None),
        installed_plugin(empty_id, &plugin_root, Some("")),
        installed_plugin(whitespace_id, &plugin_root, Some("   ")),
    ];
    let mut not_found_ids = vec![
        missing_id,
        traversal_id,
        nested_traversal_id,
        sibling_id,
        absolute_id,
        null_id,
        empty_id,
        whitespace_id,
    ];
    add_symlink_cases(
        &temporary_directory,
        &plugin_root,
        &outside_file,
        &mut installed_plugins,
        &mut not_found_ids,
    );

    PluginImageCases {
        _temporary_directory: temporary_directory,
        plugin_root,
        installed_plugins,
        not_found_ids,
        valid_id,
        nested_id,
        normalized_id,
    }
}

#[cfg(unix)]
fn add_symlink_cases(
    temporary_directory: &TempDirectory,
    plugin_root: &Path,
    outside_file: &Path,
    installed_plugins: &mut Vec<InstalledPlugin>,
    not_found_ids: &mut Vec<Uuid>,
) {
    use std::os::unix::fs::symlink;

    let outside_directory = temporary_directory.path().join("outside-directory");
    fs::create_dir_all(&outside_directory).unwrap();
    fs::write(
        outside_directory.join("logo.png"),
        b"outside-directory-image",
    )
    .unwrap();
    symlink(outside_file, plugin_root.join("linked.png")).unwrap();
    symlink(&outside_directory, plugin_root.join("linked-directory")).unwrap();

    let linked_file_id = Uuid::from_u128(0x10b);
    let linked_directory_id = Uuid::from_u128(0x10c);
    installed_plugins.push(installed_plugin(
        linked_file_id,
        plugin_root,
        Some("linked.png"),
    ));
    installed_plugins.push(installed_plugin(
        linked_directory_id,
        plugin_root,
        Some("linked-directory/logo.png"),
    ));
    not_found_ids.extend([linked_file_id, linked_directory_id]);
}

#[cfg(not(unix))]
fn add_symlink_cases(
    _temporary_directory: &TempDirectory,
    _plugin_root: &Path,
    _outside_file: &Path,
    _installed_plugins: &mut Vec<InstalledPlugin>,
    _not_found_ids: &mut Vec<Uuid>,
) {
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    user_id: Uuid,
    user_token: String,
    api_key_token: String,
    api_key_last_activity: chrono::DateTime<Utc>,
    api_key_id: i64,
}

impl Fixture {
    async fn new(plugins: Vec<PluginInfo>) -> Self {
        Self::configured(move |state| state.with_plugins(plugins)).await
    }

    async fn new_installed(plugins: Vec<InstalledPlugin>) -> Self {
        Self::configured(move |state| state.with_installed_plugins(plugins)).await
    }

    async fn configured(configure: impl FnOnce(AppState) -> AppState) -> Self {
        let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");
        let suffix = Uuid::new_v4().simple().to_string();
        let user = UserService::new(database.clone())
            .create(&format!("plugin-user-{suffix}"))
            .await
            .expect("user creation");
        let user_token = session(&DeviceRepository::new(database.clone()), user.id, &suffix).await;
        let api_keys = ApiKeyRepository::new(database.clone());
        let api_key = api_keys
            .create(&format!("plugin-key-{suffix}"))
            .await
            .expect("API key creation");
        let api_key_last_activity = Utc::now() - Duration::hours(1);
        api_keys
            .touch(&api_key.access_token, api_key_last_activity)
            .await
            .expect("API key timestamp setup");
        let state = configure(AppState::new(
            database.clone(),
            "Plugin Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app: jellyfin_api::router(state),
            user_id: user.id,
            user_token,
            api_key_token: api_key.access_token,
            api_key_last_activity,
            api_key_id: api_key.id,
        }
    }

    async fn get(&self, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get("/Plugins");
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn request(&self, uri: &str, headers: &[(&str, &str)]) -> axum::response::Response {
        let mut request = Request::get(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
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
            .expect("test API key cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.eq(self.user_id))
            .exec(&self.database)
            .await
            .expect("test user cleanup");
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Plugin Tests",
            "1.0",
            "Test",
            format!("plugin-tests-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
        .await
        .unwrap()
        .to_vec()
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("jellyfin-plugin-api-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
