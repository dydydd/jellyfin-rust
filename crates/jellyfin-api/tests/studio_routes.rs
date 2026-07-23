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

const AUTHORIZATION: &str = "MediaBrowser Client=\"Studio Tests\", DeviceId=\"studio-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_studio_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn studio_routes_match_official_studio_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Studios", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/Studios/Pixar", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let studios = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(
        &studios,
        &[&fixture.alpha_studio, &fixture.beta_studio],
        5,
        0,
    );

    let paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?startIndex=1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&paged, &[&fixture.beta_studio, &fixture.gamma_studio], 5, 1);

    let searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?searchTerm={}", encoded(&fixture.beta_studio)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&searched, &[&fixture.beta_studio], 1, 0);

    let prefixed = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Studios?nameStartsWith={}",
                    encoded(&fixture.gamma_studio[..6])
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&prefixed, &[&fixture.gamma_studio], 1, 0);

    let favorite = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?isFavorite=true",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&favorite, &[&fixture.alpha_studio], 1, 0);

    let folder_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?parentId={}", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&folder_scoped, &[&fixture.gamma_studio], 1, 0);

    let item_scoped = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?parentId={}", fixture.movie_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&item_scoped, &[&fixture.alpha_studio], 1, 0);

    let audio_filtered = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&audio_filtered, &[&fixture.music_studio], 1, 0);

    let no_total = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?limit=1&enableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().expect("items").len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 1);

    let studio = body_json(
        fixture
            .request(
                Method::GET,
                &studio_route(&fixture.alpha_studio),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(studio["Id"], fixture.alpha_studio_id.simple().to_string());
    assert_eq!(studio["Name"], fixture.alpha_studio);
    assert_eq!(studio["Type"], "Studio");
    assert_eq!(
        studio["PresentationUniqueKey"],
        format!("Studio-{}", fixture.alpha_studio)
    );
    assert!(studio.get("item_type").is_none());

    let missing = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios/Missing%20Studio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(missing["Name"], "Missing Studio");
    assert_eq!(missing["Type"], "Studio");
    assert_eq!(missing["PresentationUniqueKey"], "Studio-Missing Studio");
    assert_ne!(missing["Id"], fixture.alpha_studio_id.simple().to_string());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?userId={}", fixture.other_user_id),
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
                &format!("/Studios?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["TotalRecordCount"], 5);

    fixture.cleanup().await;
}

fn assert_studios(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: usize,
) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("studio items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("studio name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "Studio"));
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
    alpha_studio_id: Uuid,
    alpha_studio: String,
    beta_studio: String,
    gamma_studio: String,
    music_studio: String,
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
            .create_initial_administrator(&format!("studio-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("studio-user-{suffix}"))
            .await
            .unwrap();
        let other_user = users
            .create(&format!("studio-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let movie = create_item(&items, "Movie", "Studio Movie", None, false).await;
        let trailer = create_item(&items, "Trailer", "Studio Trailer", None, false).await;
        let audio = create_item(&items, "Audio", "Studio Track", None, false).await;
        let parent = create_item(&items, "Folder", "Studio Parent", None, true).await;
        let nested_movie = create_item(
            &items,
            "Movie",
            "Nested Studio Movie",
            Some(parent.id),
            false,
        )
        .await;
        let extra_movie = create_item(&items, "Movie", "Extra Studio Movie", None, false).await;

        let values = ItemValueRepository::new(database.clone());
        let alpha_studio = format!("Alpha {suffix}");
        let beta_studio = format!("Beta {suffix}");
        let gamma_studio = format!("Gamma {suffix}");
        let music_studio = format!("Music {suffix}");
        let zeta_studio = format!("Zeta {suffix}");
        let alpha = values
            .link(movie.id, item_value::ItemValueType::Studios, &alpha_studio)
            .await
            .expect("alpha studio");
        values
            .link(trailer.id, item_value::ItemValueType::Studios, &beta_studio)
            .await
            .expect("beta studio");
        values
            .link(audio.id, item_value::ItemValueType::Studios, &music_studio)
            .await
            .expect("music studio");
        values
            .link(
                nested_movie.id,
                item_value::ItemValueType::Studios,
                &gamma_studio,
            )
            .await
            .expect("gamma studio");
        values
            .link(
                extra_movie.id,
                item_value::ItemValueType::Studios,
                &zeta_studio,
            )
            .await
            .expect("zeta studio");
        values
            .link(movie.id, item_value::ItemValueType::Studios, &alpha_studio)
            .await
            .expect("duplicate studio link");

        let studio_item = create_item(&items, "Studio", &alpha_studio, None, true).await;
        let user_data = UserDataRepository::new(database.clone());
        let mut linked_item_favorite =
            NewUserData::new(trailer.id, user.id, "LinkedStudioFavorite");
        linked_item_favorite.is_favorite = true;
        user_data
            .upsert(linked_item_favorite)
            .await
            .expect("linked item favorite user data");
        let mut studio_favorite = NewUserData::new(studio_item.id, user.id, "StudioFavorite");
        studio_favorite.is_favorite = true;
        user_data
            .upsert(studio_favorite)
            .await
            .expect("studio favorite user data");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Studio Test Server".to_owned(),
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
            alpha_studio_id: alpha.item_value_id,
            alpha_studio,
            beta_studio,
            gamma_studio,
            music_studio,
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
            "Studio Tests",
            "1.0",
            "Test",
            format!("studio-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn studio_route(name: &str) -> String {
    format!("/Studios/{}", encoded(name))
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
