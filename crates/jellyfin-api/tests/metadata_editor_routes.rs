use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, NewMediaPath,
    NewVirtualFolder, ServerConfigurationRepository, StartupConfigurationUpdate,
    VirtualFolderRepository,
};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_metadata_editor_";
const MAX_RESPONSE_SIZE: usize = 2 * 1024 * 1024;

#[tokio::test]
async fn metadata_editor_route_uses_persisted_localization_provider_and_content_types() {
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
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let configuration = ServerConfigurationRepository::new(database.clone());
    configuration
        .update_startup_configuration(StartupConfigurationUpdate {
            server_name: "Metadata Editor Test".to_owned(),
            ui_culture: "en-US".to_owned(),
            metadata_country_code: "us".to_owned(),
            preferred_metadata_language: "en".to_owned(),
        })
        .await
        .expect("localization configuration update");
    VirtualFolderRepository::new(database.clone())
        .create(
            NewVirtualFolder {
                name: "Movies".to_owned(),
                collection_type: Some("movies".to_owned()),
                library_options: json!({}),
                refresh_requested: false,
            },
            vec![NewMediaPath {
                path: "/Media/Movies".to_owned(),
                normalized_path: "/media/movies".to_owned(),
                ancestors: vec!["/media/movies".to_owned(), "/media".to_owned()],
                path_info: json!({ "Path": "/Media/Movies" }),
            }],
        )
        .await
        .expect("virtual folder creation");
    let mut movie = NewBaseItem::new(Uuid::new_v4(), "Movie");
    movie.path = Some("/media/movies/Title.mkv".to_owned());
    let movie = BaseItemRepository::new(database.clone())
        .create(movie)
        .await
        .expect("movie creation");
    configuration
        .update_content_type_override("/MEDIA/MOVIES", Some("tvshows"))
        .await
        .expect("case-variant direct content type");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("metadata-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary = users
        .create(&format!("metadata-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let administrator_token =
        create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = create_session(&devices, ordinary.id, &format!("user-{suffix}")).await;
    let route_app = app(database.clone());
    assert_access(&route_app, movie.id, &administrator_token, &user_token).await;

    let response = get(&route_app, movie.id, Some(&administrator_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let configured = body_json(response).await;
    assert_real_contract(&configured);
    assert_eq!(configured["ContentType"], "tvshows");
    assert_eq!(option_values(&configured), ["", "movies", "tvshows"]);

    configuration
        .update_content_type_override("/media/movies", None)
        .await
        .expect("direct override removal");
    let inherited = body_json(get(&route_app, movie.id, Some(&administrator_token)).await).await;
    assert!(inherited.get("ContentType").is_none());
    assert_eq!(option_values(&inherited), Vec::<&str>::new());

    configuration
        .update_content_type_override("/MeDiA/MoViEs", Some("movies"))
        .await
        .expect("persisted direct override");
    let before_restart =
        body_json(get(&route_app, movie.id, Some(&administrator_token)).await).await;
    let restarted = app(database.clone());
    let after_restart =
        body_json(get(&restarted, movie.id, Some(&administrator_token)).await).await;
    assert_eq!(after_restart, before_restart);
    assert_eq!(after_restart["ContentType"], "movies");

    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_access(app: &Router, item_id: Uuid, administrator_token: &str, user_token: &str) {
    assert_eq!(
        get(app, item_id, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(app, item_id, Some(user_token)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(app, Uuid::new_v4(), Some(administrator_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

fn assert_real_contract(body: &Value) {
    let countries = body["Countries"].as_array().expect("countries array");
    assert_eq!(countries.len(), 140);
    assert!(countries.iter().any(|country| {
        country["TwoLetterISORegionName"] == "US" && country["ThreeLetterISORegionName"] == "USA"
    }));

    let cultures = body["Cultures"].as_array().expect("cultures array");
    assert_eq!(cultures.len(), 494);
    assert!(cultures.windows(2).all(|pair| {
        pair[0]["DisplayName"].as_str().unwrap() <= pair[1]["DisplayName"].as_str().unwrap()
    }));

    let ratings = body["ParentalRatingOptions"]
        .as_array()
        .expect("parental ratings array");
    assert!(
        ratings
            .iter()
            .any(|rating| { rating["Name"] == "PG-13" && rating["RatingScore"]["score"] == 13 })
    );
    assert!(ratings.iter().any(|rating| rating["Name"] == "Banned"));

    assert_eq!(
        body["ExternalIdInfos"],
        json!([
            { "Name": "IMDb", "Key": "Imdb" },
            { "Name": "TheMovieDb", "Key": "Tmdb", "Type": "Movie" },
            { "Name": "TheMovieDb", "Key": "TmdbCollection", "Type": "BoxSet" }
        ])
    );
}

fn option_values(body: &Value) -> Vec<&str> {
    body["ContentTypeOptions"]
        .as_array()
        .expect("content type options")
        .iter()
        .map(|option| option["Value"].as_str().expect("option value"))
        .collect()
}

fn app(database: sea_orm::DatabaseConnection) -> Router {
    jellyfin_api::router(AppState::new(
        database,
        "Metadata Editor Test".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ))
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Metadata Editor Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn get(app: &Router, item_id: Uuid, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(format!("/Items/{item_id}/MetadataEditor"));
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
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
