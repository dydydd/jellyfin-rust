#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewUserData, UserDataRepository, entities::item_value,
};
use jellyfin_model::UserPolicy;
use md5::{Digest, Md5};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use std::path::PathBuf;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str =
    "MediaBrowser Client=\"Year Tests\", DeviceId=\"year-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_year_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn year_route_matches_official_authenticated_item_by_name_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture
            .request(Method::GET, "/Years/2024", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(Method::GET, "/Years", Credential::None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let years = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?sortBy=ProductionYear&sortOrder=Descending&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&years, &["2024", "2001"], 4, 0);
    let listed_2024_id = years["Items"][0]["Id"].clone();

    let items = BaseItemRepository::new(fixture.database.clone());
    let backfilled_2024 = items
        .get_by_type_and_name("Year", "2024")
        .await
        .expect("backfilled Year lookup")
        .expect("backfilled Year");
    assert_eq!(listed_2024_id, backfilled_2024.id.simple().to_string());
    let backfilled_path = PathBuf::from(backfilled_2024.path.expect("backfilled Year path"));
    assert!(
        backfilled_path.starts_with(fixture.storage_directory.join("programdata/metadata/Year"))
    );
    assert!(tokio::fs::metadata(backfilled_path).await.unwrap().is_dir());

    let lowercase_years = body_json(
        fixture
            .request(
                Method::GET,
                "/years?sortBy=ProductionYear&sortOrder=Descending&limit=1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&lowercase_years, &["2024"], 4, 0);

    let pascal_years = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?SortBy=ProductionYear&SortOrder=Descending&StartIndex=1&Limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&pascal_years, &["2001", "1999"], 4, 1);

    let paged = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?startIndex=1&limit=2",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&paged, &["1999", "2001"], 4, 1);

    let negative_start = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?startIndex=-1&limit=2&sortBy=ProductionYear&sortOrder=Descending",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&negative_start, &["2024", "2001"], 4, -1);

    let negative_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?limit=-1",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&negative_limit, &[], 4, 0);

    let zero_limit = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?limit=0",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&zero_limit, &[], 4, 0);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years?startIndex=2147483648",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years?limit=-2147483649",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );

    let direct_child_years = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years?parentId={}&recursive=false", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&direct_child_years, &["1999"], 1, 0);

    let pascal_direct_child_years = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years?ParentId={}&Recursive=false", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&pascal_direct_child_years, &["1999"], 1, 0);

    let recursive_years = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years?parentId={}&recursive=true", fixture.parent_id),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&recursive_years, &["1999", "2001"], 2, 0);

    let lowercase_recursive_years = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Years?parentid={}&recursive=true&sortby=ProductionYear&sortorder=Descending&startindex=1&limit=1",
                    fixture.parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&lowercase_recursive_years, &["1999"], 2, 1);

    let item_year = body_json(
        fixture
            .request(
                Method::GET,
                &format!(
                    "/Years?parentId={}&recursive=true",
                    fixture.non_folder_parent_id
                ),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&item_year, &["1999"], 1, 0);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Years?parentId={}", Uuid::new_v4()),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );

    let audio_years = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?includeItemTypes=Audio",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&audio_years, &["1977"], 1, 0);

    let video_years = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?mediaTypes=Video",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&video_years, &["1999", "2001", "2024"], 3, 0);

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years?sortOrder=sideways",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years?sortBy=sideways",
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
                &format!("/Years?userId={}", fixture.other_user_id),
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
                &format!("/Years?userid={}", fixture.other_user_id),
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let virtual_year = body_json(
        fixture
            .request(
                Method::GET,
                "/Years/2024",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(virtual_year["Name"], "2024");
    assert_eq!(virtual_year["Type"], "Year");
    assert_eq!(virtual_year["IsFolder"], true);
    assert_eq!(virtual_year["PresentationUniqueKey"], "Year-2024");
    assert_eq!(
        virtual_year["Id"].as_str().expect("virtual year id").len(),
        32
    );
    assert!(virtual_year.get("name").is_none());

    let lowercase_virtual_year = body_json(
        fixture
            .request(
                Method::GET,
                "/years/2024",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_virtual_year["Name"], "2024");
    assert_eq!(lowercase_virtual_year["Type"], "Year");

    let unused_year = body_json(
        fixture
            .request(
                Method::GET,
                "/Years/1901",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(unused_year["Name"], "1901");
    assert_eq!(unused_year["Type"], "Year");
    assert_eq!(unused_year["IsFolder"], true);
    assert_eq!(unused_year["PresentationUniqueKey"], "Year-1901");
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years/0",
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years/-1",
                Credential::Device(&fixture.user_token)
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request(
                Method::GET,
                &format!("/Years/2024?userId={}", fixture.other_user_id),
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
                &format!("/Years/2024?userId={}", fixture.user_id),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(admin_targeted["Name"], "2024");
    assert_eq!(admin_targeted["Id"], listed_2024_id);
    assert!(admin_targeted["Path"].as_str().is_some());
    assert!(admin_targeted["DateCreated"].as_str().is_some());

    let unknown_user = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years/2024?UserId={}", Uuid::new_v4()),
                Credential::Device(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(unknown_user["Id"], listed_2024_id);
    assert!(unknown_user.get("UserData").is_none());

    assert_eq!(
        fixture
            .request(
                Method::GET,
                "/Years/2024?limit=invalid&parentId=invalid&recursive=invalid",
                Credential::Device(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::OK
    );

    let persisted_year = body_json(
        fixture
            .request(
                Method::GET,
                "/Years/1984",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(persisted_year["Name"], "1984");
    assert_eq!(persisted_year["Type"], "Year");
    assert_eq!(
        persisted_year["Id"],
        fixture.persisted_year_id.simple().to_string()
    );
    assert_eq!(persisted_year["UserData"]["IsFavorite"], true);

    let items = BaseItemRepository::new(fixture.database.clone());
    let visible_folder_id = Uuid::new_v4();
    let mut visible_folder = NewBaseItem::new(visible_folder_id, "CollectionFolder");
    visible_folder.name = Some("Visible policy library".to_owned());
    visible_folder.is_folder = true;
    items
        .create(visible_folder)
        .await
        .expect("visible policy folder creation");
    let hidden_folder_id = Uuid::new_v4();
    let mut hidden_folder = NewBaseItem::new(hidden_folder_id, "CollectionFolder");
    hidden_folder.name = Some("Hidden policy library".to_owned());
    hidden_folder.is_folder = true;
    items
        .create(hidden_folder)
        .await
        .expect("hidden policy folder creation");

    create_policy_video(&items, visible_folder_id, "Visible", 1967, "G").await;
    create_policy_video(&items, hidden_folder_id, "Hidden", 1966, "G").await;
    let blocked_video = create_policy_video(&items, visible_folder_id, "Blocked", 1968, "G").await;
    create_policy_video(&items, visible_folder_id, "Rated", 1969, "R").await;
    ItemValueRepository::new(fixture.database.clone())
        .link(blocked_video.id, item_value::ItemValueType::Tags, "Blocked")
        .await
        .expect("blocked policy tag");

    let mut policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned()),
        ..UserPolicy::default()
    };
    policy.enable_all_folders = false;
    policy.enabled_folders = vec![visible_folder_id];
    policy.blocked_tags = vec!["Blocked".to_owned()];
    policy.max_parental_rating = Some(5);
    UserService::new(fixture.database.clone())
        .update_policy(fixture.user_id, &policy)
        .await
        .expect("restricted year policy");

    let policy_years = body_json(
        fixture
            .request(
                Method::GET,
                "/Years?includeItemTypes=Video",
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&policy_years, &["1967"], 1, 0);
    let returned_ids = policy_years["Items"]
        .as_array()
        .expect("policy year items")
        .iter()
        .map(|item| item["Name"].as_str().expect("policy year name"))
        .collect::<Vec<_>>();
    assert!(!returned_ids.contains(&"1966"));
    assert!(!returned_ids.contains(&"1968"));
    assert!(!returned_ids.contains(&"1969"));
    let hidden_parent_years = body_json(
        fixture
            .request(
                Method::GET,
                &format!("/Years?parentId={hidden_folder_id}"),
                Credential::Device(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_years(&hidden_parent_years, &[], 0, 0);

    fixture.cleanup().await;
}

async fn create_policy_video(
    items: &BaseItemRepository,
    parent_id: Uuid,
    name: &str,
    year: i32,
    rating: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut video = NewBaseItem::new(Uuid::new_v4(), "Video");
    video.name = Some(format!("{name} policy video"));
    video.media_type = Some("Video".to_owned());
    video.production_year = Some(year);
    video.official_rating = Some(rating.to_owned());
    video.parent_id = Some(parent_id);
    items.create(video).await.expect("policy video creation")
}

fn assert_years(body: &Value, expected_names: &[&str], expected_total: usize, expected_start: i32) {
    assert_eq!(body["TotalRecordCount"], expected_total);
    assert_eq!(body["StartIndex"], expected_start);
    let items = body["Items"].as_array().expect("year items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("year name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "Year"));
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
    parent_id: Uuid,
    non_folder_parent_id: Uuid,
    user_token: String,
    admin_token: String,
    persisted_year_id: Uuid,
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
            .create_initial_administrator(&format!("year-admin-{suffix}"))
            .await
            .unwrap();
        let user = users.create(&format!("year-user-{suffix}")).await.unwrap();
        let other_user = users
            .create(&format!("year-other-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        movie.name = Some("The Future".to_owned());
        movie.sort_name = Some("Future".to_owned());
        movie.media_type = Some("Video".to_owned());
        movie.production_year = Some(2024);
        items.create(movie).await.expect("movie creation");

        let parent_id = Uuid::new_v4();
        let mut parent = NewBaseItem::new(parent_id, "Folder");
        parent.name = Some("Year Parent".to_owned());
        parent.sort_name = Some("Year Parent".to_owned());
        parent.is_folder = true;
        items.create(parent).await.expect("parent creation");

        let mut child_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        child_movie.name = Some("Direct Child".to_owned());
        child_movie.sort_name = Some("Direct Child".to_owned());
        child_movie.media_type = Some("Video".to_owned());
        child_movie.production_year = Some(1999);
        child_movie.parent_id = Some(parent_id);
        let child_movie = items
            .create(child_movie)
            .await
            .expect("child movie creation");
        let non_folder_parent_id = child_movie.id;

        let nested_id = Uuid::new_v4();
        let mut nested = NewBaseItem::new(nested_id, "Folder");
        nested.name = Some("Nested".to_owned());
        nested.sort_name = Some("Nested".to_owned());
        nested.is_folder = true;
        nested.parent_id = Some(parent_id);
        items.create(nested).await.expect("nested folder creation");

        let mut nested_movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
        nested_movie.name = Some("Nested Child".to_owned());
        nested_movie.sort_name = Some("Nested Child".to_owned());
        nested_movie.media_type = Some("Video".to_owned());
        nested_movie.production_year = Some(2001);
        nested_movie.parent_id = Some(nested_id);
        items
            .create(nested_movie)
            .await
            .expect("nested movie creation");

        let mut audio = NewBaseItem::new(Uuid::new_v4(), "Audio");
        audio.name = Some("Old Song".to_owned());
        audio.sort_name = Some("Old Song".to_owned());
        audio.media_type = Some("Audio".to_owned());
        audio.production_year = Some(1977);
        items.create(audio).await.expect("audio creation");

        let persisted_year_id = official_item_by_name_id("Year", "1984");
        let mut year = NewBaseItem::new(persisted_year_id, "Year");
        year.name = Some("1984".to_owned());
        year.sort_name = Some("1984".to_owned());
        year.path = Some("metadata/Year/1984".to_owned());
        year.is_folder = true;
        items.create(year).await.expect("year item creation");
        let mut year_favorite = NewUserData::new(persisted_year_id, user.id, "YearFavorite");
        year_favorite.is_favorite = true;
        UserDataRepository::new(database.clone())
            .upsert(year_favorite)
            .await
            .expect("year user data");

        let storage_directory = std::env::temp_dir().join(format!("jellyfin-year-routes-{suffix}"));
        let program_data = storage_directory.join("programdata");
        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Year Test Server".to_owned(),
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
            parent_id,
            non_folder_parent_id,
            user_token,
            admin_token,
            persisted_year_id,
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
                .expect("temporary Year storage cleanup");
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
            "Year Tests",
            "1.0",
            "Test",
            format!("year-tests-{suffix}"),
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
