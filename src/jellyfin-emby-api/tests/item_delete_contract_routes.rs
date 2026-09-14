use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_data::{ApiKeyRepository, BaseItemRepository, DatabaseConfig, NewBaseItem};
use sea_orm::ConnectionTrait;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Item Delete Tests\", DeviceId=\"emby-item-delete\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_item_delete_";

#[tokio::test]
async fn generated_emby_item_delete_ids_contract_is_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
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
    let database = jellyfin_data::connect(&temporary_database_config(database_name))
        .await
        .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let suffix = Uuid::new_v4().simple().to_string();
    let token = ApiKeyRepository::new(database.clone())
        .create(&format!("emby-item-delete-{suffix}"))
        .await
        .expect("API key")
        .access_token;

    let items = BaseItemRepository::new(database.clone());
    let last_wins = create_item(&items, &suffix, "last-wins").await;
    let post_first = create_item(&items, &suffix, "post-first").await;
    let post_second = create_item(&items, &suffix, "post-second").await;
    let preserved = create_item(&items, &suffix, "preserved").await;

    let state = AppState::new(
        database.clone(),
        "Emby Item Delete Test Server".to_owned(),
        "http://127.0.0.1:18096".to_owned(),
    );
    let app = jellyfin_api::router(state.clone()).merge(jellyfin_emby_api::router(state));

    assert_eq!(
        request(&app, Method::DELETE, "/emby/Items", None).await,
        StatusCode::UNAUTHORIZED,
        "authentication must precede required query binding"
    );
    for (method, path) in [
        (Method::DELETE, "/emby/Items"),
        (Method::POST, "/emby/Items/Delete"),
    ] {
        for query in [
            "".to_owned(),
            "?Other=value".to_owned(),
            "?Ids=".to_owned(),
            "?IDS=%20".to_owned(),
            "?ids=invalid".to_owned(),
            format!("?Ids={preserved},"),
            format!("?Ids={preserved},,{preserved}"),
        ] {
            assert_eq!(
                request(
                    &app,
                    method.clone(),
                    &format!("{path}{query}"),
                    Some(&token),
                )
                .await,
                StatusCode::BAD_REQUEST,
                "strict {method} {path} Ids query {query}"
            );
        }
    }

    assert_eq!(
        request(
            &app,
            Method::DELETE,
            &format!("/emby/iTeMs?Ids=invalid&iDs={last_wins}"),
            Some(&token),
        )
        .await,
        StatusCode::OK,
    );
    assert!(
        items
            .get(last_wins)
            .await
            .expect("last-wins lookup")
            .is_none()
    );

    assert_eq!(
        request(
            &app,
            Method::POST,
            &format!("/emby/iTeMs/dElEtE?IDS={post_first},{post_second}"),
            Some(&token),
        )
        .await,
        StatusCode::OK,
    );
    assert!(items.get(post_first).await.expect("first lookup").is_none());
    assert!(
        items
            .get(post_second)
            .await
            .expect("second lookup")
            .is_none()
    );

    assert_eq!(
        request(
            &app,
            Method::DELETE,
            &format!("/emby/Items?Ids={preserved}&IDS=invalid"),
            Some(&token),
        )
        .await,
        StatusCode::BAD_REQUEST,
    );
    assert!(
        items
            .get(preserved)
            .await
            .expect("preserved lookup")
            .is_some()
    );

    for (method, path) in [
        (Method::DELETE, "/Items"),
        (Method::DELETE, "/api/Items"),
        (Method::POST, "/Items/Delete"),
        (Method::POST, "/api/Items/Delete"),
    ] {
        assert_eq!(
            request(&app, method, path, Some(&token)).await,
            StatusCode::NO_CONTENT,
            "Jellyfin omitted Ids remain a successful no-op at {path}"
        );
    }
    assert!(
        items
            .get(preserved)
            .await
            .expect("root isolation lookup")
            .is_some()
    );

    database.close().await.expect("database pool cleanup");
}

async fn create_item(items: &BaseItemRepository, suffix: &str, name: &str) -> Uuid {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
    item.name = Some(format!("Emby delete {name}"));
    item.media_type = Some("Video".to_owned());
    item.path = Some(format!("/tmp/emby-item-delete-{suffix}-{name}.mkv"));
    items.create(item).await.expect("item creation").id
}

async fn request(app: &axum::Router, method: Method, uri: &str, token: Option<&str>) -> StatusCode {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
        .status()
}

fn temporary_database_config(database_name: &str) -> DatabaseConfig {
    let mut config = DatabaseConfig::default();
    let (prefix, _) = config
        .url
        .rsplit_once('/')
        .expect("database URL must include a database name");
    config.url = format!("{prefix}/{database_name}");
    config.max_connections = 8;
    config.min_connections = 1;
    config
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
