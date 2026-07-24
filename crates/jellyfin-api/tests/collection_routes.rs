use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, LinkedChildRepository,
    NewBaseItem, NewDevice, entities::base_item,
};
use jellyfin_model::{AccessSchedule, DynamicDayOfWeek, UserPolicy};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_collection_routes_";
const AUTHORIZATION: &str = "MediaBrowser Client=\"Collection Tests\", Device=\"PostgreSQL\", DeviceId=\"collections\", Version=\"1.0\"";

#[tokio::test]
async fn collection_routes_match_official_policy_response_and_postgres_contract() {
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
        exercise_collection_routes(&task_database_name).await;
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

async fn exercise_collection_routes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    assert_authentication_and_policy(&fixture).await;
    let collection_id = assert_create_and_initial_order(&fixture).await;
    assert_add_remove_contract(&fixture, collection_id).await;
    assert_invalid_requests_and_atomicity(&fixture, collection_id).await;
    assert_elevated_identities(&fixture).await;
    fixture.database.close().await.unwrap();
}

async fn assert_authentication_and_policy(fixture: &Fixture) {
    for credential in [Credential::None, Credential::Device("bad-token")] {
        assert_eq!(
            fixture
                .request(Method::POST, "/Collections?name=Denied", credential)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for token in [&fixture.ordinary_token, &fixture.blocked_manager_token] {
        assert_eq!(
            fixture
                .request(
                    Method::POST,
                    "/Collections?name=Denied",
                    Credential::Device(token),
                )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

async fn assert_create_and_initial_order(fixture: &Fixture) -> Uuid {
    let route = format!(
        "/Collections?Name=My%20Collection&Ids={},{},{}&ParentId={}&IsLocked=true",
        fixture.second_id, fixture.first_id, fixture.second_id, fixture.root_id
    );
    let response = fixture
        .request(
            Method::POST,
            &route,
            Credential::Device(&fixture.manager_token),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collection_id = body["Id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert_eq!(body, json!({ "Id": collection_id }));

    let collection = BaseItemRepository::new(fixture.database.clone())
        .get(collection_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(collection.item_type, "BoxSet");
    assert_eq!(collection.name.as_deref(), Some("My Collection"));
    assert_eq!(collection.sort_name.as_deref(), Some("My Collection"));
    assert_eq!(collection.parent_id, Some(fixture.root_id));
    assert!(collection.is_folder);
    assert_eq!(collection.data, Some(json!({ "IsLocked": true })));

    let links = LinkedChildRepository::new(fixture.database.clone())
        .list(collection_id)
        .await
        .unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.second_id, fixture.first_id]
    );
    assert_eq!(
        links.iter().map(|link| link.sort_order).collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
    collection_id
}

async fn assert_add_remove_contract(fixture: &Fixture, collection_id: Uuid) {
    let route = format!(
        "/Collections/{collection_id}/Items?ids={},{},{}",
        fixture.first_id, fixture.third_id, fixture.third_id
    );
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &route,
                Credential::Device(&fixture.manager_token)
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let repository = LinkedChildRepository::new(fixture.database.clone());
    let links = repository.list(collection_id).await.unwrap();
    assert_eq!(
        links.iter().map(|link| link.child_id).collect::<Vec<_>>(),
        [fixture.second_id, fixture.first_id, fixture.third_id]
    );

    let route = format!(
        "/Collections/{collection_id}/Items?Ids={},{}",
        fixture.first_id,
        Uuid::new_v4()
    );
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &route,
                Credential::Device(&fixture.manager_token)
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        repository
            .list(collection_id)
            .await
            .unwrap()
            .iter()
            .map(|link| link.child_id)
            .collect::<Vec<_>>(),
        [fixture.second_id, fixture.third_id]
    );
}

async fn assert_invalid_requests_and_atomicity(fixture: &Fixture, collection_id: Uuid) {
    for (method, route) in [
        (Method::POST, format!("/Collections/{collection_id}/Items")),
        (
            Method::DELETE,
            format!("/Collections/{collection_id}/Items?ids=invalid"),
        ),
        (
            Method::POST,
            format!(
                "/Collections/{}/Items?ids={}",
                Uuid::new_v4(),
                fixture.first_id
            ),
        ),
        (
            Method::POST,
            format!(
                "/Collections/{}/Items?ids={}",
                fixture.first_id, fixture.second_id
            ),
        ),
        (
            Method::POST,
            format!("/Collections/{collection_id}/Items?ids={}", Uuid::new_v4()),
        ),
        (
            Method::POST,
            format!("/Collections/not-a-guid/Items?ids={}", fixture.first_id),
        ),
        (
            Method::POST,
            format!("/Collections/{collection_id}/Items?ids={collection_id}"),
        ),
    ] {
        assert_eq!(
            fixture
                .request(method, &route, Credential::Device(&fixture.manager_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "invalid collection request must be rejected: {route}"
        );
    }

    let before = box_sets_named(&fixture.database, "Must Roll Back").await;
    let missing_child = Uuid::new_v4();
    assert_eq!(
        fixture
            .request(
                Method::POST,
                &format!("/Collections?name=Must%20Roll%20Back&ids={missing_child}"),
                Credential::Device(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        box_sets_named(&fixture.database, "Must Roll Back").await,
        before
    );

    assert_eq!(
        fixture
            .request(
                Method::POST,
                &format!(
                    "/Collections?name=Missing%20Parent&parentId={}",
                    Uuid::new_v4()
                ),
                Credential::Device(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(box_sets_named(&fixture.database, "Missing Parent").await, 0);

    assert_eq!(
        fixture
            .request(
                Method::POST,
                "/Collections?name=Invalid%20Child&ids=not-a-guid",
                Credential::Device(&fixture.manager_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(box_sets_named(&fixture.database, "Invalid Child").await, 0);
}

async fn assert_elevated_identities(fixture: &Fixture) {
    for (name, credential) in [
        ("Admin Collection", Credential::Device(&fixture.admin_token)),
        ("API Key Collection", Credential::ApiKey(&fixture.api_key)),
    ] {
        let route = format!("/Collections?name={}", name.replace(' ', "%20"));
        let response = fixture.request(Method::POST, &route, credential).await;
        assert_eq!(response.status(), StatusCode::OK);
        let id = body_json(response).await["Id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let item = BaseItemRepository::new(fixture.database.clone())
            .get(id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.parent_id, Some(fixture.root_id));
    }
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    root_id: Uuid,
    first_id: Uuid,
    second_id: Uuid,
    third_id: Uuid,
    ordinary_token: String,
    manager_token: String,
    blocked_manager_token: String,
    admin_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 12,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database).await.unwrap();

        let users = UserService::new(database.clone());
        let ordinary = users.create("collection-ordinary").await.unwrap();
        let manager = users.create("collection-manager").await.unwrap();
        let blocked = users.create("collection-blocked").await.unwrap();
        let administrator = users
            .create_initial_administrator("collection-admin")
            .await
            .unwrap();
        users
            .update_policy(manager.id, &manager_policy(false))
            .await
            .unwrap();
        users
            .update_policy(blocked.id, &blocked_manager_policy())
            .await
            .unwrap();

        let devices = DeviceRepository::new(database.clone());
        let ordinary_token = session(&devices, ordinary.id, "ordinary").await;
        let manager_token = session(&devices, manager.id, "manager").await;
        let blocked_manager_token = session(&devices, blocked.id, "blocked").await;
        let admin_token = session(&devices, administrator.id, "admin").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create("collection-api-key")
            .await
            .unwrap()
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.unwrap();
        let first = create_movie(&items, root.id, "First movie").await;
        let second = create_movie(&items, root.id, "Second movie").await;
        let third = create_movie(&items, root.id, "Third movie").await;
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Collection Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            root_id: root.id,
            first_id: first.id,
            second_id: second.id,
            third_id: third.id,
            ordinary_token,
            manager_token,
            blocked_manager_token,
            admin_token,
            api_key,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        match credential {
            Credential::None => {}
            Credential::Device(token) | Credential::ApiKey(token) => {
                request = request.header(
                    header::AUTHORIZATION,
                    format!("{AUTHORIZATION}, Token=\"{token}\""),
                );
            }
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
    ApiKey(&'a str),
}

async fn create_movie(
    repository: &BaseItemRepository,
    parent_id: Uuid,
    name: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.parent_id = Some(parent_id);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.media_type = Some("Video".to_owned());
    repository.create(item).await.unwrap()
}

async fn session(repository: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Collection Tests",
            "1.0",
            "PostgreSQL",
            format!("collection-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn manager_policy(is_administrator: bool) -> UserPolicy {
    UserPolicy {
        is_administrator,
        enable_collection_management: true,
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    }
}

fn blocked_manager_policy() -> UserPolicy {
    UserPolicy {
        access_schedules: vec![AccessSchedule {
            day_of_week: DynamicDayOfWeek::Everyday,
            start_hour: 18.0,
            end_hour: 6.0,
        }],
        ..manager_policy(false)
    }
}

async fn box_sets_named(database: &DatabaseConnection, name: &str) -> usize {
    base_item::Entity::find()
        .filter(base_item::Column::ItemType.eq("BoxSet"))
        .filter(base_item::Column::Name.eq(name))
        .all(database)
        .await
        .unwrap()
        .len()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
