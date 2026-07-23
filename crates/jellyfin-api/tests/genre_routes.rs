use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewUserData, UserDataRepository,
    entities::{base_item, item_value},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Genre Tests\", DeviceId=\"genre-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_genre_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn genre_routes_match_official_generic_genre_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Genres", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/Genres/Drama", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let genres = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?sortBy=SortName&sortOrder=Descending&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &genres,
        &[&fixture.parent_genre, &fixture.nested_genre],
        5,
        0,
    );

    let paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?startIndex=1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &paged,
        &[&fixture.drama_genre, &fixture.slug_genre_name],
        5,
        1,
    );

    let searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?searchTerm={}", encoded(&fixture.drama_genre)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&searched, &[&fixture.drama_genre], 1, 0);

    let prefixed = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Genres?nameStartsWith={}",
                    encoded(&fixture.comedy_genre[..6])
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&prefixed, &[&fixture.comedy_genre], 1, 0);

    let favorite = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?isFavorite=true",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&favorite, &[&fixture.comedy_genre], 1, 0);

    let parent_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(
        &parent_scoped,
        &[&fixture.nested_genre, &fixture.parent_genre],
        2,
        0,
    );

    let item_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?parentId={}", fixture.movie_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&item_scoped, &[&fixture.drama_genre], 1, 0);

    let audio_filtered = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_genres(&audio_filtered, &[], 0, 0);

    let no_total = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres?limit=1&enableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().expect("items").len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 1);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Genres?sortOrder=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Genres?sortBy=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let drama = body_json(
        fixture
            .request(
                Method::GET,
                &genre_route(&fixture.drama_genre),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(drama["Id"], fixture.drama_genre_id.simple().to_string());
    assert_eq!(drama["Name"], fixture.drama_genre);
    assert_eq!(drama["Type"], "Genre");
    assert_eq!(
        drama["PresentationUniqueKey"],
        format!("Genre-{}", fixture.drama_genre)
    );
    assert!(drama.get("item_type").is_none());

    let slug = body_json(
        fixture
            .request(
                Method::GET,
                &genre_route(&fixture.slug_route_name),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(slug["Id"], fixture.slug_genre_id.simple().to_string());
    assert_eq!(slug["Name"], fixture.slug_genre_name);

    let missing = body_json(
        fixture
            .request(
                Method::GET,
                "/Genres/Missing%20Genre",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["Name"], "Missing Genre");
    assert_eq!(missing["Type"], "Genre");
    assert_ne!(missing["Id"], fixture.drama_genre_id.simple().to_string());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let admin_targeted = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Genres?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["TotalRecordCount"], 5);

    fixture.cleanup().await;
}

fn assert_genres(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: usize,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("genre items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("genre name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "Genre"));
    assert!(items.iter().all(|item| item["IsFolder"] == true));
    assert!(body.get("items").is_none());
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
    movie_id: Uuid,
    parent_id: Uuid,
    user_token: String,
    admin_token: String,
    drama_genre_id: Uuid,
    drama_genre: String,
    comedy_genre: String,
    parent_genre: String,
    nested_genre: String,
    slug_genre_id: Uuid,
    slug_genre_name: String,
    slug_route_name: String,
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
            .create_initial_administrator(&format!("genre-admin-{suffix}"))
            .await
            .unwrap();
        let user = users.create(&format!("genre-user-{suffix}")).await.unwrap();
        let other_user = users
            .create(&format!("genre-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let movie = create_item(&items, "Movie", "Drama Movie", None, false).await;
        let trailer = create_item(&items, "Trailer", "Funny Trailer", None, false).await;
        let audio = create_item(&items, "Audio", "Music Track", None, false).await;
        let parent = create_item(&items, "Folder", "Genre Parent", None, true).await;
        let parent_movie =
            create_item(&items, "Movie", "Parent Movie", Some(parent.id), false).await;
        let nested = create_item(&items, "Folder", "Nested", Some(parent.id), true).await;
        let nested_movie =
            create_item(&items, "Movie", "Nested Movie", Some(nested.id), false).await;
        let slug_movie = create_item(&items, "Movie", "Slug Movie", None, false).await;

        let values = ItemValueRepository::new(database.clone());
        let drama_genre = format!("Drama {suffix}");
        let comedy_genre = format!("Comedy {suffix}");
        let parent_genre = format!("Parent {suffix}");
        let nested_genre = format!("Nested {suffix}");
        let music_genre = format!("Electronic {suffix}");
        let drama = values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("drama genre");
        values
            .link(trailer.id, item_value::ItemValueType::Genre, &comedy_genre)
            .await
            .expect("comedy genre");
        values
            .link(audio.id, item_value::ItemValueType::Genre, &music_genre)
            .await
            .expect("music genre");
        values
            .link(
                parent_movie.id,
                item_value::ItemValueType::Genre,
                &parent_genre,
            )
            .await
            .expect("parent genre");
        values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Genre,
                &nested_genre,
            )
            .await
            .expect("nested genre");
        values
            .link(movie.id, item_value::ItemValueType::Genre, &drama_genre)
            .await
            .expect("duplicate genre link");

        let slug_genre_name = format!("Left/Right {suffix}");
        let slug_genre = values
            .link(
                slug_movie.id,
                item_value::ItemValueType::Genre,
                &slug_genre_name,
            )
            .await
            .expect("slug genre");
        let slug_route_name = slug_genre_name.replace('/', "-");

        let mut favorite = NewUserData::new(trailer.id, user.id, "GenreFavorite");
        favorite.is_favorite = true;
        UserDataRepository::new(database.clone())
            .upsert(favorite)
            .await
            .expect("favorite user data");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Genre Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            user_id: user.id,
            other_user_id: other_user.id,
            movie_id: movie.id,
            parent_id: parent.id,
            user_token,
            admin_token,
            drama_genre_id: drama.item_value_id,
            drama_genre,
            comedy_genre,
            parent_genre,
            nested_genre,
            slug_genre_id: slug_genre.item_value_id,
            slug_genre_name,
            slug_route_name,
        }
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        credential: Credential<'_>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
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
            "Genre Tests",
            "1.0",
            "Test",
            format!("genre-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn genre_route(name: &str) -> String {
    format!("/Genres/{}", encoded(name))
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
