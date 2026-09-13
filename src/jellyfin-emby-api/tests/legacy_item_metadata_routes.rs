use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice,
    NewTrickplayInfo, TrickplayInfoRepository,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Legacy Metadata Tests\", DeviceId=\"emby-legacy-metadata-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_legacy_metadata_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn critic_reviews_and_thumbnail_set_preserve_real_data_and_sdk_contracts() {
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
        exercise_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    administrator.close().await.expect("database pool cleanup");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

async fn exercise_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_critic_reviews().await;
    fixture.assert_thumbnail_set().await;
    fixture.assert_protocol_isolation(database.clone()).await;

    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    emby: axum::Router,
    user_token: String,
    api_key: String,
    visible_item_id: Uuid,
    hidden_item_id: Uuid,
    missing_item_id: Uuid,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let user = users
            .create(&format!("legacy-metadata-user-{suffix}"))
            .await
            .expect("user creation");
        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let allowed_folder = create_item(
            &items,
            "CollectionFolder",
            "Allowed metadata",
            root.id,
            true,
        )
        .await;
        let hidden_folder =
            create_item(&items, "CollectionFolder", "Hidden metadata", root.id, true).await;
        let visible_item = create_item(
            &items,
            "Video",
            "Visible metadata item",
            allowed_folder.id,
            false,
        )
        .await;
        let hidden_item = create_item(
            &items,
            "Video",
            "Hidden metadata item",
            hidden_folder.id,
            false,
        )
        .await;

        users
            .update_policy(
                user.id,
                &UserPolicy {
                    authentication_provider_id: Some(
                        UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.to_owned(),
                    ),
                    password_reset_provider_id: Some(
                        UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.to_owned(),
                    ),
                    enable_all_folders: false,
                    enabled_folders: vec![allowed_folder.id],
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("scoped user policy");
        let user_token = DeviceRepository::new(database.clone())
            .create_session(NewDevice::new(
                user.id,
                "Emby Legacy Metadata Tests",
                "1.0",
                "Test",
                format!("legacy-metadata-device-{suffix}"),
            ))
            .await
            .expect("device session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("legacy-metadata-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;

        TrickplayInfoRepository::new(database.clone())
            .upsert(
                visible_item.id,
                NewTrickplayInfo {
                    width: 320,
                    height: 180,
                    tile_width: 2,
                    tile_height: 2,
                    thumbnail_count: 8,
                    interval: 1_500,
                    bandwidth: 12_000,
                },
            )
            .await
            .expect("persisted composite trickplay metadata");

        let state = AppState::new(
            database,
            "Emby Legacy Metadata Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state),
            user_token,
            api_key,
            visible_item_id: visible_item.id,
            hidden_item_id: hidden_item.id,
            missing_item_id: Uuid::new_v4(),
        }
    }

    async fn assert_critic_reviews(&self) {
        let canonical = format!("/emby/Items/{}/CriticReviews", self.visible_item_id);
        assert_eq!(
            request(&self.emby, &canonical, None).await.status(),
            StatusCode::UNAUTHORIZED
        );

        for route in [
            format!("{canonical}?StartIndex=-2147483648&Limit=2147483647"),
            format!(
                "/emby/items/{}/criticreviews?startindex=2147483647&limit=-2147483648",
                self.visible_item_id
            ),
            format!(
                "/emby/iTeMs/{}/cRiTiCrEvIeWs?sTaRtInDeX=-1&lImIt=0",
                self.visible_item_id
            ),
        ] {
            let response = request(&self.emby, &route, Some(&self.user_token)).await;
            assert_eq!(response.status(), StatusCode::OK, "{route}");
            let body = response_json(response).await;
            assert_eq!(body, json!({"Items": [], "TotalRecordCount": 0}));
            assert!(body.get("StartIndex").is_none(), "Emby's SDK DTO omits it");
        }

        for query in [
            "StartIndex=invalid",
            "StartIndex=2147483648",
            "Limit=-2147483649",
        ] {
            assert_eq!(
                request(
                    &self.emby,
                    &format!("{canonical}?{query}"),
                    Some(&self.user_token),
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST,
                "{query}"
            );
        }

        // Item visibility/typed lookup intentionally precedes paging binding.
        for item_id in [self.hidden_item_id, self.missing_item_id] {
            assert_eq!(
                request(
                    &self.emby,
                    &format!("/emby/Items/{item_id}/CriticReviews?StartIndex=invalid"),
                    Some(&self.user_token),
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            request(
                &self.emby,
                &format!("{canonical}?api_key={}", self.api_key),
                None,
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    async fn assert_thumbnail_set(&self) {
        let canonical = format!("/emby/Items/{}/ThumbnailSet", self.visible_item_id);
        assert_eq!(
            request(&self.emby, &canonical, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede required Width binding"
        );

        for route in [
            canonical.clone(),
            format!(
                "/emby/items/{}/thumbnailset?width=invalid",
                self.visible_item_id
            ),
            format!(
                "/emby/iTeMs/{}/tHuMbNaIlSeT?wIdTh=2147483648",
                self.visible_item_id
            ),
        ] {
            assert_eq!(
                request(&self.emby, &route, Some(&self.user_token))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST,
                "{route}"
            );
        }

        // A real persisted 320px trickplay sprite cannot truthfully populate
        // Emby's individually addressable ThumbnailInfo image tags.
        for route in [
            format!("{canonical}?Width=320"),
            format!(
                "/emby/items/{}/thumbnailset?width=-2147483648",
                self.visible_item_id
            ),
            format!(
                "/emby/iTeMs/{}/tHuMbNaIlSeT?wIdTh=2147483647",
                self.visible_item_id
            ),
            format!("/emby/Items/{}/ThumbnailSet?Width=320", self.hidden_item_id),
            format!(
                "/emby/Items/{}/ThumbnailSet?Width=320",
                self.missing_item_id
            ),
        ] {
            let response = request(&self.emby, &route, Some(&self.user_token)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
            assert!(
                to_bytes(response.into_body(), 1024)
                    .await
                    .expect("thumbnail response body")
                    .is_empty(),
                "unavailable ThumbnailSet must not contain fabricated metadata"
            );
        }
        assert_eq!(
            request(
                &self.emby,
                &format!("{canonical}?Width=320&api_key={}", self.api_key),
                None,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    async fn assert_protocol_isolation(&self, database: DatabaseConnection) {
        let jellyfin = jellyfin_api::router(AppState::new(
            database,
            "Jellyfin Legacy Metadata Isolation".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        for route in [
            format!("/Items/{}/CriticReviews", self.visible_item_id),
            format!("/Items/{}/ThumbnailSet?Width=320", self.visible_item_id),
        ] {
            assert_eq!(
                request(&jellyfin, &route, Some(&self.user_token))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{route}"
            );
        }
    }
}

async fn create_item(
    items: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    is_folder: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = is_folder;
    items.create(item).await.expect("item creation")
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("route response")
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
