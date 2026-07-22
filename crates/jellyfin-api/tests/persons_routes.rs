use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, NewBaseItem, NewDevice, NewPerson, PersonRepository,
    entities::user,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Persons Tests\", DeviceId=\"persons-tests\", Device=\"Test\", Version=\"1.0\"";

#[tokio::test]
async fn official_missing_person_is_not_found() {
    let fixture = Fixture::new().await;
    let response = fixture
        .request("/Persons/DoesntExist", Some(&fixture.user_token))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn person_returns_pascal_case_base_item_dto_for_unicode_clean_name() {
    let fixture = Fixture::new().await;
    let response = fixture
        .request(
            &person_route(&fixture.variant_name),
            Some(&fixture.user_token),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dto = body_json(response).await;
    assert_eq!(dto["Id"], fixture.person_id.simple().to_string());
    assert_eq!(dto["Name"], fixture.person_name);
    assert_eq!(dto["Type"], "Person");
    assert_eq!(dto["ProviderIds"]["Tmdb"], fixture.tmdb_id);
    assert_eq!(dto["IsFolder"], false);
    assert!(dto.get("item_type").is_none());
    assert!(dto.get("provider_ids").is_none());

    let exact = fixture
        .request(
            &person_route(&fixture.person_name),
            Some(&fixture.user_token),
        )
        .await;
    assert_eq!(exact.status(), StatusCode::OK);
    assert_eq!(body_json(exact).await["Id"], dto["Id"]);
    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_and_target_user_permissions_are_enforced() {
    let fixture = Fixture::new().await;
    let route = person_route(&fixture.person_name);
    assert_eq!(
        fixture.request(&route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let for_admin = format!("{route}?userId={}", fixture.admin_id);
    assert_eq!(
        fixture
            .request(&for_admin, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let for_user = format!("{route}?userId={}", fixture.user_id);
    assert_eq!(
        fixture
            .request(&for_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::OK
    );
    let missing_user = format!("{route}?userId={}", Uuid::new_v4());
    assert_eq!(
        fixture
            .request(&missing_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
    person_id: Uuid,
    person_name: String,
    variant_name: String,
    tmdb_id: String,
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
            .create_initial_administrator(&format!("persons-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("persons-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("persons-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("persons-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Persons Movie {suffix}"));
        item.sort_name = item.name.clone();
        let item = items.create(item).await.expect("base item creation");
        let people = PersonRepository::new(database.clone());
        let person_name = format!("Zoë 東京 {suffix}");
        let variant_name = format!("ZOE---東京---{suffix}");
        let tmdb_id = format!("person-{suffix}");
        let mut input = NewPerson::new(person_name.clone());
        input.provider_ids = json!({ "Tmdb": tmdb_id });
        let person = people
            .link(item.id, input, "Actor", Some("Lead"), Some(0), 0)
            .await
            .expect("person link");
        let (one, two, three, four) = tokio::join!(
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
        );
        for result in [one, two, three, four] {
            assert_eq!(result.expect("concurrent deduplication").id, person.id);
        }
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Persons Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_id: item.id,
            person_id: person.id,
            person_name,
            variant_name,
            tmdb_id,
        }
    }

    async fn request(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
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
        BaseItemRepository::new(self.database.clone())
            .delete(self.item_id)
            .await
            .expect("item cleanup");
        PersonRepository::new(self.database.clone())
            .delete(self.person_id)
            .await
            .expect("person cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
    }
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Persons Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

fn person_route(name: &str) -> String {
    format!("/Persons/{}", utf8_percent_encode(name, NON_ALPHANUMERIC))
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
