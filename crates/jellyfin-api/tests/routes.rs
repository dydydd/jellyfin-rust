use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_data::entities::user;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, sea_query::Expr};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn system_routes_follow_the_public_contract() {
    let database = test_database().await;
    let app = jellyfin_api::router(AppState::new(
        database,
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let response = app
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "Healthy");

    let response = app
        .clone()
        .oneshot(
            Request::get("/System/Info/Public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = body_json(response).await;
    assert_eq!(body["ServerName"], "Test Server");
    assert_eq!(body["LocalAddress"], "http://127.0.0.1:8096");
    assert_eq!(body["ProductName"], "Jellyfin Server");
    assert_eq!(body["StartupWizardCompleted"], false);
    assert_eq!(body["Id"].as_str().unwrap().len(), 32);
    assert!(body.get("server_name").is_none());

    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/System/Ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "Jellyfin Server");
    }
}

#[tokio::test]
async fn user_routes_create_and_filter_users_with_pascal_case_dtos() {
    let database = test_database().await;
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let username = format!("api-route-{}", Uuid::new_v4().simple());

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": username }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    assert_eq!(created["Name"], username);
    assert_eq!(created["Policy"]["IsHidden"], true);
    assert_eq!(
        created["Policy"]["AuthenticationProviderId"],
        "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
    );
    assert!(created.get("name").is_none());
    let id = Uuid::parse_str(created["Id"].as_str().unwrap()).unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": username }),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "response body: {body}");
    assert_eq!(body["Message"], "A user with that name already exists");

    let response = app
        .clone()
        .oneshot(Request::get("/Users/Public").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let public_users = body_json(response).await;
    assert!(public_users.as_array().unwrap().iter().all(|item| {
        item["Id"]
            .as_str()
            .is_none_or(|value| value != id.simple().to_string())
    }));

    user::Entity::update_many()
        .col_expr(user::Column::IsHidden, Expr::value(false))
        .col_expr(
            user::Column::Policy,
            Expr::value(json!({
                "AuthenticationProviderId": "test.provider",
                "EnableContentDeletion": true
            })),
        )
        .col_expr(
            user::Column::Preferences,
            Expr::value(json!({ "DisplayMissingEpisodes": true })),
        )
        .filter(user::Column::Id.eq(id))
        .exec(&database)
        .await
        .expect("test user must be made public");

    let response = app
        .clone()
        .oneshot(Request::get("/Users/Public").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let public_users = body_json(response).await;
    let public_user = public_users
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == id.simple().to_string())
        .expect("newly visible user must be returned");
    assert_eq!(public_user["Name"], username);
    assert_eq!(public_user["Policy"]["IsHidden"], false);
    assert_eq!(public_user["Policy"]["EnableContentDeletion"], true);
    assert_eq!(public_user["Configuration"]["DisplayMissingEpisodes"], true);

    user::Entity::delete_many()
        .filter(user::Column::Id.eq(id))
        .exec(&database)
        .await
        .expect("created test user must be removable");
}

#[tokio::test]
async fn create_user_maps_invalid_names_and_json_to_bad_request() {
    let app = jellyfin_api::router(AppState::new(
        DatabaseConnection::Disconnected,
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let response = app
        .clone()
        .oneshot(json_request("POST", "/Users/New", &json!({ "Name": " " })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["Message"], "Invalid username");

    let response = app
        .oneshot(json_request("POST", "/Users/New", &json!({ "Name": null })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["Message"], "Invalid request body");
}

#[tokio::test]
async fn health_reports_service_unavailable_when_database_is_disconnected() {
    let database = test_database().await;
    let closed_database = database.clone();
    database.close().await.unwrap();
    let app = jellyfin_api::router(AppState::new(
        closed_database,
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_text(response).await, "Unhealthy");
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

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
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

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
