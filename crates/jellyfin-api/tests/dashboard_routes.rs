use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{DashboardPage, UserService};
use jellyfin_data::{DeviceRepository, NewDevice, entities::user};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Dashboard Tests\", DeviceId=\"dashboard-tests\", Device=\"Test\", Version=\"1.0\"";
const TEST_PAGE: &str =
    include_str!("../../../jellyfin/tests/Jellyfin.Server.Integration.Tests/TestPage.html");
const TEST_PLUGIN_ID: Uuid = Uuid::from_u128(0x2d35_0a13_0bf7_4b61_859c_d5e6_01b5_facf);

#[tokio::test]
async fn official_public_dashboard_configuration_page_contract() {
    let fixture = Fixture::new().await;
    for uri in [
        "/web/ConfigurationPage?name=ThisPageDoesntExists",
        "/web/ConfigurationPage?name=BrokenPage",
        "/web/ConfigurationPage",
    ] {
        assert_eq!(
            fixture.get(uri, None, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    let response = fixture
        .get("/web/ConfigurationPage?name=TestPlugin", None, None)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH],
        TEST_PAGE.len().to_string()
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        TEST_PAGE.as_bytes()
    );
    fixture.cleanup().await;
}

#[tokio::test]
async fn official_configuration_pages_contract_and_elevation() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .get("/web/ConfigurationPages", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get("/web/ConfigurationPages", Some(&fixture.user_token), None,)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let pages = fixture
        .json("/web/ConfigurationPages", &fixture.admin_token)
        .await;
    let test_plugin = pages
        .as_array()
        .unwrap()
        .iter()
        .find(|page| page["Name"] == "TestPlugin")
        .unwrap();
    assert_eq!(test_plugin["EnableInMainMenu"], false);
    assert_eq!(test_plugin["DisplayName"], "Test Plugin");
    assert_eq!(test_plugin["PluginId"], TEST_PLUGIN_ID.to_string());

    let response = fixture
        .get(
            "/web/ConfigurationPages?enableInMainMenu=true",
            Some(&fixture.admin_token),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json; charset=utf-8"
    );
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body, serde_json::json!([]));
    fixture.cleanup().await;
}

#[tokio::test]
async fn dashboard_pages_stream_ranges_and_block_resource_escape() {
    let fixture = Fixture::new().await;
    let range_start = 10;
    let range_end = 31;
    let response = fixture
        .get(
            "/web/ConfigurationPage?name=testplugin",
            None,
            Some(&format!("bytes={range_start}-{range_end}")),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[header::CONTENT_RANGE],
        format!("bytes {range_start}-{range_end}/{}", TEST_PAGE.len())
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        TEST_PAGE.as_bytes()[range_start..=range_end]
    );

    let mut malicious_names = vec![
        "Traversal",
        "Absolute",
        "Directory",
        "..%2Fsecret.html",
        "..%5Csecret.html",
        "%2Fetc%2Fpasswd",
        "%00TestPlugin",
    ];
    #[cfg(unix)]
    malicious_names.push("Symlink");
    for name in malicious_names {
        let uri = format!("/web/ConfigurationPage?name={name}");
        assert_eq!(
            fixture.get(&uri, None, None).await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&fixture.secret_path).unwrap(),
        "secret"
    );
    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    user_id: Uuid,
    admin_token: String,
    user_token: String,
    temp_root: std::path::PathBuf,
    secret_path: std::path::PathBuf,
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
        let admin = users
            .create_initial_administrator(&format!("dashboard-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("dashboard-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("dashboard-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("dashboard-user-{suffix}")).await;

        let temp_root = std::env::temp_dir().join(format!("jellyfin-rust-dashboard-{suffix}"));
        let plugin_root = temp_root.join("plugin");
        let page_directory = plugin_root.join("pages");
        std::fs::create_dir_all(&page_directory).expect("plugin page directory");
        std::fs::write(page_directory.join("TestPage.html"), TEST_PAGE).expect("plugin page");
        let secret_path = temp_root.join("secret.html");
        std::fs::write(&secret_path, "secret").expect("outside secret");

        let mut test_page = DashboardPage::new("TestPlugin", &plugin_root, "pages/TestPage.html");
        test_page.display_name = Some("Test Plugin".to_owned());
        test_page.plugin_id = Some(TEST_PLUGIN_ID);
        let mut pages = vec![
            test_page,
            DashboardPage::new("BrokenPage", &plugin_root, "pages/missing.foobar"),
            DashboardPage::new("Traversal", &plugin_root, "../secret.html"),
            DashboardPage::new("Absolute", &plugin_root, &secret_path),
            DashboardPage::new("Directory", &plugin_root, "pages"),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret_path, page_directory.join("escape.html"))
                .expect("escape symlink");
            pages.push(DashboardPage::new(
                "Symlink",
                &plugin_root,
                "pages/escape.html",
            ));
        }
        let state = AppState::new(
            database.clone(),
            "Dashboard Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_dashboard_pages(pages);
        Self {
            database,
            app: jellyfin_api::router(state),
            admin_id: admin.id,
            user_id: user.id,
            admin_token,
            user_token,
            temp_root,
            secret_path,
        }
    }

    async fn get(
        &self,
        uri: &str,
        token: Option<&str>,
        range: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn json(&self, uri: &str, token: &str) -> Value {
        let response = self.get(uri, Some(token), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("dashboard user cleanup");
        std::fs::remove_dir_all(&self.temp_root).expect("dashboard fixture cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Dashboard Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("dashboard session")
        .access_token
}
