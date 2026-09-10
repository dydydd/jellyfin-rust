use std::path::PathBuf;

use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::MediaAttachmentService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, NewBaseItem};
use jellyfin_model::MediaAttachment;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const DATABASE_PREFIX: &str = "jellyfin_video_attachment_routes_";

#[tokio::test]
async fn video_attachment_route_serves_persisted_attachment_file() {
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
        exercise_video_attachment_route(&task_database_name).await;
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

async fn exercise_video_attachment_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    let response = get(&fixture.app, &Fixture::route(fixture.item_id, 4)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-font-ttf"
    );
    assert_eq!(
        body_bytes(response).await,
        Bytes::from_static(b"test attachment bytes")
    );
    for route in [
        format!(
            "/Videos/{}/{}/Attachments/4/Stream",
            fixture.item_id,
            fixture.item_id.simple()
        ),
        format!(
            "/videos/{}/{}/attachments/4/stream",
            fixture.item_id,
            fixture.item_id.simple()
        ),
    ] {
        let response = get(&fixture.app, &route).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(
            body_bytes(response).await,
            Bytes::from_static(b"test attachment bytes"),
            "{route}"
        );
    }

    let alternate_source = fixture
        .alternate_id
        .simple()
        .to_string()
        .to_ascii_uppercase();
    let response = get(
        &fixture.app,
        &Fixture::route_with_source(fixture.item_id, &alternate_source, 4),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(response).await,
        Bytes::from_static(b"alternate attachment bytes")
    );
    let lowercase_response = get(
        &fixture.app,
        &format!(
            "/videos/{}/{alternate_source}/attachments/4",
            fixture.item_id
        ),
    )
    .await;
    assert_eq!(lowercase_response.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(lowercase_response).await,
        Bytes::from_static(b"alternate attachment bytes")
    );
    assert_eq!(
        get(
            &fixture.app,
            &Fixture::route_with_source(
                fixture.item_id,
                &fixture.outsider_id.simple().to_string(),
                4,
            ),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &fixture.app,
            &Fixture::route_with_source(fixture.item_id, &fixture.alternate_id.to_string(), 4,),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &fixture.app,
            &Fixture::route_with_source(fixture.item_id, "not-a-media-source", 4),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        get(&fixture.app, &Fixture::route(Uuid::new_v4(), 4))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&fixture.app, &Fixture::route(fixture.item_id, 99))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    tokio::fs::remove_file(&fixture.attachment_path)
        .await
        .expect("remove attachment file");
    assert_eq!(
        get(&fixture.app, &Fixture::route(fixture.item_id, 4))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    item_id: Uuid,
    alternate_id: Uuid,
    outsider_id: Uuid,
    attachment_path: PathBuf,
    storage_root: PathBuf,
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
        let storage_root =
            std::env::temp_dir().join(format!("jellyfin-video-attachment-routes-{suffix}"));
        tokio::fs::create_dir_all(&storage_root)
            .await
            .expect("fixture storage directory");
        let attachment_path = storage_root.join("font.ttf");
        let alternate_attachment_path = storage_root.join("alternate-font.ttf");
        let outsider_attachment_path = storage_root.join("outsider-font.ttf");
        tokio::fs::write(&attachment_path, b"test attachment bytes")
            .await
            .expect("attachment fixture file");
        tokio::fs::write(&alternate_attachment_path, b"alternate attachment bytes")
            .await
            .expect("alternate attachment fixture file");
        tokio::fs::write(&outsider_attachment_path, b"outsider attachment bytes")
            .await
            .expect("outsider attachment fixture file");

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Attachment Movie {suffix}"));
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Attachment Movie {suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie item creation");
        let mut alternate = NewBaseItem::new(Uuid::new_v4(), "Movie");
        alternate.name = Some(format!("Attachment Movie Alternate {suffix}"));
        alternate.media_type = Some("Video".to_owned());
        alternate.path = Some(format!("/media/Attachment Movie {suffix} - 4K.mkv"));
        let alternate = BaseItemRepository::new(database.clone())
            .create(alternate)
            .await
            .expect("alternate movie item creation");
        let mut outsider = NewBaseItem::new(Uuid::new_v4(), "Movie");
        outsider.name = Some(format!("Attachment Outsider {suffix}"));
        outsider.media_type = Some("Video".to_owned());
        outsider.path = Some(format!("/media/Attachment Outsider {suffix}.mkv"));
        let outsider = BaseItemRepository::new(database.clone())
            .create(outsider)
            .await
            .expect("outsider movie item creation");
        let items = BaseItemRepository::new(database.clone());
        items
            .assign_local_alternate_versions(&[(alternate.id, item.id)])
            .await
            .expect("alternate movie assignment");
        let attachments = MediaAttachmentService::new(database.clone());
        attachments
            .save_media_attachments(
                item.id,
                vec![MediaAttachment {
                    index: 4,
                    codec: Some("ttf".to_owned()),
                    file_name: Some("font.ttf".to_owned()),
                    mime_type: Some("application/x-font-ttf".to_owned()),
                    delivery_url: Some(attachment_path.to_string_lossy().into_owned()),
                    ..MediaAttachment::default()
                }],
            )
            .await
            .expect("media attachment creation");
        attachments
            .save_media_attachments(
                alternate.id,
                vec![MediaAttachment {
                    index: 4,
                    codec: Some("ttf".to_owned()),
                    file_name: Some("alternate-font.ttf".to_owned()),
                    mime_type: Some("application/x-font-ttf".to_owned()),
                    delivery_url: Some(alternate_attachment_path.to_string_lossy().into_owned()),
                    ..MediaAttachment::default()
                }],
            )
            .await
            .expect("alternate media attachment creation");
        attachments
            .save_media_attachments(
                outsider.id,
                vec![MediaAttachment {
                    index: 4,
                    codec: Some("ttf".to_owned()),
                    file_name: Some("outsider-font.ttf".to_owned()),
                    mime_type: Some("application/x-font-ttf".to_owned()),
                    delivery_url: Some(outsider_attachment_path.to_string_lossy().into_owned()),
                    ..MediaAttachment::default()
                }],
            )
            .await
            .expect("outsider media attachment creation");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Video Attachment Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            item_id: item.id,
            alternate_id: alternate.id,
            outsider_id: outsider.id,
            attachment_path,
            storage_root,
        }
    }

    fn route(item_id: Uuid, index: i32) -> String {
        format!("/Videos/{item_id}/{}/Attachments/{index}", item_id.simple())
    }

    fn route_with_source(item_id: Uuid, media_source_id: &str, index: i32) -> String {
        format!("/Videos/{item_id}/{media_source_id}/Attachments/{index}")
    }

    async fn cleanup(self) {
        BaseItemRepository::new(self.database.clone())
            .delete_many(&[self.item_id, self.alternate_id, self.outsider_id])
            .await
            .expect("item cleanup");
        let _ = tokio::fs::remove_dir_all(&self.storage_root).await;
        self.database.close().await.unwrap();
    }
}

async fn get(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_bytes(response: axum::response::Response) -> Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body")
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
