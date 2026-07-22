use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DeviceRepository, ItemValueRepository, NewBaseItem, NewDevice,
    entities::{item_value, user},
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Music Genre Tests\", Device=\"Test\", DeviceId=\"music-genre\", Version=\"1.0\"";

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

struct MusicGenreFixture {
    database: DatabaseConnection,
    app: axum::Router,
    administrator_id: Uuid,
    administrator_token: String,
    user_id: Uuid,
    user_token: String,
    item_ids: Vec<Uuid>,
    genre_id: Uuid,
    genre_name: String,
    slug_genre_id: Uuid,
    slug_genre_name: String,
    slug_name: String,
    book_only_genre: String,
}

impl MusicGenreFixture {
    async fn new() -> Self {
        let database = test_database().await;
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
        let book_only_genre = format!("Literature {suffix}");
        values
            .link(book.id, item_value::ItemValueType::Genre, &book_only_genre)
            .await
            .expect("book-only genre link");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Music Genre Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            administrator_id: administrator.id,
            administrator_token,
            user_id: user.id,
            user_token,
            item_ids: vec![audio.id, second_audio.id, slug_audio.id, book.id],
            genre_id: genre.item_value_id,
            genre_name,
            slug_genre_id: slug_genre.item_value_id,
            slug_genre_name,
            slug_name,
            book_only_genre,
        }
    }

    async fn cleanup(self) {
        let items = BaseItemRepository::new(self.database.clone());
        for item_id in self.item_ids {
            items.delete(item_id).await.expect("item cleanup");
        }
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.administrator_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
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

async fn test_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}
