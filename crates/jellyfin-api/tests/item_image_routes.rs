use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    thread,
};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{TimeZone, Utc};
use image::{Rgba, RgbaImage};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemImageRepository, BaseItemImageType, BaseItemRepository,
    DatabaseConfig, DeviceRepository, NewBaseItem, NewBaseItemImage, NewDevice,
    entities::{base_item, base_item_image, user},
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Item Image Tests\", Device=\"Test\", DeviceId=\"item-images\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_item_image_routes_";
const ITEM_PATH: &str = "/media/image-info-test.mkv";
const IMAGE_TAG: &str = "fdcbd27b24b37e862315a492f0300d8c";

#[tokio::test]
async fn item_image_infos_match_official_postgres_contract() {
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
        exercise_item_image_infos(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn item_image_files_match_official_processing_and_cache_contract() {
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
        exercise_item_image_files(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn item_image_deletes_match_official_authorization_and_ordinal_contract() {
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
        exercise_item_image_deletes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn item_image_uploads_match_official_authorization_and_storage_contract() {
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
        exercise_item_image_uploads(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[tokio::test]
async fn item_image_index_updates_match_official_file_swap_contract() {
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
        exercise_item_image_index_updates(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.unwrap();
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_item_image_index_updates(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = format!(
        "/Items/{}/Images/Backdrop/0/Index?newIndex=1",
        fixture.item_id
    );
    assert_eq!(
        fixture.request(Method::POST, &route, &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request_with_token(Method::POST, &route, &fixture.ordinary_token)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request_with_token(
                Method::POST,
                &format!("/Items/{}/Images/Backdrop/0/Index", fixture.item_id),
                &fixture.token,
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let first_path = fixture.path("backdrop.png");
    let second_path = fixture.path("remote-backdrop.png");
    let first_bytes = fs::read(&first_path).unwrap();
    let second_bytes = fs::read(&second_path).unwrap();
    assert_eq!(
        fixture
            .request_with_token(Method::POST, &route, &fixture.token)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fs::read(&first_path).unwrap(), second_bytes);
    assert_eq!(fs::read(&second_path).unwrap(), first_bytes);
    let repository = BaseItemImageRepository::new(fixture.database.clone());
    let swapped = repository
        .list(fixture.item_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|image| image.image_type == BaseItemImageType::Backdrop)
        .collect::<Vec<_>>();
    assert_eq!(swapped[0].path, first_path);
    assert_eq!(swapped[1].path, second_path);
    assert_eq!((swapped[0].width, swapped[0].height), (None, None));
    assert_eq!((swapped[1].width, swapped[1].height), (None, None));

    fixture
        .database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jellyfin.base_item_images SET path = $4 \
             WHERE item_id = $1 AND image_type = $2 AND image_index = $3",
            [
                fixture.item_id.into(),
                BaseItemImageType::Backdrop.as_i16().into(),
                4_i32.into(),
                "https://images.example.invalid/remote.png".into(),
            ],
        ))
        .await
        .unwrap();
    let before_remote_noop = fs::read(&second_path).unwrap();
    assert_eq!(
        fixture
            .request_with_token(Method::POST, &route, &fixture.token)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fs::read(&second_path).unwrap(), before_remote_noop);

    for (image_type, index, new_index, expected) in [
        ("Backdrop", -1, 0, StatusCode::NO_CONTENT),
        ("Backdrop", 0, 99, StatusCode::NO_CONTENT),
        ("Primary", 0, 0, StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            fixture
                .request_with_token(
                    Method::POST,
                    &format!(
                        "/Items/{}/Images/{image_type}/{index}/Index?newIndex={new_index}",
                        fixture.item_id
                    ),
                    &fixture.token,
                )
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        fixture
            .request_with_token(
                Method::POST,
                &format!(
                    "/Items/{}/Images/Backdrop/0/Index?newIndex=1",
                    Uuid::new_v4()
                ),
                &fixture.token,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

async fn exercise_item_image_uploads(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let primary = format!("/Items/{}/Images/Primary", fixture.item_id);
    let png = fs::read(fixture.path("poster.png")).unwrap();
    let encoded = BASE64_STANDARD.encode(&png);

    assert_eq!(
        fixture
            .request_with_body(Method::POST, &primary, &[], encoded.as_bytes())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request_with_body(
                Method::POST,
                &primary,
                &[
                    ("X-Emby-Token", "invalid-token"),
                    ("Content-Type", "image/png")
                ],
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &primary,
                &fixture.ordinary_token,
                "image/png",
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &primary,
                &fixture.token,
                "text/plain",
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &primary,
                &fixture.token,
                "image/png; charset=utf-8",
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let repository = BaseItemImageRepository::new(fixture.database.clone());
    let first = repository
        .get(fixture.item_id, BaseItemImageType::Primary, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(&first.path).unwrap(), png);
    assert_eq!((first.width, first.height), (Some(8), Some(4)));
    assert!(first.path.contains("/metadata/library/"));
    assert!(Path::new(&fixture.path("poster.png")).exists());
    let reconnected = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 1,
        min_connections: 1,
    })
    .await
    .unwrap();
    assert_eq!(
        BaseItemImageRepository::new(reconnected.clone())
            .get(fixture.item_id, BaseItemImageType::Primary, 0)
            .await
            .unwrap()
            .unwrap()
            .path,
        first.path
    );
    reconnected.close().await.unwrap();
    let downloaded = fixture.request(Method::GET, &primary, &[]).await;
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(downloaded.into_body(), usize::MAX).await.unwrap(),
        png.as_slice()
    );

    let replacement = b"not actually an image";
    assert_eq!(
        fixture
            .request_with_body(
                Method::POST,
                &format!("{primary}/-999?api_key={}", fixture.api_key),
                &[("Content-Type", "image/avif")],
                BASE64_STANDARD.encode(replacement).as_bytes(),
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let second = repository
        .get(fixture.item_id, BaseItemImageType::Primary, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(&second.path).unwrap(), replacement);
    assert_eq!((second.width, second.height), (None, None));
    assert!(!Path::new(&first.path).exists());

    for index in [-1, 999] {
        assert_eq!(
            fixture
                .request_with_token_and_body(
                    Method::POST,
                    &format!("/Items/{}/Images/Backdrop/{index}", fixture.item_id),
                    &fixture.token,
                    "image/png",
                    encoded.as_bytes(),
                )
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let backdrops = repository
        .list(fixture.item_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|image| image.image_type == BaseItemImageType::Backdrop)
        .collect::<Vec<_>>();
    assert_eq!(
        backdrops
            .iter()
            .map(|image| image.image_index)
            .collect::<Vec<_>>(),
        [4, 9, 10, 11]
    );

    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &format!("/Items/{}/Images/Chapter", fixture.item_id),
                &fixture.token,
                "image/png",
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &format!("/Items/{}/Images/Primary", Uuid::new_v4()),
                &fixture.token,
                "image/png",
                encoded.as_bytes(),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &format!("/Items/{}/Images/Logo", fixture.empty_item_id),
                &fixture.token,
                "image/png",
                b"",
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let empty = repository
        .get(fixture.empty_item_id, BaseItemImageType::Logo, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::metadata(empty.path).unwrap().len(), 0);
    assert_eq!(
        fixture
            .request_with_token_and_body(
                Method::POST,
                &primary,
                &fixture.token,
                "image/png",
                b"!!!!",
            )
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    for content_type in [
        "image/apng",
        "image/avif",
        "image/bmp",
        "image/gif",
        "image/x-icon",
        "image/jpeg",
        "image/png",
        "image/svg+xml",
        "image/tiff",
        "image/webp",
    ] {
        assert_eq!(
            fixture
                .request_with_token_and_body(
                    Method::POST,
                    &format!("/Items/{}/Images/Banner", fixture.empty_item_id),
                    &fixture.token,
                    content_type,
                    b"",
                )
                .await
                .status(),
            StatusCode::NO_CONTENT,
            "content type {content_type}"
        );
    }

    fixture.cleanup().await;
}

async fn exercise_item_image_deletes(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let primary = format!("/Items/{}/Images/Primary", fixture.item_id);
    let backdrop = format!("/Items/{}/Images/Backdrop/0", fixture.item_id);

    assert_eq!(
        fixture
            .request(Method::DELETE, &primary, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &primary,
                &[("X-Emby-Token", "invalid-token")],
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request_with_token(Method::DELETE, &primary, &fixture.ordinary_token)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        fixture
            .request_with_token(Method::DELETE, &backdrop, &fixture.token)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(!Path::new(&fixture.path("backdrop.png")).exists());
    let remaining = BaseItemImageRepository::new(fixture.database.clone())
        .at(fixture.item_id, BaseItemImageType::Backdrop, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remaining.image_index, 9);

    let remote_path = "https://images.example.invalid/remote.png";
    fixture
        .database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jellyfin.base_item_images SET path = $4 \
             WHERE item_id = $1 AND image_type = $2 AND image_index = $3",
            [
                fixture.item_id.into(),
                2_i16.into(),
                9_i32.into(),
                remote_path.into(),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .request(
                Method::DELETE,
                &format!(
                    "/Items/{}/Images/Backdrop?ImageIndex=0&api_key={}",
                    fixture.item_id, fixture.api_key
                ),
                &[],
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(Path::new(&fixture.path("remote-backdrop.png")).exists());
    assert!(
        BaseItemImageRepository::new(fixture.database.clone())
            .at(fixture.item_id, BaseItemImageType::Backdrop, 0)
            .await
            .unwrap()
            .is_none()
    );

    assert_eq!(
        fixture
            .request_with_token(
                Method::DELETE,
                &format!("{primary}?imageIndex=0"),
                &fixture.token,
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(!Path::new(&fixture.path("poster.png")).exists());

    for route in [
        format!("/Items/{}/Images/Primary/99", fixture.item_id),
        format!("/Items/{}/Images/Primary/-1", fixture.item_id),
    ] {
        assert_eq!(
            fixture
                .request_with_token(Method::DELETE, &route, &fixture.token)
                .await
                .status(),
            StatusCode::NO_CONTENT,
            "route {route}"
        );
    }
    assert_eq!(
        fixture
            .request_with_token(
                Method::DELETE,
                &format!("/Items/{}/Images/Primary", Uuid::new_v4()),
                &fixture.token,
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .request_with_token(
                Method::DELETE,
                &format!("/Items/{}/Images/not-an-image", fixture.item_id),
                &fixture.token,
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    fixture.cleanup().await;
}

async fn exercise_item_image_files(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let primary = format!("/Items/{}/Images/Primary", fixture.item_id);

    let anonymous = fixture.request(Method::GET, &primary, &[]).await;
    assert_eq!(anonymous.status(), StatusCode::OK);
    assert_eq!(anonymous.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        anonymous.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );
    assert_eq!(anonymous.headers()[header::CACHE_CONTROL], "public");
    assert_eq!(anonymous.headers()[header::VARY], "Accept");
    assert_eq!(anonymous.headers()["transfermode.dlna.org"], "Interactive");
    assert_eq!(
        anonymous.headers()["realtimeinfo.dlna.org"],
        "DLNA.ORG_TLAG=*"
    );
    assert!(anonymous.headers().contains_key(header::LAST_MODIFIED));
    assert!(!anonymous.headers().contains_key(header::ETAG));
    assert!(!anonymous.headers().contains_key(header::ACCEPT_RANGES));
    let primary_bytes = to_bytes(anonymous.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        primary_bytes.as_ref(),
        fs::read(fixture.path("poster.png")).unwrap()
    );

    let lowercase = fixture
        .request(
            Method::GET,
            &format!("/Items/{}/Images/primary", fixture.item_id),
            &[],
        )
        .await;
    assert_eq!(lowercase.status(), StatusCode::OK);

    let invalid_token = fixture
        .request(Method::GET, &primary, &[("X-Emby-Token", "invalid-token")])
        .await;
    assert_eq!(invalid_token.status(), StatusCode::UNAUTHORIZED);

    let api_key = fixture
        .request(
            Method::GET,
            &format!("{primary}?api_key={}", fixture.api_key),
            &[],
        )
        .await;
    assert_eq!(api_key.status(), StatusCode::OK);

    let head = fixture.request(Method::HEAD, &primary, &[]).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        fs::metadata(fixture.path("poster.png"))
            .unwrap()
            .len()
            .to_string()
    );
    assert!(
        to_bytes(head.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );

    let range = fixture
        .request(
            Method::GET,
            &primary,
            &[(header::RANGE.as_str(), "bytes=0-2")],
        )
        .await;
    assert_eq!(range.status(), StatusCode::OK);
    assert!(!range.headers().contains_key(header::CONTENT_RANGE));

    let tagged_route = format!("{primary}?tag=immutable-tag");
    let tagged = fixture.request(Method::GET, &tagged_route, &[]).await;
    assert_eq!(tagged.status(), StatusCode::OK);
    assert_eq!(tagged.headers()[header::ETAG], "\"immutable-tag\"");
    assert_eq!(
        tagged.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    for validator in ["immutable-tag", "\"immutable-tag\""] {
        let cached = fixture
            .request(
                Method::GET,
                &tagged_route,
                &[(header::IF_NONE_MATCH.as_str(), validator)],
            )
            .await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        assert!(
            to_bytes(cached.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    let cached_by_date = fixture
        .request(
            Method::GET,
            &primary,
            &[(
                header::IF_MODIFIED_SINCE.as_str(),
                "Wed, 03 Jan 2024 00:00:00 GMT",
            )],
        )
        .await;
    assert_eq!(cached_by_date.status(), StatusCode::NOT_MODIFIED);

    let no_cache = fixture
        .request(
            Method::GET,
            &tagged_route,
            &[
                (header::CACHE_CONTROL.as_str(), "no-cache"),
                (header::IF_NONE_MATCH.as_str(), "immutable-tag"),
            ],
        )
        .await;
    assert_eq!(no_cache.status(), StatusCode::OK);
    assert_eq!(
        no_cache.headers()[header::CACHE_CONTROL],
        "no-cache, no-store, must-revalidate"
    );
    assert_eq!(
        no_cache.headers()[header::PRAGMA],
        "no-cache, no-store, must-revalidate"
    );
    assert!(!no_cache.headers().contains_key(header::LAST_MODIFIED));
    assert!(!no_cache.headers().contains_key(header::ETAG));

    let resized = fixture
        .request(
            Method::GET,
            &format!("{primary}?maxWidth=4&format=Jpg&quality=75"),
            &[(header::ACCEPT.as_str(), "image/jpeg")],
        )
        .await;
    assert_eq!(resized.status(), StatusCode::OK);
    assert_eq!(resized.headers()[header::CONTENT_TYPE], "image/jpeg");
    let resized_bytes = to_bytes(resized.into_body(), usize::MAX).await.unwrap();
    let decoded = image::load_from_memory(&resized_bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));

    let selected_by_query = fixture
        .request(
            Method::GET,
            &format!("/Items/{}/Images/Backdrop?imageIndex=1", fixture.item_id),
            &[],
        )
        .await;
    assert_eq!(selected_by_query.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(selected_by_query.into_body(), usize::MAX)
            .await
            .unwrap(),
        fs::read(fixture.path("remote-backdrop.png")).unwrap()
    );

    let webp = fixture
        .request(
            Method::GET,
            &format!("{primary}?maxWidth=4"),
            &[(header::ACCEPT.as_str(), "image/webp")],
        )
        .await;
    assert_eq!(webp.status(), StatusCode::OK);
    assert_eq!(webp.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(
        image::guess_format(&to_bytes(webp.into_body(), usize::MAX).await.unwrap()).unwrap(),
        image::ImageFormat::WebP
    );

    for route in [
        format!("{primary}?format=not-an-image"),
        format!("{primary}?quality=0"),
        format!("{primary}?quality=101"),
    ] {
        let invalid = fixture.request(Method::GET, &route, &[]).await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST, "route {route}");
    }

    let remote_bytes = fs::read(fixture.path("remote-backdrop.png")).unwrap();
    let (remote_url, remote_server) = serve_image_once(remote_bytes.clone());
    fixture
        .database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE jellyfin.base_item_images SET path = $4 \
             WHERE item_id = $1 AND image_type = $2 AND image_index = $3",
            [
                fixture.item_id.into(),
                2_i16.into(),
                9_i32.into(),
                remote_url.into(),
            ],
        ))
        .await
        .unwrap();
    let backdrop = fixture
        .request(
            Method::GET,
            &format!("/Items/{}/Images/Backdrop/1", fixture.item_id),
            &[],
        )
        .await;
    assert_eq!(backdrop.status(), StatusCode::OK);
    assert_eq!(backdrop.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        to_bytes(backdrop.into_body(), usize::MAX).await.unwrap(),
        remote_bytes
    );
    remote_server.join().unwrap();
    let relocated = base_item_image::Entity::find_by_id((fixture.item_id, 2, 9))
        .one(&fixture.database)
        .await
        .unwrap()
        .unwrap();
    assert!(!relocated.path.starts_with("http"));
    assert!(relocated.path.contains("cache/images/remote"));
    assert!(Path::new(&relocated.path).is_file());

    for route in [
        format!("/Items/{}/Images/Backdrop/2", fixture.item_id),
        format!("/Items/{}/Images/Primary/-1", fixture.item_id),
        format!("/Items/{}/Images/Primary", Uuid::new_v4()),
        format!("/Items/{}/Images/Logo", fixture.item_id),
    ] {
        let missing = fixture.request(Method::GET, &route, &[]).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND, "route {route}");
    }

    fixture.cleanup().await;
}

async fn exercise_item_image_infos(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = format!("/Items/{}/Images", fixture.item_id);

    let unauthenticated = fixture.get_without_auth(&route).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let missing = fixture
        .get(&format!("/Items/{}/Images", Uuid::new_v4()))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let empty = fixture
        .get(&format!("/Items/{}/Images", fixture.empty_item_id))
        .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(body_json(empty).await, json!([]));

    let response = fixture.get(&route).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        json!([
            {
                "ImageType": "Primary",
                "ImageIndex": null,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("poster.png"),
                "BlurHash": "primary-blurhash",
                "Height": 900,
                "Width": 600,
                "Size": fs::metadata(fixture.path("poster.png")).unwrap().len()
            },
            {
                "ImageType": "Logo",
                "ImageIndex": null,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("missing-logo.png"),
                "BlurHash": null,
                "Height": null,
                "Width": null,
                "Size": 0
            },
            {
                "ImageType": "Backdrop",
                "ImageIndex": 0,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("backdrop.png"),
                "BlurHash": "backdrop-blurhash",
                "Height": 1080,
                "Width": 1920,
                "Size": fs::metadata(fixture.path("backdrop.png")).unwrap().len()
            },
            {
                "ImageType": "Backdrop",
                "ImageIndex": 1,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("remote-backdrop.png"),
                "BlurHash": "remote-blurhash",
                "Height": 720,
                "Width": 1280,
                "Size": fs::metadata(fixture.path("remote-backdrop.png")).unwrap().len()
            },
            {
                "ImageType": "Chapter",
                "ImageIndex": 0,
                "ImageTag": IMAGE_TAG,
                "Path": fixture.path("chapter.png"),
                "BlurHash": "chapter-blurhash",
                "Height": 360,
                "Width": 640,
                "Size": fs::metadata(fixture.path("chapter.png")).unwrap().len()
            }
        ])
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    temporary: TempDirectory,
    user_id: Uuid,
    ordinary_user_id: Uuid,
    item_id: Uuid,
    empty_item_id: Uuid,
    token: String,
    api_key: String,
    ordinary_token: String,
}

impl Fixture {
    async fn new(database_name: &str) -> Self {
        let database = jellyfin_data::connect(&DatabaseConfig {
            url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
            max_connections: 4,
            min_connections: 1,
        })
        .await
        .expect("temporary PostgreSQL database must be available");
        jellyfin_data::migrate(&database)
            .await
            .expect("PostgreSQL migrations must succeed");

        let suffix = Uuid::new_v4().simple().to_string();
        let temporary = TempDirectory::new();
        image_fixture(&temporary.path().join("poster.png"), 8, 4, [255, 0, 0, 255]);
        image_fixture(
            &temporary.path().join("backdrop.png"),
            8,
            4,
            [0, 0, 255, 255],
        );
        image_fixture(
            &temporary.path().join("remote-backdrop.png"),
            4,
            2,
            [0, 255, 0, 255],
        );
        image_fixture(
            &temporary.path().join("chapter.png"),
            6,
            3,
            [255, 255, 0, 255],
        );

        let users = UserService::new(database.clone());
        let user = users
            .create_initial_administrator(&format!("item-image-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Item Image Tests",
                "1.0",
                "Test",
                format!("item-images-{suffix}"),
            ))
            .await
            .expect("administrator session")
            .access_token;
        let ordinary_user = users
            .create(&format!("item-image-user-{suffix}"))
            .await
            .expect("ordinary user creation");
        let ordinary_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                ordinary_user.id,
                "Item Image Tests",
                "1.0",
                "Test",
                format!("item-images-user-{suffix}"),
            ))
            .await
            .expect("ordinary user session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("item-image-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some("Image Info Movie".to_owned());
        item.media_type = Some("Video".to_owned());
        item.path = Some(ITEM_PATH.to_owned());
        let item = items.create(item).await.expect("movie item creation");

        let mut empty_item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        empty_item.name = Some("No Images Movie".to_owned());
        empty_item.media_type = Some("Video".to_owned());
        empty_item.path = Some("/media/no-images.mkv".to_owned());
        let empty_item = items
            .create(empty_item)
            .await
            .expect("empty movie item creation");

        let modified = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).single().unwrap();
        BaseItemImageRepository::new(database.clone())
            .replace(
                item.id,
                &[
                    image(
                        BaseItemImageType::Primary,
                        0,
                        temporary.path().join("poster.png"),
                        modified,
                        Some((600, 900)),
                        Some("primary-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Backdrop,
                        4,
                        temporary.path().join("backdrop.png"),
                        modified,
                        Some((1920, 1080)),
                        Some("backdrop-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Backdrop,
                        9,
                        temporary.path().join("remote-backdrop.png"),
                        modified,
                        Some((1280, 720)),
                        Some("remote-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Logo,
                        0,
                        temporary.path().join("missing-logo.png"),
                        modified,
                        Some((400, 200)),
                        Some("missing-blurhash"),
                    ),
                    image(
                        BaseItemImageType::Chapter,
                        8,
                        temporary.path().join("chapter.png"),
                        modified,
                        Some((640, 360)),
                        Some("chapter-blurhash"),
                    ),
                ],
            )
            .await
            .expect("image metadata replacement");

        let app = jellyfin_api::router(
            AppState::new(
                database.clone(),
                "Item Image Test Server".to_owned(),
                "http://127.0.0.1:8096".to_owned(),
            )
            .with_storage_paths(
                temporary.path().join("programdata"),
                temporary.path().join("web"),
                temporary.path().join("cache/images"),
                temporary.path().join("cache"),
                temporary.path().join("metadata"),
            ),
        );
        Self {
            database,
            app,
            temporary,
            user_id: user.id,
            ordinary_user_id: ordinary_user.id,
            item_id: item.id,
            empty_item_id: empty_item.id,
            token,
            api_key,
            ordinary_token,
        }
    }

    async fn get(&self, uri: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::get(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{}\"", self.token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_without_auth(&self, uri: &str) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn request(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn request_with_token(
        &self,
        method: Method,
        uri: &str,
        token: &str,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn request_with_body(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.app
            .clone()
            .oneshot(request.body(Body::from(body.to_vec())).unwrap())
            .await
            .unwrap()
    }

    async fn request_with_token_and_body(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        content_type: &str,
        body: &[u8],
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(
                        header::AUTHORIZATION,
                        format!("{AUTHORIZATION}, Token=\"{token}\""),
                    )
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    fn path(&self, name: &str) -> String {
        self.temporary
            .path()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.is_in([self.item_id, self.empty_item_id]))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.user_id, self.ordinary_user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
    }
}

fn image(
    image_type: BaseItemImageType,
    image_index: u32,
    path: PathBuf,
    date_modified: chrono::DateTime<Utc>,
    dimensions: Option<(u32, u32)>,
    blurhash: Option<&str>,
) -> NewBaseItemImage {
    NewBaseItemImage {
        image_type,
        image_index,
        path: path.to_string_lossy().into_owned(),
        date_modified,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
        blurhash: blurhash.map(str::to_owned),
    }
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn image_fixture(path: &Path, width: u32, height: u32, color: [u8; 4]) {
    RgbaImage::from_pixel(width, height, Rgba(color))
        .save(path)
        .expect("write image fixture");
}

fn serve_image_once(bytes: Vec<u8>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{address}/remote.png"), server)
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-item-image-api-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
