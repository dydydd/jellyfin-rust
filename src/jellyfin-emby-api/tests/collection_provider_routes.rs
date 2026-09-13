use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    routing::get,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository,
    LinkedChildRepository, NewBaseItem, NewDevice, entities::item_value,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Collection Provider Tests\", DeviceId=\"emby-collection-provider\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_collection_provider_";

#[tokio::test]
async fn collection_provider_discovery_is_real_filtered_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");
    let task_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_name).await }).await;
    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator close");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database task cancelled: {error}");
    }
}

async fn exercise(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 12,
        min_connections: 1,
    })
    .await
    .expect("temporary database connection");
    jellyfin_data::migrate(&database)
        .await
        .expect("temporary migrations");
    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_provider_filters_and_paging().await;
    fixture.assert_authorization_and_policy().await;
    fixture.assert_protocol_isolation().await;
    fixture.mock_task.abort();
    database.close().await.expect("database close");
}

struct Fixture {
    emby: Router,
    jellyfin: Router,
    collection_id: Uuid,
    hidden_collection_id: Uuid,
    user_id: Uuid,
    other_user_id: Uuid,
    user_token: String,
    api_key: String,
    mock_task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("collection-provider-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("collection-provider-user-{suffix}"))
            .await
            .unwrap();
        let other = users
            .create(&format!("collection-provider-other-{suffix}"))
            .await
            .unwrap();
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
                    blocked_tags: vec!["Hidden Collection".to_owned()],
                    ..UserPolicy::default()
                },
            )
            .await
            .unwrap();

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.unwrap();
        let collection =
            create_item(&items, "BoxSet", "Provider Collection", root.id, Some(100)).await;
        let hidden = create_item(
            &items,
            "BoxSet",
            "Hidden Provider Collection",
            root.id,
            Some(101),
        )
        .await;
        ItemValueRepository::new(database.clone())
            .link(
                hidden.id,
                item_value::ItemValueType::Tags,
                "Hidden Collection",
            )
            .await
            .unwrap();
        let existing = create_item(&items, "Movie", "Existing movie", root.id, Some(2)).await;
        LinkedChildRepository::new(database.clone())
            .add_manual(collection.id, &[existing.id])
            .await
            .unwrap();

        let devices = DeviceRepository::new(database.clone());
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let _administrator_token =
            session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("collection-provider-key-{suffix}"))
            .await
            .unwrap()
            .access_token;
        let (base_url, mock_task) = mock_tmdb().await;
        let state = AppState::new(
            database,
            "Collection Provider Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_tmdb_api_key("test-key")
        .with_item_lookup_tmdb_base_url(base_url);
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            collection_id: collection.id,
            hidden_collection_id: hidden.id,
            user_id: user.id,
            other_user_id: other.id,
            user_token,
            api_key,
            mock_task,
        }
    }

    async fn assert_provider_filters_and_paging(&self) {
        let all = body_json(
            request(
                &self.emby,
                &format!(
                    "/emby/cOlLeCtIoNs/{}/pRoViDeRiTeMs?USERID={}&UnknownFilter=ignored",
                    self.collection_id, self.user_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        assert_eq!(all["TotalRecordCount"], 3, "{all}");
        assert_eq!(all["Items"][0]["ProviderIds"]["Tmdb"], "1");
        assert_eq!(all["Items"][1]["ProviderIds"]["Tmdb"], "2");
        assert_eq!(all["Items"][2]["ProviderIds"]["Tmdb"], "3");

        let present = body_json(
            request(
                &self.emby,
                &format!(
                    "/emby/Collections/{}/ProviderItems?UserId={}&isMissing=true&ISMISSING=false",
                    self.collection_id, self.user_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        assert_eq!(present["TotalRecordCount"], 1, "{present}");
        assert_eq!(present["Items"][0]["ProviderIds"]["Tmdb"], "2");

        let missing = body_json(
            request(
                &self.emby,
                &format!(
                    "/emby/Collections/{}/Missing?UserId={}&StartIndex=-1&Limit=1",
                    self.collection_id, self.user_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        assert_eq!(missing["TotalRecordCount"], 1, "{missing}");
        assert_eq!(missing["Items"][0]["ProviderIds"]["Tmdb"], "1");

        let with_unaired = body_json(
            request(
                &self.emby,
                &format!(
                    "/emby/Collections/{}/Missing?UserId={}&includeunaired=TRUE&Limit=0",
                    self.collection_id, self.user_id
                ),
                Some(&self.user_token),
            )
            .await,
        )
        .await;
        assert_eq!(with_unaired["TotalRecordCount"], 2, "{with_unaired}");
        assert_eq!(with_unaired["Items"], json!([]));
    }

    async fn assert_authorization_and_policy(&self) {
        let route = format!("/emby/Collections/{}/ProviderItems", self.collection_id);
        assert_eq!(
            request(&self.emby, &route, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                &format!("{route}?UserId={}", self.other_user_id),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &self.emby,
                &format!(
                    "/emby/Collections/{}/ProviderItems?UserId={}",
                    self.hidden_collection_id, self.user_id
                ),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(&self.emby, &route, Some(&self.api_key))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            request(
                &self.emby,
                &format!("{route}?StartIndex=2147483648"),
                Some(&self.api_key),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in [
            format!("/Collections/{}/ProviderItems", self.collection_id),
            format!("/api/Collections/{}/Missing", self.collection_id),
        ] {
            assert_eq!(
                request(&self.jellyfin, &path, Some(&self.api_key))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Emby collection provider route leaked at {path}"
            );
        }
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    tmdb_id: Option<i64>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = item_type == "BoxSet";
    item.data = tmdb_id.map(|id| json!({ "ProviderIds": { "Tmdb": id.to_string() } }));
    repository.create(item).await.unwrap()
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Collection Provider Tests",
            "1.0",
            "Test",
            format!("emby-collection-provider-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

async fn mock_tmdb() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/collection/{id}",
        get(|| async {
            Json(json!({
                "id": 100,
                "parts": [
                    {"id": 1, "media_type": "movie", "title": "Missing old", "release_date": "2000-01-01", "poster_path": "/one.jpg"},
                    {"id": 2, "media_type": "movie", "title": "Present", "release_date": "2001-01-01", "poster_path": "/two.jpg"},
                    {"id": 3, "media_type": "movie", "title": "Missing future", "release_date": "2999-01-01", "poster_path": "/three.jpg"},
                    {"id": 4, "media_type": "tv", "name": "Unsupported series"}
                ]
            }))
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn request(app: &Router, uri: &str, token: Option<&str>) -> axum::response::Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
