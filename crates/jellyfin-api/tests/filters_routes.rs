use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice,
    entities::{base_item, item_value},
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Filters Tests\", DeviceId=\"filters-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_filters_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn filters2_returns_official_query_filter_shape() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request("/Items/Filters2", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let movie_filters = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Movie",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &movie_filters["Genres"],
        &[
            (&fixture.drama_genre, fixture.drama_genre_id),
            (&fixture.nested_genre, fixture.nested_genre_id),
        ],
    );
    assert_eq!(movie_filters["Tags"], Value::Array(Vec::new()));
    assert_eq!(movie_filters["AudioLanguages"], Value::Array(Vec::new()));
    assert_eq!(movie_filters["SubtitleLanguages"], Value::Array(Vec::new()));
    assert!(movie_filters.get("genres").is_none());

    let music_filters = body_json(
        fixture
            .request(
                "/Items/Filters2?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &music_filters["Genres"],
        &[(&fixture.music_genre, fixture.music_genre_id)],
    );

    let parent_scoped = body_json(
        fixture
            .request(
                &format!("/Items/Filters2?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &parent_scoped["Genres"],
        &[(&fixture.nested_genre, fixture.nested_genre_id)],
    );

    let direct_children = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?parentId={}&recursive=false",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &direct_children["Genres"],
        &[(&fixture.nested_genre, fixture.nested_genre_id)],
    );

    let trailer_parent_is_ignored = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?parentId={}&includeItemTypes=Trailer",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &trailer_parent_is_ignored["Genres"],
        &[(&fixture.trailer_genre, fixture.trailer_genre_id)],
    );

    assert_eq!(
        fixture
            .request(
                &format!("/Items/Filters2?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let admin_targeted = body_json(
        fixture
            .request(
                &format!(
                    "/Items/Filters2?userId={}&includeItemTypes=Movie",
                    fixture.user_id
                ),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_pairs(
        &admin_targeted["Genres"],
        &[
            (&fixture.drama_genre, fixture.drama_genre_id),
            (&fixture.nested_genre, fixture.nested_genre_id),
        ],
    );

    fixture.cleanup().await;
}

fn assert_pairs(value: &Value, expected: &[(&str, Uuid)]) {
    let items = value.as_array().expect("name-guid pairs");
    assert_eq!(items.len(), expected.len());
    let actual = items
        .iter()
        .map(|item| {
            (
                item["Name"].as_str().expect("name").to_owned(),
                item["Id"].as_str().expect("id").to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let expected = expected
        .iter()
        .map(|(name, id)| ((*name).to_owned(), id.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
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

#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    Device(&'a str),
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    user_id: Uuid,
    other_user_id: Uuid,
    parent_id: Uuid,
    user_token: String,
    admin_token: String,
    drama_genre: String,
    drama_genre_id: Uuid,
    music_genre: String,
    music_genre_id: Uuid,
    nested_genre: String,
    nested_genre_id: Uuid,
    trailer_genre: String,
    trailer_genre_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
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

        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("filters-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("filters-user-{suffix}"))
            .await
            .unwrap();
        let other_user = users
            .create(&format!("filters-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let movie = create_item(&items, "Movie", "Filter Movie", None, false).await;
        let audio = create_item(&items, "Audio", "Filter Track", None, false).await;
        let trailer = create_item(&items, "Trailer", "Filter Trailer", None, false).await;
        let parent = create_item(&items, "Folder", "Filter Parent", None, true).await;
        let nested_movie = create_item(
            &items,
            "Movie",
            "Nested Filter Movie",
            Some(parent.id),
            false,
        )
        .await;

        let values = ItemValueRepository::new(database.clone());
        let drama_genre = format!("Drama {suffix}");
        let drama = values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("movie genre");
        let music_genre = format!("Electronic {suffix}");
        let music = values
            .link(audio.id, item_value::ItemValueType::Genre, &music_genre)
            .await
            .expect("music genre");
        let nested_genre = format!("Nested {suffix}");
        let nested = values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Genre,
                &nested_genre,
            )
            .await
            .expect("nested genre");
        let trailer_genre = format!("Trailer {suffix}");
        let trailer_value = values
            .link(trailer.id, item_value::ItemValueType::Genre, &trailer_genre)
            .await
            .expect("trailer genre");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Filters Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            parent_id: parent.id,
            user_token,
            admin_token,
            drama_genre,
            drama_genre_id: drama.item_value_id,
            music_genre,
            music_genre_id: music.item_value_id,
            nested_genre,
            nested_genre_id: nested.item_value_id,
            trailer_genre,
            trailer_genre_id: trailer_value.item_value_id,
        }
    }

    async fn request(&self, uri: &str, credential: Credential<'_>) -> axum::response::Response {
        let mut request = Request::get(uri);
        if let Credential::Device(token) = credential {
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

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
) -> base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    repository.create(item).await.expect("base item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Filters Tests",
            "1.0",
            "Test",
            format!("filters-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
