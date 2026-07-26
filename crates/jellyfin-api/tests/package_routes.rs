use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice, ServerConfigurationRepository,
    entities::{api_key, user},
};
use jellyfin_model::PackageInfo;
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Package Tests\", DeviceId=\"package-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_package_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn package_routes_match_official_contract() {
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
        exercise_package_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_package_routes(database_name: &str) {
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

    let package_id = Uuid::from_u128(0x1f75_3b4d_22f1_4fed_9e30_4cc8_14d1_0a11);
    let alternate_id = Uuid::from_u128(0x3df8_5706_b6de_459a_9177_17a5_6145_a4ec);
    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Package Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_packages(vec![
            package("Bookshelf", package_id),
            package("Playback Reporting", alternate_id),
        ]),
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("package-admin-{suffix}"))
        .await
        .expect("administrator creation");
    let user = users
        .create(&format!("package-user-{suffix}"))
        .await
        .expect("user creation");
    let devices = DeviceRepository::new(database.clone());
    let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
    let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("package-key-{suffix}"))
        .await
        .expect("API key creation");

    for route in ["/Packages", "/Packages/Bookshelf", "/Repositories"] {
        assert_eq!(
            request(&app, route, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, route, Some(&user_token)).await.status(),
            StatusCode::FORBIDDEN
        );
    }

    let packages = body_json(request(&app, "/Packages", Some(&admin_token)).await).await;
    assert_eq!(packages.as_array().expect("packages").len(), 2);
    assert_eq!(packages[0]["name"], "Bookshelf");
    assert_eq!(packages[0]["guid"], package_id.simple().to_string());
    assert!(packages[0].get("Name").is_none());

    let by_name = body_json(request(&app, "/Packages/bookshelf", Some(&admin_token)).await).await;
    assert_eq!(by_name["name"], "Bookshelf");
    assert_eq!(by_name["guid"], package_id.simple().to_string());

    let by_guid = body_json(
        request(
            &app,
            &format!(
                "/Packages/not-the-name?assemblyGuid={}",
                alternate_id.hyphenated()
            ),
            Some(&admin_token),
        )
        .await,
    )
    .await;
    assert_eq!(by_guid["name"], "Playback Reporting");
    assert_eq!(by_guid["guid"], alternate_id.simple().to_string());

    assert_eq!(
        request(&app, "/Packages/Missing", Some(&admin_token))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        mutation(&app, Method::POST, "/Packages/Installed/Bookshelf", None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mutation(
            &app,
            Method::POST,
            "/Packages/Installed/Bookshelf",
            Some(&user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_no_content(
        mutation(
            &app,
            Method::POST,
            "/Packages/Installed/Bookshelf",
            Some(&admin_token),
        )
        .await,
    )
    .await;

    assert_no_content(
        mutation(
            &app,
            Method::POST,
            &format!("/Packages/Installed/not-the-name?assemblyGuid={alternate_id}"),
            Some(&admin_token),
        )
        .await,
    )
    .await;
    assert_no_content(
        mutation(
            &app,
            Method::POST,
            "/Packages/Installed/Bookshelf?version=1.0.0.0&repositoryUrl=https://repo.jellyfin.org/files/plugin/manifest.json",
            Some(&admin_token),
        )
        .await,
    )
    .await;
    assert_no_content(
        mutation(
            &app,
            Method::POST,
            "/Packages/Installed/Bookshelf?version=1.0.0.0&repositoryUrl=HTTPS://REPO.JELLYFIN.ORG/FILES/PLUGIN/MANIFEST.JSON",
            Some(&admin_token),
        )
        .await,
    )
    .await;

    for route in [
        "/Packages/Installed/Missing",
        "/Packages/Installed/Bookshelf?version=9.9.9.9",
        "/Packages/Installed/Bookshelf?repositoryUrl=https://repo.example.test/manifest.json",
    ] {
        assert_eq!(
            mutation(&app, Method::POST, route, Some(&admin_token))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    assert_eq!(
        mutation(
            &app,
            Method::DELETE,
            &format!("/Packages/Installing/{package_id}"),
            None,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mutation(
            &app,
            Method::DELETE,
            &format!("/Packages/Installing/{package_id}"),
            Some(&user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_no_content(
        mutation(
            &app,
            Method::DELETE,
            &format!("/Packages/Installing/{package_id}"),
            Some(&admin_token),
        )
        .await,
    )
    .await;

    let repositories = body_json(request(&app, "/Repositories", Some(&admin_token)).await).await;
    assert_eq!(repositories, json!([]));

    assert_eq!(
        post_repositories(&app, "/Repositories", None, repositories_fixture())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_repositories(
            &app,
            "/Repositories",
            Some(&user_token),
            repositories_fixture(),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    assert_no_content(
        post_repositories(
            &app,
            "/Repositories",
            Some(&admin_token),
            repositories_fixture(),
        )
        .await,
    )
    .await;

    let repositories = body_json(request(&app, "/Repositories", Some(&admin_token)).await).await;
    assert_eq!(
        repositories,
        json!([
            {
                "Name": "Jellyfin Stable",
                "Url": "https://repo.jellyfin.org/files/plugin/manifest.json",
                "Enabled": true
            }
        ])
    );
    let persisted = ServerConfigurationRepository::new(database.clone())
        .load()
        .await
        .expect("server configuration load");
    assert_eq!(persisted.plugin_repositories, repositories);

    let api_key_packages = body_json(
        request(
            &app,
            &format!("/Packages?api_key={}", api_key.access_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(api_key_packages[1]["name"], "Playback Reporting");

    api_key::Entity::delete_many()
        .exec(&database)
        .await
        .expect("package route api key cleanup");
    user::Entity::delete_many()
        .exec(&database)
        .await
        .expect("package route user cleanup");
    database.close().await.expect("database pool cleanup");
}

fn package(name: &str, id: Uuid) -> PackageInfo {
    PackageInfo {
        name: name.to_owned(),
        description: format!("{name} long description"),
        overview: format!("{name} overview"),
        owner: "Jellyfin".to_owned(),
        category: "General".to_owned(),
        id,
        versions: vec![json!({
            "version": "1.0.0.0",
            "repositoryUrl": "https://repo.jellyfin.org/files/plugin/manifest.json"
        })],
        image_url: Some(format!(
            "https://repo.example.test/{}.png",
            name.to_ascii_lowercase().replace(' ', "-")
        )),
    }
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Package Tests",
            "1.0",
            "Test",
            format!("package-tests-{suffix}"),
        ))
        .await
        .expect("device session creation")
        .access_token
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    raw_request(app, Method::GET, uri, token, Body::empty()).await
}

async fn mutation(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> axum::response::Response {
    raw_request(app, method, uri, token, Body::empty()).await
}

async fn raw_request(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn post_repositories(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> axum::response::Response {
    let mut request = Request::post(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn assert_no_content(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .is_empty()
    );
}

fn repositories_fixture() -> Value {
    json!([
        {
            "Name": "Jellyfin Stable",
            "Url": "https://repo.jellyfin.org/files/plugin/manifest.json",
            "Enabled": true
        }
    ])
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

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
