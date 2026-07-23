use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    DeviceRepository, NewDevice,
    entities::{device, quick_connect, user},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const INIT_AUTHORIZATION: &str = "MediaBrowser Client=\"Quick Connect App\", DeviceId=\"quick-connect-device\", Device=\"Living Room\", Version=\"1.0\"";
const USER_AUTHORIZATION: &str = "MediaBrowser Client=\"Quick Connect Authorizer\", DeviceId=\"quick-connect-authorizer\", Device=\"Browser\", Version=\"1.0\"";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn quick_connect_routes_authorize_and_authenticate_from_postgres() {
    let fixture = Fixture::new().await;
    assert_enabled_and_missing_client_metadata(&fixture).await;
    let (secret, code) = initiate_quick_connect(&fixture).await;
    assert_pending_request(&fixture, &secret, &code).await;
    authorize_quick_connect(&fixture, &code).await;
    assert_connected_request(&fixture, &secret).await;
    let access_token = authenticate_with_quick_connect(&fixture, &secret).await;
    assert_authenticated_access_token(&fixture, &access_token).await;
    fixture.cleanup().await;
}

async fn assert_enabled_and_missing_client_metadata(fixture: &Fixture) {
    assert_eq!(
        fixture
            .request("GET", "/QuickConnect/Enabled", None, Body::empty())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        body_json(
            fixture
                .request("GET", "/QuickConnect/Enabled", None, Body::empty())
                .await
        )
        .await,
        json!(true)
    );
    assert_eq!(
        fixture
            .request("POST", "/QuickConnect/Initiate", None, Body::empty())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

async fn initiate_quick_connect(fixture: &Fixture) -> (String, String) {
    let initiated = body_json(
        fixture
            .request(
                "POST",
                "/QuickConnect/Initiate",
                Some(INIT_AUTHORIZATION),
                Body::empty(),
            )
            .await,
    )
    .await;
    let secret = initiated["Secret"].as_str().expect("secret").to_owned();
    let code = initiated["Code"].as_str().expect("code").to_owned();
    assert_eq!(secret.len(), 64);
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
    assert_eq!(initiated["Authenticated"], false);
    assert_eq!(initiated["DeviceName"], "Living Room");
    (secret, code)
}

async fn assert_pending_request(fixture: &Fixture, secret: &str, code: &str) {
    let pending = body_json(
        fixture
            .request(
                "GET",
                &format!("/QuickConnect/Connect?secret={secret}"),
                None,
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(pending["Authenticated"], false);
    assert_eq!(pending["Code"], code);
}

async fn authorize_quick_connect(fixture: &Fixture, code: &str) {
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!("/QuickConnect/Authorize?code={code}"),
                None,
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &format!(
                    "/QuickConnect/Authorize?code={code}&userId={}",
                    fixture.other_user_id
                ),
                Some(&fixture.user_authorization),
                Body::empty(),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let authorized = body_json(
        fixture
            .request(
                "POST",
                &format!("/QuickConnect/Authorize?code={code}"),
                Some(&fixture.user_authorization),
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(authorized, json!(true));
}

async fn assert_connected_request(fixture: &Fixture, secret: &str) {
    let connected = body_json(
        fixture
            .request(
                "GET",
                &format!("/QuickConnect/Connect?secret={secret}"),
                None,
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(connected["Authenticated"], true);
}

async fn authenticate_with_quick_connect(fixture: &Fixture, secret: &str) -> String {
    assert_eq!(
        fixture
            .request(
                "POST",
                "/Users/AuthenticateWithQuickConnect",
                None,
                Body::from(json!({}).to_string()),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                "/Users/AuthenticateWithQuickConnect",
                None,
                Body::from(json!({ "Secret": "missing" }).to_string()),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let authenticated = body_json(
        fixture
            .request(
                "POST",
                "/Users/AuthenticateWithQuickConnect",
                None,
                Body::from(json!({ "Secret": secret }).to_string()),
            )
            .await,
    )
    .await;
    let access_token = authenticated["AccessToken"]
        .as_str()
        .expect("quick connect access token");
    assert!(!access_token.is_empty());
    assert_eq!(
        authenticated["User"]["Id"],
        fixture.user_id.simple().to_string()
    );
    assert_eq!(authenticated["SessionInfo"]["DeviceName"], "Living Room");
    assert_eq!(
        authenticated["SessionInfo"]["DeviceId"],
        "quick-connect-device"
    );
    access_token.to_owned()
}

async fn assert_authenticated_access_token(fixture: &Fixture, access_token: &str) {
    let current_user = body_json(
        fixture
            .request(
                "GET",
                "/Users/Me",
                Some(&format!("{INIT_AUTHORIZATION}, Token=\"{access_token}\"")),
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(current_user["Id"], fixture.user_id.simple().to_string());
}

struct Fixture {
    database: sea_orm::DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    user_authorization: String,
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
        let user = users
            .create(&format!("quick-connect-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("quick-connect-other-{suffix}"))
            .await
            .expect("other user creation");
        let authorizing_session = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Quick Connect Authorizer",
                "1.0",
                "Browser",
                format!("quick-connect-authorizer-{suffix}"),
            ))
            .await
            .expect("authorizer session creation");
        Self {
            database: database.clone(),
            app: jellyfin_api::router(AppState::new(
                database,
                "Quick Connect Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )),
            user_id: user.id,
            other_user_id: other_user.id,
            user_authorization: format!(
                "{USER_AUTHORIZATION}, Token=\"{}\"",
                authorizing_session.access_token
            ),
        }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        authorization: Option<&str>,
        body: Body,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        quick_connect::Entity::delete_many()
            .filter(quick_connect::Column::DeviceId.eq("quick-connect-device"))
            .exec(&self.database)
            .await
            .expect("quick connect cleanup");
        device::Entity::delete_many()
            .filter(device::Column::UserId.is_in([self.user_id, self.other_user_id]))
            .exec(&self.database)
            .await
            .expect("device cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.user_id, self.other_user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}
