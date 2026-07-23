use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, NewPerson,
    NewUserData, PersonRepository, UserDataRepository,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Persons Tests\", DeviceId=\"persons-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_persons_routes_";

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

#[tokio::test]
async fn persons_list_matches_official_persons_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.request("/Persons", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let listed = body_json(
        fixture
            .request("/Persons?limit=2", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &listed,
        &[&fixture.director_name, &fixture.nested_person_name],
        3,
        0,
    );

    let unlimited = body_json(
        fixture
            .request("/Persons?limit=0", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &unlimited,
        &[
            &fixture.director_name,
            &fixture.nested_person_name,
            &fixture.person_name,
        ],
        3,
        0,
    );

    let searched = body_json(
        fixture
            .request(
                &format!("/Persons?searchTerm={}", encoded(&fixture.person_name)),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&searched, &[&fixture.person_name], 1, 0);

    let prefixed = body_json(
        fixture
            .request(
                &format!(
                    "/Persons?nameStartsWith={}",
                    encoded(&fixture.director_name[..3])
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&prefixed, &[&fixture.director_name], 1, 0);

    let actors = body_json(
        fixture
            .request("/Persons?personTypes=Actor", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&actors, &[&fixture.person_name], 1, 0);

    let invalid_types = body_json(
        fixture
            .request("/Persons?personTypes=@@@", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(
        &invalid_types,
        &[
            &fixture.director_name,
            &fixture.nested_person_name,
            &fixture.person_name,
        ],
        3,
        0,
    );

    let mixed_types = body_json(
        fixture
            .request("/Persons?personTypes=Actor,@@@", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&mixed_types, &[&fixture.person_name], 1, 0);

    let non_actors = body_json(
        fixture
            .request(
                "/Persons?excludePersonTypes=Actor",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(
        &non_actors,
        &[&fixture.director_name, &fixture.nested_person_name],
        2,
        0,
    );

    let favorite = body_json(
        fixture
            .request("/Persons?filters=IsFavorite", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_people(&favorite, &[&fixture.person_name], 1, 0);

    let appears_in = body_json(
        fixture
            .request(
                &format!("/Persons?appearsInItemId={}", fixture.item_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&appears_in, &[&fixture.person_name], 1, 0);

    let parent_scoped = body_json(
        fixture
            .request(
                &format!("/Persons?parentId={}", fixture.parent_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&parent_scoped, &[&fixture.nested_person_name], 1, 0);

    let item_scoped = body_json(
        fixture
            .request(
                &format!("/Persons?parentId={}", fixture.item_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_people(&item_scoped, &[&fixture.person_name], 1, 0);

    let for_admin = format!("/Persons?userId={}", fixture.admin_id);
    assert_eq!(
        fixture
            .request(&for_admin, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let for_user = format!("/Persons?userId={}", fixture.user_id);
    assert_eq!(
        fixture
            .request(&for_user, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::OK
    );

    fixture.cleanup().await;
}

fn assert_people(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: usize,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("person items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("person name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "Person"));
    assert!(items.iter().all(|item| item["IsFolder"] == false));
    assert!(body.get("items").is_none());
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
    parent_id: Uuid,
    person_id: Uuid,
    person_name: String,
    director_name: String,
    nested_person_name: String,
    variant_name: String,
    tmdb_id: String,
}

impl Fixture {
    async fn new() -> Self {
        let (database_name, database) = test_database().await;
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
        let mut second_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        second_item.name = Some(format!("Persons Second Movie {suffix}"));
        second_item.sort_name = second_item.name.clone();
        let second_item = items.create(second_item).await.expect("base item creation");
        let mut parent = NewBaseItem::new(Uuid::new_v4(), "Folder");
        parent.name = Some(format!("Persons Parent {suffix}"));
        parent.sort_name = parent.name.clone();
        parent.is_folder = true;
        let parent = items.create(parent).await.expect("parent creation");
        let mut child_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        child_item.name = Some(format!("Persons Nested Movie {suffix}"));
        child_item.sort_name = child_item.name.clone();
        child_item.parent_id = Some(parent.id);
        let child_item = items.create(child_item).await.expect("child item creation");
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
        let director_name = format!("Ana {suffix}");
        people
            .link(
                second_item.id,
                NewPerson::new(director_name.clone()),
                "Director",
                None,
                Some(1),
                1,
            )
            .await
            .expect("director link");
        let nested_person_name = format!("Milo {suffix}");
        people
            .link(
                child_item.id,
                NewPerson::new(nested_person_name.clone()),
                "Writer",
                None,
                Some(2),
                2,
            )
            .await
            .expect("nested person link");
        let (one, two, three, four) = tokio::join!(
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
            people.upsert(NewPerson::new(person_name.clone())),
            people.upsert(NewPerson::new(variant_name.clone())),
        );
        for result in [one, two, three, four] {
            assert_eq!(result.expect("concurrent deduplication").id, person.id);
        }
        let mut favorite = NewUserData::new(item.id, user.id, "PersonFavorite");
        favorite.is_favorite = true;
        UserDataRepository::new(database.clone())
            .upsert(favorite)
            .await
            .expect("favorite user data");
        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Persons Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database_name,
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_id: item.id,
            parent_id: parent.id,
            person_id: person.id,
            person_name,
            director_name,
            nested_person_name,
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
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
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

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn test_database() -> (String, DatabaseConnection) {
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");
    administrator.close().await.unwrap();

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
    (database_name, database)
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
