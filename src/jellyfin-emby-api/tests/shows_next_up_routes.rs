use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby NextUp Tests\", DeviceId=\"emby-next-up-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_next_up_";

#[tokio::test]
async fn emby_next_up_requires_generated_user_id_without_changing_jellyfin() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(
        database_name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task was cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let user = users
        .create_initial_administrator(&format!("next-up-admin-{suffix}"))
        .await
        .expect("administrator");
    let other = users
        .create(&format!("next-up-other-{suffix}"))
        .await
        .expect("other user");
    let token = DeviceRepository::new(database.clone())
        .create_session(NewDevice::new(
            user.id,
            "Emby NextUp Tests",
            "1.0",
            "Test",
            format!("emby-next-up-{suffix}"),
        ))
        .await
        .expect("administrator session")
        .access_token;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("emby-next-up-key-{suffix}"))
        .await
        .expect("API key")
        .access_token;

    let state = AppState::new(
        database.clone(),
        "Emby NextUp Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    );
    let jellyfin = jellyfin_api::router(state.clone());
    let emby = jellyfin_emby_api::router(state);

    assert_eq!(
        get(&emby, "/emby/Shows/NextUp", None).await.status(),
        StatusCode::UNAUTHORIZED,
        "authentication precedes the generated required query"
    );
    for query in [
        String::new(),
        "?Other=value".to_owned(),
        "?UserId=".to_owned(),
        "?UserId=not-a-guid".to_owned(),
        "?UserId=00000000000000000000000000000000".to_owned(),
        format!("?UserId={}&USERID=", user.id),
    ] {
        assert_eq!(
            get(&emby, &format!("/emby/Shows/NextUp{query}"), Some(&token),)
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "required UserId at {query}"
        );
    }

    let response = get(
        &emby,
        &format!(
            "/emby/sHoWs/nExTuP?USERID={}&uSeRiD={}&STARTINDEX=-1&LIMIT=0&FIELDS=Path&fields=ProviderIds&ENABLEIMAGETYPES=Primary&enableimagetypes=Backdrop",
            Uuid::new_v4(),
            user.id,
        ),
        Some(&token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response bytes"),
    )
    .expect("NextUp response JSON");
    assert_eq!(body["Items"], serde_json::json!([]));
    assert_eq!(body["StartIndex"], -1);

    assert_eq!(
        get(
            &emby,
            &format!("/emby/Shows/NextUp?UserId={}&Limit=0", user.id),
            Some(&api_key),
        )
        .await
        .status(),
        StatusCode::OK,
        "the generated API-key security scheme can use an explicit target user"
    );

    assert_eq!(
        get(
            &emby,
            &format!("/emby/Shows/NextUp?UserId={}&Limit=bad", user.id),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        get(
            &emby,
            &format!("/emby/Shows/NextUp?UserId={other}", other = other.id),
            Some(&token),
        )
        .await
        .status(),
        StatusCode::OK,
        "an administrator may target another existing user"
    );

    for path in ["/Shows/NextUp?Limit=0", "/api/Shows/NextUp?Limit=0"] {
        assert_eq!(
            get(&jellyfin, path, Some(&token)).await.status(),
            StatusCode::OK,
            "Jellyfin keeps its optional UserId contract at {path}"
        );
    }
    assert_eq!(
        get(
            &jellyfin,
            &format!("/Shows/NextUp?UserId={}&Limit=0", user.id),
            Some(&api_key),
        )
        .await
        .status(),
        StatusCode::OK,
        "an API key with an explicit target is valid on the shared controller too"
    );

    drop(emby);
    drop(jellyfin);
    database.close().await.expect("database cleanup");
}

async fn get(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}
