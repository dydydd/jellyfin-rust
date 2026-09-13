use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, ItemValueRepository,
    NewBaseItem, NewDevice, entities::item_value,
};
use jellyfin_model::UserPolicy;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Facet Tests\", DeviceId=\"emby-facet-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_facets_";

#[tokio::test]
async fn discovery_facets_use_real_policy_visible_postgres_values() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.strip_prefix(DATABASE_PREFIX).is_some_and(
        |suffix| suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    ));
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
        .expect("temporary database migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_visible_facets().await;
    fixture.assert_global_facets_and_paging().await;
    fixture.assert_features_and_protocol_isolation().await;
    database.close().await.expect("database cleanup");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_token: String,
    admin_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("facet-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("facet-user-{suffix}"))
            .await
            .expect("user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let visible_folder = create_item(
            &items,
            "CollectionFolder",
            "Visible library",
            root.id,
            true,
            None,
            None,
        )
        .await;
        let hidden_folder = create_item(
            &items,
            "CollectionFolder",
            "Hidden library",
            root.id,
            true,
            None,
            None,
        )
        .await;
        let visible_movie = create_item(
            &items,
            "Movie",
            "alpha movie",
            visible_folder.id,
            false,
            Some("PG-13"),
            Some(json!({"Container": "mkv, webm"})),
        )
        .await;
        let visible_audio = create_item(
            &items,
            "Audio",
            "beta song",
            visible_folder.id,
            false,
            Some("PG"),
            Some(json!({"container": "mp3"})),
        )
        .await;
        let hidden_video = create_item(
            &items,
            "Video",
            "zulu hidden",
            hidden_folder.id,
            false,
            Some("R"),
            Some(json!({"Container": "avi"})),
        )
        .await;
        let blocked_by_tag = create_item(
            &items,
            "Audio",
            "gamma blocked tag",
            visible_folder.id,
            false,
            Some("NC-17"),
            Some(json!({"Container": "mov"})),
        )
        .await;

        let item_values = ItemValueRepository::new(database.clone());
        item_values
            .link(
                visible_movie.id,
                item_value::ItemValueType::Artist,
                "Alpha Artist",
            )
            .await
            .expect("visible artist relation");
        item_values
            .link(
                hidden_video.id,
                item_value::ItemValueType::AlbumArtist,
                "Zulu Artist",
            )
            .await
            .expect("hidden artist relation");
        item_values
            .link(
                blocked_by_tag.id,
                item_value::ItemValueType::Tags,
                "Blocked Facet",
            )
            .await
            .expect("blocked tag relation");
        item_values
            .link(
                blocked_by_tag.id,
                item_value::ItemValueType::Artist,
                "Gamma Artist",
            )
            .await
            .expect("blocked artist relation");

        insert_stream(
            &database,
            visible_movie.id,
            0,
            0,
            Some("aac"),
            Some("stereo"),
            None,
            None,
            None,
        )
        .await;
        insert_stream(
            &database,
            visible_movie.id,
            1,
            1,
            None,
            None,
            Some("smpte2084"),
            None,
            Some(true),
        )
        .await;
        insert_stream(
            &database,
            visible_audio.id,
            0,
            0,
            Some("mp3"),
            Some("mono"),
            None,
            None,
            None,
        )
        .await;
        insert_stream(
            &database,
            hidden_video.id,
            0,
            0,
            Some("flac"),
            Some("5.1"),
            None,
            None,
            None,
        )
        .await;
        insert_stream(
            &database,
            blocked_by_tag.id,
            0,
            0,
            Some("ogg"),
            Some("7.1"),
            None,
            None,
            None,
        )
        .await;
        insert_stream(
            &database,
            hidden_video.id,
            1,
            1,
            None,
            None,
            Some("arib-std-b67"),
            Some(8),
            None,
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
                    enabled_folders: vec![visible_folder.id],
                    blocked_tags: vec!["Blocked Facet".to_owned()],
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("visible-folder policy");

        let devices = DeviceRepository::new(database.clone());
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let admin_token = session(&devices, administrator.id, &format!("admin-{suffix}")).await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("facet-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Facet Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_token,
            admin_token,
            api_key,
        }
    }

    async fn assert_visible_facets(&self) {
        assert_names(
            request_json(&self.emby, "/emby/AudioCodecs", Some(&self.user_token)).await,
            &["aac", "mp3"],
        );
        assert_names(
            request_json(&self.emby, "/emby/audiolayouts", Some(&self.user_token)).await,
            &["mono", "stereo"],
        );
        assert_names(
            request_json(&self.emby, "/emby/containers", Some(&self.user_token)).await,
            &["mkv", "mp3", "webm"],
        );
        assert_names(
            request_json(
                &self.emby,
                "/emby/extendedvideotypes",
                Some(&self.user_token),
            )
            .await,
            &["Hdr10", "Hdr10Plus"],
        );
        assert_names(
            request_json(&self.emby, "/emby/officialratings", Some(&self.user_token)).await,
            &["PG", "PG-13"],
        );
        assert_names(
            request_json(&self.emby, "/emby/itemtypes", Some(&self.user_token)).await,
            &["Audio", "CollectionFolder", "Movie", "UserRootFolder"],
        );

        let prefixes = request_json(
            &self.emby,
            "/emby/Items/Prefixes?IncludeItemTypes=Movie,Audio",
            Some(&self.user_token),
        )
        .await;
        assert_eq!(prefix_values(&prefixes), ["A", "B"]);
        assert_eq!(prefixes[0], json!({"Name": "A", "Value": "A"}));
        let artists =
            request_json(&self.emby, "/emby/artists/prefixes", Some(&self.user_token)).await;
        assert_eq!(prefix_values(&artists), ["A"]);
    }

    async fn assert_global_facets_and_paging(&self) {
        let global = request_json(&self.emby, "/emby/AudioCodecs", Some(&self.api_key)).await;
        assert_names(global, &["aac", "flac", "mp3", "ogg"]);

        let empty_page = request_json(
            &self.emby,
            "/emby/AudioCodecs?Limit=0",
            Some(&self.user_token),
        )
        .await;
        assert_eq!(empty_page["Items"], json!([]));
        assert_eq!(empty_page["TotalRecordCount"], 2);

        let second = request_json(
            &self.emby,
            "/emby/AudioCodecs?StartIndex=1&Limit=1",
            Some(&self.user_token),
        )
        .await;
        assert_names_with_total(second, &["mp3"], 2);

        let second_prefix = request_json(
            &self.emby,
            "/emby/Items/Prefixes?IncludeItemTypes=Movie,Audio&StartIndex=1&Limit=1",
            Some(&self.user_token),
        )
        .await;
        assert_eq!(prefix_values(&second_prefix), ["B"]);

        let unlimited = request_json(
            &self.emby,
            "/emby/AudioCodecs?StartIndex=-2&Limit=-1",
            Some(&self.user_token),
        )
        .await;
        assert_names(unlimited, &["aac", "mp3"]);
        assert_eq!(
            request(
                &self.emby,
                "/emby/Items/Prefixes?Limit=2147483648",
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_features_and_protocol_isolation(&self) {
        assert_eq!(
            request(&self.emby, "/emby/Features", None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&self.emby, "/emby/Features", Some(&self.user_token))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        for token in [&self.admin_token, &self.api_key] {
            let response = request(&self.emby, "/emby/fEaTuReS", Some(token)).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response_json(response).await, json!([]));
        }
        for path in ["/AudioCodecs", "/api/AudioCodecs", "/ItemTypes"] {
            assert_eq!(
                request(&self.jellyfin, path, Some(&self.api_key))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{path} must remain Emby-only"
            );
        }
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
    is_folder: bool,
    official_rating: Option<&str>,
    data: Option<Value>,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = is_folder;
    item.official_rating = official_rating.map(str::to_owned);
    item.data = data;
    repository.create(item).await.expect("base item creation")
}

#[allow(clippy::too_many_arguments)]
async fn insert_stream(
    database: &DatabaseConnection,
    item_id: Uuid,
    stream_index: i32,
    stream_type: i16,
    codec: Option<&str>,
    layout: Option<&str>,
    transfer: Option<&str>,
    dv_profile: Option<i32>,
    hdr10_plus: Option<bool>,
) {
    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jellyfin.media_streams (\
                 item_id, stream_index, stream_type, codec, channel_layout, color_transfer, \
                 dv_profile, hdr10_plus_present_flag, is_default, is_forced, is_external, is_original\
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, false, false, false)",
            [
                item_id.into(),
                stream_index.into(),
                stream_type.into(),
                codec.map(str::to_owned).into(),
                layout.map(str::to_owned).into(),
                transfer.map(str::to_owned).into(),
                dv_profile.into(),
                hdr10_plus.into(),
            ],
        ))
        .await
        .expect("media stream creation");
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Facet Tests",
            "1.0",
            "Test",
            format!("facet-device-{suffix}"),
        ))
        .await
        .expect("device session")
        .access_token
}

fn assert_names(value: Value, expected: &[&str]) {
    assert_names_with_total(value, expected, expected.len());
}

fn assert_names_with_total(value: Value, expected: &[&str], total: usize) {
    assert_eq!(
        value["Items"]
            .as_array()
            .expect("Items array")
            .iter()
            .map(|item| item["Name"].as_str().expect("Name"))
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(value["TotalRecordCount"], total);
}

fn prefix_values(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("prefix array")
        .iter()
        .map(|pair| pair["Value"].as_str().expect("Value"))
        .collect()
}

async fn request(app: &axum::Router, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request
            .header(header::AUTHORIZATION, AUTHORIZATION)
            .header("x-emby-token", token);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn request_json(app: &axum::Router, uri: &str, token: Option<&str>) -> Value {
    let response = request(app, uri, token).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    response_json(response).await
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
