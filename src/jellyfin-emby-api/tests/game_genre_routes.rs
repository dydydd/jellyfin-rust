#![allow(clippy::too_many_lines)]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemImageRepository, BaseItemImageType, BaseItemRepository,
    DatabaseConfig, DeviceRepository, ItemValueRepository, NewBaseItem, NewBaseItemImage,
    NewDevice, entities::item_value,
};
use jellyfin_model::UserPolicy;
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Game Genre Tests\", DeviceId=\"emby-game-genres\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_game_genres_";

#[tokio::test]
async fn game_genres_match_legacy_emby_without_polluting_jellyfin_routes() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.starts_with(DATABASE_PREFIX));
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary database migrations");

    let temp_root = TempRoot::new();
    let users = UserService::new(database.clone());
    let user = users
        .create(&format!("game-genre-user-{}", Uuid::new_v4().simple()))
        .await
        .expect("user creation");
    let items = BaseItemRepository::new(database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let allowed = create_item(
        &items,
        "CollectionFolder",
        "Allowed games",
        Some(root.id),
        true,
    )
    .await;
    let hidden = create_item(
        &items,
        "CollectionFolder",
        "Hidden games",
        Some(root.id),
        true,
    )
    .await;
    let game = create_item(&items, "Game", "Visible game", Some(allowed.id), false).await;
    let similar_game = create_item(
        &items,
        "Game",
        "Visible similar game",
        Some(allowed.id),
        false,
    )
    .await;
    let hidden_game = create_item(&items, "Game", "Hidden game", Some(hidden.id), false).await;
    let movie = create_item(&items, "Movie", "Movie", Some(allowed.id), false).await;

    let suffix = Uuid::new_v4().simple().to_string();
    let visible_genre = format!("Action{suffix}");
    let hidden_genre = format!("Hidden{suffix}");
    let movie_only_genre = format!("MovieOnly{suffix}");
    let slug_value = format!("Left/Right{suffix}");
    let values = ItemValueRepository::new(database.clone());
    for (item_id, name) in [
        (game.id, visible_genre.as_str()),
        (game.id, slug_value.as_str()),
        (hidden_game.id, hidden_genre.as_str()),
        (movie.id, movie_only_genre.as_str()),
    ] {
        values
            .link(item_id, item_value::ItemValueType::Genre, name)
            .await
            .expect("genre link");
    }
    users
        .update_policy(
            user.id,
            &UserPolicy {
                authentication_provider_id: Some(
                    UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                ),
                password_reset_provider_id: Some(
                    UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                ),
                enable_all_folders: false,
                enabled_folders: vec![allowed.id],
                ..UserPolicy::default()
            },
        )
        .await
        .expect("user policy");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "Emby Game Genre Tests",
            "1.0",
            "Test",
            format!("game-genre-device-{}", Uuid::new_v4().simple()),
        ))
        .await
        .expect("device session")
        .access_token;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("game-genre-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let state = AppState::new(
        database.clone(),
        "Emby Game Genre Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    )
    .with_storage_paths(
        temp_root.path().join("programdata"),
        temp_root.path().join("web"),
        temp_root.path().join("images"),
        temp_root.path().join("cache"),
        temp_root.path().join("metadata"),
    );
    let jellyfin = jellyfin_api::router(state.clone());
    let emby = jellyfin_emby_api::router(state);

    let similar_route = format!("/emby/gAmEs/{}/sImIlAr?UsErId={}&LiMiT=1", game.id, user.id);
    let similar =
        json_response(request(&emby, Method::GET, &similar_route, Some(&token)).await).await;
    assert_eq!(similar["TotalRecordCount"], 1);
    assert_eq!(
        similar["Items"][0]["Id"],
        similar_game.id.simple().to_string()
    );
    assert_eq!(similar["Items"][0]["Type"], "Game");
    let api_key_similar = json_response(
        request(
            &emby,
            Method::GET,
            &format!("/emby/Games/{}/Similar?Limit=10", game.id),
            Some(&api_key),
        )
        .await,
    )
    .await;
    assert_eq!(api_key_similar["TotalRecordCount"], 2);
    assert!(
        api_key_similar["Items"]
            .as_array()
            .expect("API-key similar games")
            .iter()
            .any(|item| item["Id"] == hidden_game.id.simple().to_string())
    );
    assert_eq!(
        request(
            &jellyfin,
            Method::GET,
            &format!("/Games/{}/Similar", game.id),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "Emby Game similarity must not leak into Jellyfin"
    );

    let remote_search_body = serde_json::json!({
        "SearchInfo": {"Name": "Example Game", "ProviderIds": {}},
        "ItemId": "42",
        "Providers": ["LegacyGameProvider"],
        "IncludeDisabledProviders": true
    });
    let remote_search = request_json(
        &emby,
        "/emby/iTeMs/rEmOtEsEaRcH/gAmE",
        Some(&token),
        &remote_search_body,
    )
    .await;
    assert_eq!(remote_search.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(remote_search.into_body(), 64 * 1024)
            .await
            .expect("remote search body"),
        &b"[]"[..]
    );
    assert_eq!(
        request_json(
            &jellyfin,
            "/Items/RemoteSearch/Game",
            Some(&token),
            &remote_search_body,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "Emby Game remote search must not leak into Jellyfin"
    );

    assert_eq!(
        request(&emby, Method::GET, "/emby/GameGenres", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let scoped_route = format!(
        "/emby/gAmEgEnReS?uSeRiD={}&iNcLuDeItEmTyPeS=Game&fIeLdS=CanDelete",
        user.id
    );
    let scoped =
        json_response(request(&emby, Method::GET, &scoped_route, Some(&token)).await).await;
    assert_eq!(scoped["TotalRecordCount"], 2);
    let scoped_items = scoped["Items"].as_array().expect("scoped items");
    assert!(
        scoped_items
            .iter()
            .any(|item| item["Name"] == visible_genre)
    );
    assert!(scoped_items.iter().any(|item| item["Name"] == slug_value));
    assert!(scoped_items.iter().all(|item| item["Name"] != hidden_genre));
    assert!(scoped_items.iter().all(|item| item["Type"] == "GameGenre"));
    for item in scoped_items {
        assert_eq!(item["IsFolder"], false);
        assert_eq!(item["IsVirtualItem"], false);
        assert_eq!(item["PrimaryImageAspectRatio"], 1.0);
        assert_eq!(item["CanDelete"], false);
        assert_eq!(item["ChildCount"], 1);
        assert_eq!(item["GameCount"], 1);
    }

    let global = json_response(
        request(
            &emby,
            Method::GET,
            "/emby/GameGenres?StartIndex=-2&Limit=-1",
            Some(&token),
        )
        .await,
    )
    .await;
    let global_items = global["Items"].as_array().expect("global items");
    assert!(global_items.iter().any(|item| item["Name"] == hidden_genre));
    assert!(
        global_items
            .iter()
            .all(|item| item["Name"] != movie_only_genre)
    );
    let api_key_list = json_response(
        request(
            &emby,
            Method::GET,
            "/emby/GameGenres?IncludeItemTypes=Game",
            Some(&api_key),
        )
        .await,
    )
    .await;
    assert!(
        api_key_list["Items"]
            .as_array()
            .expect("API-key items")
            .iter()
            .any(|item| item["Name"] == hidden_genre)
    );

    let empty =
        json_response(request(&emby, Method::GET, "/emby/GameGenres?lImIt=0", Some(&token)).await)
            .await;
    assert_eq!(empty["Items"], serde_json::json!([]));
    for query in [
        "StartIndex=2147483648",
        "Limit=-2147483649",
        "IsFavorite=invalid",
    ] {
        assert_eq!(
            request(
                &emby,
                Method::GET,
                &format!("/emby/GameGenres?{query}"),
                Some(&token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }

    let detail = json_response(
        request(
            &emby,
            Method::GET,
            &format!("/emby/GameGenres/{visible_genre}?USERID={}", user.id),
            Some(&token),
        )
        .await,
    )
    .await;
    assert_eq!(detail["Name"], visible_genre);
    assert_eq!(detail["Type"], "GameGenre");
    assert_eq!(detail["CanDelete"], false);
    assert_eq!(detail["ChildCount"], 1);
    assert_eq!(detail["GameCount"], 1);
    assert_eq!(detail["MovieCount"], 0);
    assert_eq!(detail["SeriesCount"], 0);
    assert!(detail.get("UserData").is_some());
    let api_key_detail = json_response(
        request(
            &emby,
            Method::GET,
            &format!("/emby/GameGenres/{visible_genre}"),
            Some(&api_key),
        )
        .await,
    )
    .await;
    assert_eq!(api_key_detail["Name"], visible_genre);
    assert!(api_key_detail.get("UserData").is_none());

    let slug = slug_value.replace('/', "-");
    let slug_detail = json_response(
        request(
            &emby,
            Method::GET,
            &format!("/emby/gAmEgEnReS/{slug}"),
            Some(&token),
        )
        .await,
    )
    .await;
    assert_eq!(slug_detail["Name"], slug_value);
    let literal_fallback = json_response(
        request(
            &emby,
            Method::GET,
            "/emby/GameGenres/Definitely-Unknown",
            Some(&token),
        )
        .await,
    )
    .await;
    assert_eq!(literal_fallback["Name"], "Definitely-Unknown");

    let owner = items
        .get_by_type_and_name("GameGenre", &visible_genre)
        .await
        .expect("GameGenre image owner query")
        .expect("GameGenre image owner");
    assert!(!owner.is_folder);
    assert!(
        owner
            .path
            .as_deref()
            .is_some_and(|path| path.contains("GameGenre"))
    );
    let image_path = temp_root.path().join("game-genre.png");
    let backdrop_zero = temp_root.path().join("game-genre-backdrop-0.png");
    let backdrop_one = temp_root.path().join("game-genre-backdrop-1.png");
    std::fs::write(&image_path, b"original-game-genre-image").expect("image fixture");
    std::fs::write(&backdrop_zero, b"game-genre-backdrop-zero").expect("image fixture");
    std::fs::write(&backdrop_one, b"game-genre-backdrop-one").expect("image fixture");
    BaseItemImageRepository::new(database.clone())
        .replace(
            owner.id,
            &[
                NewBaseItemImage {
                    image_type: BaseItemImageType::Primary,
                    image_index: 0,
                    path: image_path.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(1),
                    height: Some(1),
                    blurhash: None,
                },
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 0,
                    path: backdrop_zero.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(1),
                    height: Some(1),
                    blurhash: None,
                },
                NewBaseItemImage {
                    image_type: BaseItemImageType::Backdrop,
                    image_index: 1,
                    path: backdrop_one.to_string_lossy().into_owned(),
                    date_modified: Utc::now(),
                    width: Some(1),
                    height: Some(1),
                    blurhash: None,
                },
            ],
        )
        .await
        .expect("GameGenre image metadata");
    let image_route = format!("/emby/gAmEgEnReS/{visible_genre}/iMaGeS/pRiMaRy/0");
    let image = request(&emby, Method::GET, &image_route, Some(&token)).await;
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(image.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        to_bytes(image.into_body(), 1024).await.expect("image body"),
        &b"original-game-genre-image"[..]
    );
    let indexed_image = request(
        &emby,
        Method::GET,
        &format!("/emby/GameGenres/{visible_genre}/Images/Backdrop?Index=1"),
        Some(&api_key),
    )
    .await;
    assert_eq!(indexed_image.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(indexed_image.into_body(), 1024)
            .await
            .expect("indexed image body"),
        &b"game-genre-backdrop-one"[..]
    );
    let slug_owner = items
        .get_by_type_and_name("GameGenre", &slug_value)
        .await
        .expect("slug GameGenre owner query")
        .expect("slug GameGenre owner");
    BaseItemImageRepository::new(database.clone())
        .replace(
            slug_owner.id,
            &[NewBaseItemImage {
                image_type: BaseItemImageType::Primary,
                image_index: 0,
                path: image_path.to_string_lossy().into_owned(),
                date_modified: Utc::now(),
                width: Some(1),
                height: Some(1),
                blurhash: None,
            }],
        )
        .await
        .expect("slug GameGenre image metadata");
    assert_eq!(
        request(
            &emby,
            Method::GET,
            &format!("/emby/GameGenres/{slug}/Images/Primary"),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let head = request(&emby, Method::HEAD, &image_route, Some(&token)).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(
        to_bytes(head.into_body(), 1024)
            .await
            .expect("HEAD body")
            .is_empty()
    );

    assert_eq!(
        request(&jellyfin, Method::GET, "/GameGenres", Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "Emby GameGenres must not leak into the Jellyfin tree"
    );
    assert_ne!(
        request(&jellyfin, Method::GET, "/Genres", Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(
        request(&emby, Method::GET, "/emby/Genres", Some(&token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let jellyfin_items = json_response(
        request(
            &jellyfin,
            Method::GET,
            &format!("/Items?UserId={}&Ids={}", user.id, owner.id),
            Some(&token),
        )
        .await,
    )
    .await;
    assert_eq!(jellyfin_items["Items"], serde_json::json!([]));
    assert_eq!(
        request(
            &jellyfin,
            Method::GET,
            &format!("/Items/{}?UserId={}", owner.id, user.id),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    drop(emby);
    drop(jellyfin);
    database.close().await.expect("database close");
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    is_folder: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.is_folder = is_folder;
    repository.create(item).await.expect("base item creation")
}

async fn request(app: &Router, method: Method, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn request_json(app: &Router, uri: &str, token: Option<&str>, body: &Value) -> Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.to_string()))
                .expect("JSON request"),
        )
        .await
        .expect("route response")
}

async fn json_response(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-emby-game-genres-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("temporary storage root");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
