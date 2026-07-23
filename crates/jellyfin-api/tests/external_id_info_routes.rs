use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    entities::base_item,
};
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_external_id_infos_";
const MAX_RESPONSE_SIZE: usize = 64 * 1024;

#[tokio::test]
async fn external_id_infos_route_is_elevated_persisted_and_item_specific() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_route(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_route(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 6,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("external-id-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary = users
        .create(&format!("external-id-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let administrator_token =
        create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let ordinary_token = create_session(&devices, ordinary.id, &format!("user-{suffix}")).await;
    let movie = BaseItemRepository::new(database.clone())
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .expect("movie creation");
    let route_app = app(database.clone());

    assert_eq!(
        get(&route_app, movie.id, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&route_app, movie.id, Some(&ordinary_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&route_app, Uuid::new_v4(), Some(&administrator_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let response = get(&route_app, movie.id, Some(&administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let expected = json!([
        { "Name": "IMDb", "Key": "Imdb" },
        { "Name": "TheMovieDb", "Key": "Tmdb", "Type": "Movie" },
        { "Name": "TheMovieDb", "Key": "TmdbCollection", "Type": "BoxSet" }
    ]);
    assert_eq!(body_json(response).await, expected);

    assert_remote_search_contract(
        &route_app,
        &database,
        movie.id,
        &ordinary_token,
        &administrator_token,
    )
    .await;

    let restarted = app(database.clone());
    let response = get(&restarted, movie.id, Some(&administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, expected);

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_remote_search_contract(
    app: &Router,
    database: &sea_orm::DatabaseConnection,
    item_id: Uuid,
    ordinary_token: &str,
    administrator_token: &str,
) {
    let body = json!({
        "SearchInfo": {
            "Name": "Remote Candidate",
            "ProviderIds": { "Imdb": "tt0000001" }
        },
        "ItemId": item_id,
        "SearchProviderName": "Example",
        "IncludeDisabledProviders": true
    });
    for route in [
        "/Items/RemoteSearch/Movie",
        "/Items/RemoteSearch/Trailer",
        "/Items/RemoteSearch/MusicVideo",
        "/Items/RemoteSearch/Series",
        "/Items/RemoteSearch/BoxSet",
        "/Items/RemoteSearch/MusicArtist",
        "/Items/RemoteSearch/MusicAlbum",
        "/Items/RemoteSearch/Book",
    ] {
        assert_eq!(
            post_json(app, route, None, &body).await.status(),
            StatusCode::UNAUTHORIZED,
            "{route}"
        );
        let response = post_json(app, route, Some(ordinary_token), &body).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(
            body_json(response).await,
            Value::Array(Vec::new()),
            "{route}"
        );
    }

    let person = post_json(
        app,
        "/Items/RemoteSearch/Person",
        Some(ordinary_token),
        &body,
    )
    .await;
    assert_eq!(person.status(), StatusCode::FORBIDDEN);
    let person = post_json(
        app,
        "/Items/RemoteSearch/Person",
        Some(administrator_token),
        &body,
    )
    .await;
    assert_eq!(person.status(), StatusCode::OK);
    assert_eq!(body_json(person).await, Value::Array(Vec::new()));

    let invalid_body = post_raw_with_content_type(
        app,
        "/Items/RemoteSearch/Movie",
        Some(ordinary_token),
        b"not-json",
    )
    .await;
    assert_eq!(invalid_body.status(), StatusCode::BAD_REQUEST);

    let apply_body = json!({
        "Name": "Applied Candidate",
        "ProviderIds": {
            "Imdb": "tt7654321",
            "Tmdb": "98765"
        },
        "ProductionYear": 2026,
        "Artists": []
    });
    let apply_route = format!("/Items/RemoteSearch/Apply/{item_id}?replaceAllImages=false");
    assert_eq!(
        post_json(app, &apply_route, Some(ordinary_token), &apply_body)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_json(
            app,
            &format!("/Items/RemoteSearch/Apply/{}", Uuid::new_v4()),
            Some(administrator_token),
            &apply_body
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let applied = post_json(app, &apply_route, Some(administrator_token), &apply_body).await;
    assert_eq!(applied.status(), StatusCode::NO_CONTENT);
    let stored = base_item::Entity::find_by_id(item_id)
        .one(database)
        .await
        .expect("item lookup")
        .expect("item exists after apply");
    let metadata = stored.data.expect("metadata after apply");
    assert_eq!(metadata["ProviderIds"]["Imdb"], "tt7654321");
    assert_eq!(metadata["ProviderIds"]["Tmdb"], "98765");
}

fn app(database: sea_orm::DatabaseConnection) -> Router {
    jellyfin_api::router(AppState::new(
        database,
        "External Id Info Test".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "External Id Info Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn get(app: &Router, item_id: Uuid, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(format!("/Items/{item_id}/ExternalIdInfos"));
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_json(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: &Value,
) -> axum::response::Response {
    post_raw_with_content_type(app, uri, token, body.to_string().as_bytes()).await
}

async fn post_raw_with_content_type(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: &[u8],
) -> axum::response::Response {
    let mut request = Request::post(uri).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
