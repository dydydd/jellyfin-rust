use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DeviceRepository, NewDevice, entities::user};
use jellyfin_model::{PluginInfo, PluginStatus};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
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

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    user_id: Uuid,
    user_token: String,
}

impl Fixture {
    async fn new(plugins: Vec<PluginInfo>) -> Self {
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
        let state = AppState::new(
            database.clone(),
            "Plugin Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_plugins(plugins);
        Self {
            database,
            app: jellyfin_api::router(state),
            user_id: user.id,
            user_token,
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

    async fn cleanup(self) {
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
