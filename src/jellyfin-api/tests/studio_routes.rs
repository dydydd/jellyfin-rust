#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemImageRepository, BaseItemImageType, BaseItemRepository, DatabaseConfig,
    DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage, NewDevice, NewUserData,
    UserDataRepository,
    entities::{base_item, item_value},
};
use jellyfin_model::UserPolicy;
use md5::{Digest, Md5};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use std::path::PathBuf;
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
    assert_eq!(
        fixture
            .request(Method::GET, "/studios", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/studios/Pixar", Credential::None)
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

    let lowercase_studios = body_json(
        fixture
            .request(
                Method::GET,
                "/studios?limit=1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&lowercase_studios, &[&fixture.alpha_studio], 5, 0);

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

    let pascal_paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?StartIndex=1&Limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(
        &pascal_paged,
        &[&fixture.beta_studio, &fixture.gamma_studio],
        5,
        1,
    );

    let negative_start = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?startIndex=-1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(
        &negative_start,
        &[&fixture.alpha_studio, &fixture.beta_studio],
        5,
        -1,
    );

    let zero_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?limit=0",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&zero_limit, &[], 5, 0);

    let negative_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?limit=-1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(negative_limit["Items"].as_array().expect("items").len(), 5);
    assert_eq!(negative_limit["TotalRecordCount"], 5);
    assert_eq!(negative_limit["StartIndex"], 0);

    for query in [
        "startIndex=2147483648",
        "startIndex=-2147483649",
        "limit=2147483648",
        "limit=-2147483649",
    ] {
        assert_eq!(
            fixture
                .request(
                    Method::GET,
                    &format!("/Studios?{query}"),
                    Credential::Device(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
        );
    }

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

    let lowercase_searched = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Studios?searchterm={}&includeitemtypes=Movie&startindex=0&limit=1&enabletotalrecordcount=false",
                    encoded(&fixture.alpha_studio)
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&lowercase_searched, &[&fixture.alpha_studio], 0, 0);
    assert_eq!(lowercase_searched["Items"][0]["MovieCount"], 1);

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

    for parent_id in [Uuid::new_v4(), Uuid::nil()] {
        assert_eq!(
            fixture
                .request(
                    Method::GET,
                    &format!("/Studios?parentId={parent_id}"),
                    Credential::Device(&fixture.user_token),
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST,
        );
    }

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
                "/Studios?startIndex=1&limit=2&enableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(
        &no_total,
        &[&fixture.beta_studio, &fixture.gamma_studio],
        0,
        1,
    );

    let pascal_no_total = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios?StartIndex=2&Limit=1&EnableTotalRecordCount=false",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&pascal_no_total, &[&fixture.gamma_studio], 0, 2);

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
    assert_eq!(
        studio["Id"],
        fixture.alpha_studio_item_id.simple().to_string()
    );
    assert_eq!(studio["Name"], fixture.alpha_studio);
    assert_eq!(studio["Type"], "Studio");
    assert_eq!(studio["ChildCount"], 1);
    assert_eq!(studio["MovieCount"], 1);
    assert_eq!(studio["EpisodeCount"], 0);
    assert_eq!(studio["UserData"]["IsFavorite"], true);
    assert!(studio["Path"].as_str().is_some());
    assert!(studio["DateCreated"].as_str().is_some());
    assert_eq!(
        studio["PresentationUniqueKey"],
        format!("Studio-{}", fixture.alpha_studio)
    );
    assert!(studio.get("item_type").is_none());

    let lowercase_studio = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/studios/{}", encoded(&fixture.alpha_studio)),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        lowercase_studio["Id"],
        fixture.alpha_studio_item_id.simple().to_string()
    );
    assert_eq!(lowercase_studio["Name"], fixture.alpha_studio);

    let studio_route = studio_route(&fixture.alpha_studio);
    for user_id_query in ["userId", "UserId", "userid"] {
        assert_eq!(
            fixture
                .request(
                    Method::GET,
                    &format!("{studio_route}?{user_id_query}={}", fixture.user_id),
                    Credential::Device(&fixture.admin_token),
                )
                .await
                .status(),
            StatusCode::OK,
            "{user_id_query}"
        );
    }
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("{studio_route}?userId={}", Uuid::nil()),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("{studio_route}?limit=bad&parentId=bad&isFavorite=bad"),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("{studio_route}?userId={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let unknown_user = body_json(
        fixture
            .request(
                Method::GET,
                &format!("{studio_route}?userId={}", Uuid::new_v4()),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        unknown_user["Id"],
        fixture.alpha_studio_item_id.simple().to_string()
    );
    assert!(unknown_user.get("UserData").is_none());

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
    let hyphenated = body_json(
        fixture
            .request(
                Method::GET,
                "/Studios/Missing-Studio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(hyphenated["Name"], "Missing-Studio");
    assert_ne!(hyphenated["Id"], Uuid::nil().simple().to_string());

    let items = BaseItemRepository::new(fixture.database.clone());
    let beta_entity = items
        .get_by_type_and_name("Studio", &fixture.beta_studio)
        .await
        .expect("backfilled Studio lookup")
        .expect("backfilled Studio");
    let beta_path = PathBuf::from(beta_entity.path.expect("backfilled Studio path"));
    assert!(
        beta_path.starts_with(
            fixture
                .storage_directory
                .join("programdata/metadata/Studio")
        )
    );
    assert!(tokio::fs::metadata(beta_path).await.unwrap().is_dir());

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
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?userid={}", fixture.other_user_id),
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

    let items = BaseItemRepository::new(fixture.database.clone());
    let hidden_folder_id = Uuid::new_v4();
    let mut hidden_folder = NewBaseItem::new(hidden_folder_id, "CollectionFolder");
    hidden_folder.name = Some("Hidden studio library".to_owned());
    hidden_folder.is_folder = true;
    items
        .create(hidden_folder)
        .await
        .expect("hidden studio folder creation");
    let hidden_movie = create_item(
        &items,
        "Movie",
        "Hidden Studio Movie",
        Some(hidden_folder_id),
        false,
    )
    .await;
    ItemValueRepository::new(fixture.database.clone())
        .link(
            hidden_movie.id,
            item_value::ItemValueType::Studios,
            "Hidden Studio",
        )
        .await
        .expect("hidden studio relation");

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.enable_all_folders = false;
    policy.enabled_folders = vec![fixture.parent_id];
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted studio policy");

    let hidden_parent = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Studios?parentId={hidden_folder_id}"),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_studios(&hidden_parent, &[], 0, 0);

    fixture.cleanup().await;
}

#[tokio::test]
async fn studio_image_routes_resolve_public_base_item_ordinals() {
    let fixture = Fixture::new().await;
    assert_ne!(fixture.alpha_studio_item_id, fixture.alpha_studio_id);
    let first_path = std::env::temp_dir().join(format!("studio-{}.png", Uuid::new_v4().simple()));
    let second_path = std::env::temp_dir().join(format!("studio-{}.png", Uuid::new_v4().simple()));
    image::RgbaImage::from_pixel(4, 2, image::Rgba([220, 30, 30, 255]))
        .save(&first_path)
        .unwrap();
    image::RgbaImage::from_pixel(4, 2, image::Rgba([30, 30, 220, 255]))
        .save(&second_path)
        .unwrap();
    BaseItemImageRepository::new(fixture.database.clone())
        .replace(
            fixture.alpha_studio_item_id,
            &[
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 4,
                    path: first_path.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(4),
                    height: Some(2),
                    blurhash: None,
                },
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 9,
                    path: second_path.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(4),
                    height: Some(2),
                    blurhash: None,
                },
            ],
        )
        .await
        .unwrap();

    let base = format!("{}/Images/Backdrop", studio_route(&fixture.alpha_studio));
    for route in [format!("{base}?imageIndex=1"), format!("{base}/1")] {
        let response = fixture.request(Method::GET, &route, Credential::None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), std::fs::read(&second_path).unwrap());
    }
    let lowercase_base = format!(
        "/studios/{}/images/Backdrop",
        encoded(&fixture.alpha_studio)
    );
    for route in [
        format!("{lowercase_base}?imageIndex=1"),
        format!("{lowercase_base}/1"),
    ] {
        let response = fixture.request(Method::GET, &route, Credential::None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let bytes = to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), std::fs::read(&second_path).unwrap());
    }

    let head = fixture
        .request(Method::HEAD, &format!("{base}/0"), Credential::None)
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .request(Method::GET, &format!("{base}/99"), Credential::None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Studios/{}/Images/Backdrop/0", encoded("missing studio")),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/studios/{}/images/Backdrop/0", encoded("missing studio")),
                Credential::None,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request(Method::GET, &base, Credential::Device("invalid-token"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &lowercase_base,
                Credential::Device("invalid-token"),
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    fixture.cleanup().await;
}

fn assert_studios(
    body: &Value,
    expected_names: &[&str],
    expected_total: usize,
    expected_start: i32,
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
    storage_directory: PathBuf,
    user_id: Uuid,
    other_user_id: Uuid,
    movie_id: Uuid,
    parent_id: Uuid,
    user_token: String,
    admin_token: String,
    alpha_studio_id: Uuid,
    alpha_studio_item_id: Uuid,
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

        let alpha_studio_item_id = official_item_by_name_id("Studio", &alpha_studio);
        let studio_item = create_item_by_name(
            &items,
            alpha_studio_item_id,
            "Studio",
            &alpha_studio,
            &format!("Studio-{alpha_studio}"),
        )
        .await;
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

        let storage_directory =
            std::env::temp_dir().join(format!("jellyfin-studio-routes-{suffix}"));
        let program_data = storage_directory.join("programdata");
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Studio Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                &program_data,
                storage_directory.join("web"),
                storage_directory.join("cache/images"),
                storage_directory.join("cache"),
                program_data.join("metadata"),
            ),
        );

        Self {
            database_name,
            database,
            app,
            storage_directory,
            user_id: user.id,
            other_user_id: other_user.id,
            movie_id: movie.id,
            parent_id: parent.id,
            user_token,
            admin_token,
            alpha_studio_id: alpha.item_value_id,
            alpha_studio_item_id: studio_item.id,
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
            storage_directory,
            ..
        } = self;
        drop(app);
        if tokio::fs::try_exists(&storage_directory)
            .await
            .unwrap_or(false)
        {
            tokio::fs::remove_dir_all(&storage_directory)
                .await
                .expect("temporary Studio storage cleanup");
        }
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

async fn create_item_by_name(
    repository: &BaseItemRepository,
    id: Uuid,
    item_type: &str,
    name: &str,
    presentation_unique_key: &str,
) -> base_item::Model {
    let mut item = NewBaseItem::new(id, item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.path = Some(format!("metadata/{item_type}/{name}"));
    item.is_folder = true;
    item.presentation_unique_key = Some(presentation_unique_key.to_owned());
    repository
        .create(item)
        .await
        .expect("item-by-name creation")
}

fn official_item_by_name_id(item_type: &str, name: &str) -> Uuid {
    let clr_type = match item_type {
        "Studio" => "MediaBrowser.Controller.Entities.Studio",
        "Year" => "MediaBrowser.Controller.Entities.Year",
        _ => panic!("unsupported test item-by-name type"),
    };
    let path = format!("metadata\\{item_type}\\{name}").to_lowercase();
    let value = format!("{clr_type}{path}");
    let digest = Md5::digest(
        value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    Uuid::from_bytes([
        digest[3], digest[2], digest[1], digest[0], digest[5], digest[4], digest[7], digest[6],
        digest[8], digest[9], digest[10], digest[11], digest[12], digest[13], digest[14],
        digest[15],
    ])
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
