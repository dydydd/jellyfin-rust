use std::collections::HashSet;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
    StartupConfigurationUpdate,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_localization_routes_";
const MAX_RESPONSE_SIZE: usize = 2 * 1024 * 1024;
const ROUTES: [&str; 3] = [
    "/Localization/Cultures",
    "/Localization/Countries",
    "/Localization/ParentalRatings",
];

#[tokio::test]
async fn localization_routes_use_official_resources_persisted_country_and_setup_policy() {
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
        exercise_localization_routes(&task_database_name).await;
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

async fn exercise_localization_routes(database_name: &str) {
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
    update_country(&configuration, "DE").await;
    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("localization-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let ordinary = users
        .create(&format!("localization-user-{suffix}"))
        .await
        .expect("ordinary user creation");
    let devices = DeviceRepository::new(database.clone());
    let administrator_token =
        create_session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = create_session(&devices, ordinary.id, &format!("user-{suffix}")).await;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("localization-key-{suffix}"))
        .await
        .expect("API key creation")
        .access_token;

    let route_app = app(database.clone());
    assert_anonymous_de_contract(&route_app).await;
    assert_eq!(
        post(&route_app, "/Startup/Complete", None).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_completed_authorization(&route_app, &administrator_token, &user_token, &api_key).await;

    update_country(&configuration, "US").await;
    let before_restart = body_json(
        get(
            &route_app,
            "/Localization/ParentalRatings",
            Some(&administrator_token),
        )
        .await,
    )
    .await;
    assert_us_ratings(&before_restart);

    let restarted = app(database.clone());
    assert_completed_authorization(&restarted, &administrator_token, &user_token, &api_key).await;
    let after_restart = body_json(
        get(
            &restarted,
            "/Localization/ParentalRatings",
            Some(&administrator_token),
        )
        .await,
    )
    .await;
    assert_eq!(after_restart, before_restart);
    assert_us_ratings(&after_restart);
    assert_cultures(
        &body_json(get(&restarted, "/Localization/Cultures", Some(&api_key)).await).await,
    );
    assert_countries(
        &body_json(get(&restarted, "/Localization/Countries", Some(&api_key)).await).await,
    );

    drop(route_app);
    drop(restarted);
    drop(configuration);
    database
        .close()
        .await
        .expect("temporary database connection must close");
}

async fn assert_anonymous_de_contract(app: &Router) {
    let cultures = get(app, "/Localization/Cultures", None).await;
    assert_eq!(cultures.status(), StatusCode::OK);
    assert_cultures(&body_json(cultures).await);

    let countries = get(app, "/Localization/Countries", None).await;
    assert_eq!(countries.status(), StatusCode::OK);
    assert_countries(&body_json(countries).await);

    let ratings = get(app, "/Localization/ParentalRatings", None).await;
    assert_eq!(ratings.status(), StatusCode::OK);
    assert_de_ratings(&body_json(ratings).await);
}

async fn assert_completed_authorization(
    app: &Router,
    administrator_token: &str,
    user_token: &str,
    api_key: &str,
) {
    for route in ROUTES {
        assert_eq!(
            get(app, route, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get(app, route, Some(user_token)).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(app, route, Some(administrator_token)).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            get(app, route, Some(api_key)).await.status(),
            StatusCode::OK
        );
    }
}

fn assert_cultures(body: &Value) {
    let cultures = body.as_array().expect("cultures array");
    assert_eq!(cultures.len(), 494);
    assert!(cultures.windows(2).all(|pair| {
        pair[0]["DisplayName"].as_str().unwrap() <= pair[1]["DisplayName"].as_str().unwrap()
    }));
    let distinct_names = cultures
        .iter()
        .map(|culture| {
            culture["DisplayName"]
                .as_str()
                .expect("culture display name")
                .to_lowercase()
        })
        .collect::<HashSet<_>>();
    assert_eq!(distinct_names.len(), cultures.len());
    assert!(cultures.iter().any(|culture| {
        culture
            == &json!({
                "Name": "German",
                "DisplayName": "German",
                "TwoLetterISOLanguageName": "de",
                "ThreeLetterISOLanguageName": "deu",
                "ThreeLetterISOLanguageNames": ["deu", "ger"]
            })
    }));
}

fn assert_countries(body: &Value) {
    let countries = body.as_array().expect("countries array");
    assert_eq!(countries.len(), 140);
    assert!(countries.iter().any(|country| {
        country
            == &json!({
                "Name": "DE",
                "DisplayName": "Germany",
                "TwoLetterISORegionName": "DE",
                "ThreeLetterISORegionName": "DEU"
            })
    }));
}

fn assert_de_ratings(body: &Value) {
    let ratings = body.as_array().expect("parental ratings array");
    assert_eq!(ratings.len(), 24);
    assert!(ratings.iter().any(|rating| {
        rating
            == &json!({
                "Name": "FSK-12",
                "Value": 12,
                "RatingScore": { "score": 12 }
            })
    }));
    assert!(ratings.iter().any(|rating| rating["Name"] == "Banned"));
}

fn assert_us_ratings(body: &Value) {
    let ratings = body.as_array().expect("parental ratings array");
    assert_eq!(ratings.len(), 56);
    assert!(ratings.iter().any(|rating| {
        rating
            == &json!({
                "Name": "TV-MA",
                "Value": 17,
                "RatingScore": { "score": 17, "subScore": 1 }
            })
    }));
    assert!(ratings.iter().any(|rating| rating["Name"] == "Banned"));
}

async fn update_country(repository: &ServerConfigurationRepository, country_code: &str) {
    repository
        .update_startup_configuration(StartupConfigurationUpdate {
            server_name: "Localization Test".to_owned(),
            ui_culture: "en-US".to_owned(),
            metadata_country_code: country_code.to_owned(),
            preferred_metadata_language: "en".to_owned(),
        })
        .await
        .expect("localization configuration update");
}

fn app(database: DatabaseConnection) -> Router {
    let repository = ServerConfigurationRepository::new(database.clone());
    jellyfin_api::router(
        AppState::new(
            database,
            "Localization Test".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_persistent_startup(repository),
    )
}

async fn create_session(devices: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Localization Tests",
            "1.0",
            "PostgreSQL",
            device_id,
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn get(app: &Router, route: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(route);
    if let Some(token) = token {
        request = request.header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post(app: &Router, route: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::post(route);
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
