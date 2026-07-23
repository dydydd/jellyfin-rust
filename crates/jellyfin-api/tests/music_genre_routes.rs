use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem,
    NewDevice, NewUserData, UserDataRepository, entities::item_value,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Music Genre Tests\", Device=\"Test\", DeviceId=\"music-genre\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_music_genre_routes_";

#[tokio::test]
async fn official_fake_music_genre_is_not_found() {
    let fixture = MusicGenreFixture::new().await;
    let response = request(
        &fixture.app,
        "/MusicGenres/Fake-MusicGenre",
        Some(&fixture.administrator_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let punctuation = request(
        &fixture.app,
        "/MusicGenres/---",
        Some(&fixture.administrator_token),
    )
    .await;
    assert_eq!(punctuation.status(), StatusCode::NOT_FOUND);
    fixture.cleanup().await;
}

#[tokio::test]
async fn music_genre_returns_pascal_case_base_item_dto() {
    let fixture = MusicGenreFixture::new().await;
    let response = request(
        &fixture.app,
        &genre_route(&fixture.genre_name),
        Some(&fixture.user_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dto = body_json(response).await;
    assert_eq!(dto["Id"], fixture.genre_id.simple().to_string());
    assert_eq!(dto["Name"], fixture.genre_name);
    assert_eq!(dto["Type"], "MusicGenre");
    assert_eq!(dto["IsFolder"], true);
    assert_eq!(dto["IsVirtualItem"], false);
    assert_eq!(dto["ServerId"].as_str().unwrap().len(), 32);
    assert!(dto["Etag"].is_string());
    assert!(dto.get("item_type").is_none());
    assert!(dto.get("server_id").is_none());

    let slug_response = request(
        &fixture.app,
        &format!("/MusicGenres/{}", fixture.slug_name),
        Some(&fixture.user_token),
    )
    .await;
    assert_eq!(slug_response.status(), StatusCode::OK);
    let slug_dto = body_json(slug_response).await;
    assert_eq!(slug_dto["Id"], fixture.slug_genre_id.simple().to_string());
    assert_eq!(slug_dto["Name"], fixture.slug_genre_name);

    fixture.cleanup().await;
}

#[tokio::test]
async fn unicode_and_case_normalization_reuse_one_genre() {
    let fixture = MusicGenreFixture::new().await;
    let variant = fixture.genre_name.replace('É', "e").to_uppercase();
    let response = request(
        &fixture.app,
        &genre_route(&variant),
        Some(&fixture.user_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dto = body_json(response).await;
    assert_eq!(dto["Id"], fixture.genre_id.simple().to_string());
    assert_eq!(dto["Name"], fixture.genre_name);

    let values = ItemValueRepository::new(fixture.database.clone());
    let normalized = values
        .get_normalized(item_value::ItemValueType::Genre, &variant)
        .await
        .expect("normalized lookup")
        .expect("genre must exist");
    assert_eq!(normalized.item_value_id, fixture.genre_id);

    let book_only = request(
        &fixture.app,
        &genre_route(&fixture.book_only_genre),
        Some(&fixture.user_token),
    )
    .await;
    assert_eq!(book_only.status(), StatusCode::NOT_FOUND);

    fixture.cleanup().await;
}

#[tokio::test]
async fn authentication_and_target_user_permissions_are_enforced() {
    let fixture = MusicGenreFixture::new().await;
    let route = genre_route(&fixture.genre_name);
    let response = request(&fixture.app, &route, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let for_administrator = format!("{route}?userId={}", fixture.administrator_id);
    let response = request(&fixture.app, &for_administrator, Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let for_user = format!("{route}?userId={}", fixture.user_id);
    let response = request(&fixture.app, &for_user, Some(&fixture.administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);

    let missing_user = format!("{route}?userId={}", Uuid::new_v4());
    let response = request(
        &fixture.app,
        &missing_user,
        Some(&fixture.administrator_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    fixture.cleanup().await;
}

#[tokio::test]
async fn music_genre_list_matches_official_music_genre_contract() {
    let fixture = MusicGenreFixture::new().await;

    let unauthenticated = request(&fixture.app, "/MusicGenres", None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let listed = body_json(
        request(
            &fixture.app,
            "/MusicGenres?limit=2",
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(
        &listed,
        &[&fixture.nested_genre_name, &fixture.genre_name],
        3,
        0,
    );

    let descending = body_json(
        request(
            &fixture.app,
            "/MusicGenres?sortOrder=Descending&limit=1",
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&descending, &[&fixture.slug_genre_name], 3, 0);

    let searched = body_json(
        request(
            &fixture.app,
            &format!("/MusicGenres?searchTerm={}", encoded(&fixture.genre_name)),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&searched, &[&fixture.genre_name], 1, 0);

    let favorite = body_json(
        request(
            &fixture.app,
            "/MusicGenres?isFavorite=true",
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&favorite, &[&fixture.genre_name], 1, 0);

    let parent_scoped = body_json(
        request(
            &fixture.app,
            &format!("/MusicGenres?parentId={}", fixture.parent_id),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&parent_scoped, &[&fixture.nested_genre_name], 1, 0);

    let item_scoped = body_json(
        request(
            &fixture.app,
            &format!("/MusicGenres?parentId={}", fixture.audio_id),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&item_scoped, &[&fixture.genre_name], 1, 0);

    let book_filtered = body_json(
        request(
            &fixture.app,
            "/MusicGenres?includeItemTypes=Book",
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_genres(&book_filtered, &[], 0, 0);

    let no_total = body_json(
        request(
            &fixture.app,
            "/MusicGenres?limit=1&enableTotalRecordCount=false",
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_eq!(no_total["Items"].as_array().expect("items").len(), 1);
    assert_eq!(no_total["TotalRecordCount"], 1);

    assert_eq!(
        request(
            &fixture.app,
            "/MusicGenres?sortOrder=sideways",
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let for_administrator = format!("/MusicGenres?userId={}", fixture.administrator_id);
    let response = request(&fixture.app, &for_administrator, Some(&fixture.user_token)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let for_user = format!("/MusicGenres?userId={}", fixture.user_id);
    let response = request(&fixture.app, &for_user, Some(&fixture.administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);

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
    let items = body["Items"].as_array().expect("music genre items");
    assert_eq!(items.len(), expected_names.len());
    let names = items
        .iter()
        .map(|item| item["Name"].as_str().expect("music genre name"))
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(items.iter().all(|item| item["Type"] == "MusicGenre"));
    assert!(items.iter().all(|item| item["IsFolder"] == true));
    assert!(body.get("items").is_none());
}

struct MusicGenreFixture {
    database_name: String,
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    audio_id: Uuid,
    parent_id: Uuid,
    genre_id: Uuid,
    genre_name: String,
    nested_genre_name: String,
    slug_genre_id: Uuid,
    slug_genre_name: String,
    slug_name: String,
    book_only_genre: String,
}

impl MusicGenreFixture {
    async fn new() -> Self {
        let (database_name, database) = test_database().await;
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("genre-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("genre-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token = devices
            .create_session(NewDevice::new(
                administrator.id,
                "Music Genre Tests",
                "1.0",
                "Test",
                format!("genre-admin-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Music Genre Tests",
                "1.0",
                "Test",
                format!("genre-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let audio = create_item(&items, "Audio", "Unicode Track").await;
        let second_audio = create_item(&items, "MusicVideo", "Unicode Video").await;
        let slug_audio = create_item(&items, "MusicAlbum", "Slug Album").await;
        let parent = create_folder(&items, "Music Genre Parent").await;
        let nested_audio =
            create_child_item(&items, "Audio", "Nested Genre Track", parent.id).await;
        let book = create_item(&items, "Book", "Genre Book").await;
        let values = ItemValueRepository::new(database.clone());
        let genre_name = format!("Électronique 東京 {suffix}");
        let genre = values
            .link(audio.id, item_value::ItemValueType::Genre, &genre_name)
            .await
            .expect("unicode genre link");
        let variant = genre_name.replace('É', "e").to_uppercase();
        let (same_genre, duplicate_link) = tokio::join!(
            values.link(second_audio.id, item_value::ItemValueType::Genre, &variant),
            values.link(audio.id, item_value::ItemValueType::Genre, &genre_name)
        );
        assert_eq!(
            same_genre
                .expect("normalized concurrent link")
                .item_value_id,
            genre.item_value_id
        );
        assert_eq!(
            duplicate_link
                .expect("duplicate concurrent link")
                .item_value_id,
            genre.item_value_id
        );

        let slug_genre_name = format!("Left/Right{suffix}");
        let slug_genre = values
            .link(
                slug_audio.id,
                item_value::ItemValueType::Genre,
                &slug_genre_name,
            )
            .await
            .expect("slug genre link");
        let slug_name = format!("Left-Right{suffix}");
        let nested_genre_name = format!("Ambient {suffix}");
        values
            .link(
                nested_audio.id,
                item_value::ItemValueType::Genre,
                &nested_genre_name,
            )
            .await
            .expect("nested genre link");
        let book_only_genre = format!("Literature {suffix}");
        values
            .link(book.id, item_value::ItemValueType::Genre, &book_only_genre)
            .await
            .expect("book-only genre link");
        let mut favorite = NewUserData::new(audio.id, user.id, "MusicGenreFavorite");
        favorite.is_favorite = true;
        UserDataRepository::new(database.clone())
            .upsert(favorite)
            .await
            .expect("favorite user data");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Music Genre Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database_name,
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            user_id: user.id,
            user_token,
            audio_id: audio.id,
            parent_id: parent.id,
            genre_id: genre.item_value_id,
            genre_name,
            nested_genre_name,
            slug_genre_id: slug_genre.item_value_id,
            slug_genre_name,
            slug_name,
            book_only_genre,
        }
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
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    repository.create(item).await.expect("base item creation")
}

async fn create_child_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    repository.create(item).await.expect("base item creation")
}

async fn create_folder(
    repository: &BaseItemRepository,
    name: &str,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Folder");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.is_folder = true;
    repository.create(item).await.expect("base item creation")
}

fn encoded(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn genre_route(name: &str) -> String {
    format!(
        "/MusicGenres/{}",
        utf8_percent_encode(name, NON_ALPHANUMERIC)
    )
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
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
    .expect("local PostgreSQL must be available");
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
