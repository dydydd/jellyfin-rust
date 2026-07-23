use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::{MediaStreamFilter, MediaStreamService, UserService};
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    entities::{base_item, user},
};
use jellyfin_model::{MediaStream, MediaStreamType};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Subtitle Tests\", Device=\"Test\", DeviceId=\"subtitle-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_subtitle_routes_";

#[tokio::test]
async fn delete_subtitle_route_requires_elevation_and_deletes_only_target_subtitle_stream() {
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
        exercise_delete_subtitle_route(&task_database_name).await;
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

async fn exercise_delete_subtitle_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;
    let route = Fixture::subtitle_route(fixture.item_id, 2);

    assert_eq!(
        fixture.send(Method::DELETE, &route, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.user_token))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .send(
                Method::DELETE,
                &Fixture::subtitle_route(Uuid::new_v4(), 2),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        fixture
            .send(Method::DELETE, &route, Some(&fixture.admin_token))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let streams = MediaStreamService::new(fixture.database.clone())
        .get_media_streams(MediaStreamFilter::for_item(fixture.item_id))
        .await
        .expect("media streams after delete");
    let remaining = streams
        .iter()
        .map(|stream| (stream.index, stream.stream_type))
        .collect::<Vec<_>>();
    assert_eq!(
        remaining,
        vec![
            (0, MediaStreamType::Video),
            (1, MediaStreamType::Audio),
            (3, MediaStreamType::Subtitle),
        ]
    );

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
    item_id: Uuid,
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
        let users = UserService::new(database.clone());
        let admin = users
            .create_initial_administrator(&format!("subtitle-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("subtitle-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = devices
            .create_session(NewDevice::new(
                admin.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-admin-{suffix}"),
            ))
            .await
            .expect("admin session")
            .access_token;
        let user_token = devices
            .create_session(NewDevice::new(
                user.id,
                "Subtitle Tests",
                "1.0",
                "Test",
                format!("subtitle-user-{suffix}"),
            ))
            .await
            .expect("user session")
            .access_token;

        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(format!("Subtitle Movie {suffix}"));
        item.media_type = Some("Video".to_owned());
        item.path = Some(format!("/media/Subtitle Movie {suffix}.mkv"));
        let item = BaseItemRepository::new(database.clone())
            .create(item)
            .await
            .expect("movie item creation");
        MediaStreamService::new(database.clone())
            .save_media_streams(
                item.id,
                &[
                    MediaStream {
                        index: 0,
                        stream_type: MediaStreamType::Video,
                        codec: Some("h264".to_owned()),
                        path: Some(format!("/media/Subtitle Movie {suffix}.mkv")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 1,
                        stream_type: MediaStreamType::Audio,
                        codec: Some("aac".to_owned()),
                        language: Some("eng".to_owned()),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 2,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("srt".to_owned()),
                        language: Some("eng".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.eng.srt")),
                        ..MediaStream::default()
                    },
                    MediaStream {
                        index: 3,
                        stream_type: MediaStreamType::Subtitle,
                        codec: Some("ass".to_owned()),
                        language: Some("jpn".to_owned()),
                        is_external: true,
                        path: Some(format!("/media/Subtitle Movie {suffix}.jpn.ass")),
                        ..MediaStream::default()
                    },
                ],
            )
            .await
            .expect("media stream creation");

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Subtitle Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
            item_id: item.id,
        }
    }

    fn subtitle_route(item_id: Uuid, index: i32) -> String {
        format!("/Videos/{item_id}/Subtitles/{index}")
    }

    async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(
                header::AUTHORIZATION,
                format!("{AUTHORIZATION}, Token=\"{token}\""),
            );
        }
        self.app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn cleanup(self) {
        base_item::Entity::delete_many()
            .filter(base_item::Column::Id.eq(self.item_id))
            .exec(&self.database)
            .await
            .expect("item cleanup");
        user::Entity::delete_many()
            .filter(user::Column::Id.is_in([self.admin_id, self.user_id]))
            .exec(&self.database)
            .await
            .expect("user cleanup");
        self.database.close().await.unwrap();
    }
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name[DATABASE_PREFIX.len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
}
