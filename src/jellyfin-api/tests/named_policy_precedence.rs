use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{DatabaseConfig, DeviceRepository, NewDevice, entities::user};
use jellyfin_model::UserPolicy;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Named Policy Tests\", Device=\"Test\", DeviceId=\"named-policy-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_named_policy_precedence_";

#[tokio::test]
async fn named_policies_precede_malformed_path_and_query_binding() {
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
        exercise_named_policy_precedence(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_named_policy_precedence(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let item_id = Uuid::new_v4();
    let cases = [
        (Method::GET, "/Items/not-a-guid/Download".to_owned()),
        (Method::GET, "/items/not-a-guid/download".to_owned()),
        (
            Method::GET,
            format!("/Items/{item_id}/RemoteSearch/Subtitles/eng?isPerfectMatch=not-a-bool"),
        ),
        (
            Method::GET,
            format!("/items/{item_id}/remotesearch/subtitles/eng?isperfectmatch=not-a-bool"),
        ),
        (
            Method::POST,
            "/Items/not-a-guid/RemoteSearch/Subtitles/provider-id".to_owned(),
        ),
        (
            Method::POST,
            "/items/not-a-guid/remotesearch/subtitles/provider-id".to_owned(),
        ),
        (Method::POST, "/Videos/not-a-guid/Subtitles".to_owned()),
        (Method::POST, "/videos/not-a-guid/subtitles".to_owned()),
        (Method::POST, "/Audio/not-a-guid/Lyrics".to_owned()),
        (Method::POST, "/audio/not-a-guid/lyrics".to_owned()),
        (Method::DELETE, "/Audio/not-a-guid/Lyrics".to_owned()),
        (Method::DELETE, "/audio/not-a-guid/lyrics".to_owned()),
        (
            Method::GET,
            "/Audio/not-a-guid/RemoteSearch/Lyrics".to_owned(),
        ),
        (
            Method::GET,
            "/audio/not-a-guid/remotesearch/lyrics".to_owned(),
        ),
        (
            Method::POST,
            "/Audio/not-a-guid/RemoteSearch/Lyrics/provider-id".to_owned(),
        ),
        (
            Method::POST,
            "/audio/not-a-guid/remotesearch/lyrics/provider-id".to_owned(),
        ),
    ];

    for (method, route) in cases {
        assert_eq!(
            fixture
                .send(method.clone(), &route, &fixture.restricted_token)
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "named policy must run before malformed binding for {method} {route}",
        );
        assert_eq!(
            fixture
                .send(method.clone(), &route, &fixture.administrator_token)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "permitted malformed request must still fail binding for {method} {route}",
        );
    }

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    restricted_id: Uuid,
    restricted_token: String,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 4,
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
            .create_initial_administrator(&format!("named-policy-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let restricted = users
            .create(&format!("named-policy-user-{suffix}"))
            .await
            .expect("restricted user creation");
        let mut policy: UserPolicy =
            serde_json::from_value(restricted.policy.clone()).expect("restricted user policy");
        policy.enable_content_downloading = false;
        policy.enable_subtitle_management = false;
        policy.enable_lyric_management = false;
        users
            .update_policy(restricted.id, &policy)
            .await
            .expect("restricted named-policy permissions");

        let devices = DeviceRepository::new(database.clone());
        let administrator_token = devices
            .create_session(NewDevice::new(
                administrator.id,
                "Named Policy Tests",
                "1.0",
                "Test",
                format!("named-policy-admin-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;
        let restricted_token = devices
            .create_session(NewDevice::new(
                restricted.id,
                "Named Policy Tests",
                "1.0",
                "Test",
                format!("named-policy-user-{suffix}"),
            ))
            .await
            .expect("restricted user session")
            .access_token;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Named Policy Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            restricted_id: restricted.id,
            restricted_token,
        }
    }

    async fn send(&self, method: Method, route: &str, token: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(route)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.restricted_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
