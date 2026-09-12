#![allow(clippy::too_many_lines)]
use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Channel Tests\", Device=\"Test\", DeviceId=\"channel-tests\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_channel_routes_";

#[tokio::test]
async fn channels_route_lists_persisted_channels_from_postgres() {
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
        exercise_channels_route(&task_database_name).await;
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

async fn exercise_channels_route(database_name: &str) {
    let fixture = Fixture::new(database_name).await;

    assert_eq!(
        fixture.get("/Channels", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels?userId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let body = body_json(
        fixture
            .get("/Channels?startIndex=1&limit=1", Some(&fixture.user_token))
            .await,
    )
    .await;
    assert_eq!(body["TotalRecordCount"], 2);
    assert_eq!(body["StartIndex"], 1);
    let items = body["Items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0]["Id"],
        fixture.second_channel_id.simple().to_string()
    );
    assert_eq!(items[0]["Name"], "B Channel");
    assert_eq!(items[0]["Type"], "Channel");

    let lowercase = body_json(
        fixture
            .get(
                "/channels?startindex=1&limit=1&supportslatestitems=true&supportsmediadeletion=false&isfavorite=false",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase["StartIndex"], 1);
    assert_eq!(lowercase["Items"].as_array().expect("items").len(), 1);

    let filtered = body_json(
        fixture
            .get(
                "/Channels?supportsLatestItems=true&supportsMediaDeletion=false&isFavorite=false",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(filtered["TotalRecordCount"], 2);
    assert!(
        filtered["Items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["Type"] == "Channel")
    );

    assert_signed_channel_pagination(&fixture).await;
    assert_channel_features(&fixture).await;
    assert_channel_items(&fixture).await;
    assert_latest_channel_items(&fixture).await;

    fixture.cleanup().await;
}

struct Fixture {
    database: DatabaseConnection,
    app: axum::Router,
    admin_id: Uuid,
    admin_token: String,
    user_token: String,
    first_channel_id: Uuid,
    second_channel_id: Uuid,
    movie_id: Uuid,
    channel_folder_id: Uuid,
    channel_folder_item_id: Uuid,
    outside_folder_id: Uuid,
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
            .create_initial_administrator(&format!("channel-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("channel-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("channel-admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("channel-user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let first_channel = create_item(&items, "Channel", "A Channel", root.id).await;
        let second_channel = create_item(&items, "Channel", "B Channel", root.id).await;
        let movie = create_item(&items, "Movie", "Ignored Movie", root.id).await;
        let _first_channel_item =
            create_item(&items, "Movie", "A Channel Movie", first_channel.id).await;
        let _second_channel_item =
            create_item(&items, "Video", "B Channel Video", first_channel.id).await;
        let channel_folder = create_folder(&items, "Nested Channel Folder", first_channel.id).await;
        let channel_folder_item =
            create_item(&items, "Audio", "Folder Song", channel_folder.id).await;
        let outside_folder = create_folder(&items, "Other Channel Folder", second_channel.id).await;

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Channel Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));
        Self {
            database,
            app,
            admin_id: admin.id,
            admin_token,
            user_token,
            first_channel_id: first_channel.id,
            second_channel_id: second_channel.id,
            movie_id: movie.id,
            channel_folder_id: channel_folder.id,
            channel_folder_item_id: channel_folder_item.id,
            outside_folder_id: outside_folder.id,
        }
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut request = Request::get(uri);
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
        self.database.close().await.unwrap();
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    repository.create(item).await.expect("item creation")
}

async fn create_folder(
    repository: &BaseItemRepository,
    name: &str,
    parent_id: Uuid,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), "Folder");
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = Some(parent_id);
    item.is_folder = true;
    repository.create(item).await.expect("folder creation")
}

async fn session(repository: &DeviceRepository, user_id: Uuid, device_id: &str) -> String {
    repository
        .create_session(NewDevice::new(
            user_id,
            "Channel Tests",
            "1.0",
            "Test",
            device_id,
        ))
        .await
        .expect("session creation")
        .access_token
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&body_bytes(response).await).expect("JSON response")
}

async fn assert_signed_channel_pagination(fixture: &Fixture) {
    // ChannelManager.GetChannelsInternalAsync uses List.GetRange rather than
    // InternalItemsQuery. Its non-positive limits are unlimited, while invalid
    // list offsets retain the official server-error behavior.
    for (route, start_index_name, limit_name) in [
        ("/Channels", "StartIndex", "Limit"),
        ("/channels", "startindex", "limit"),
    ] {
        for start_index in [-1_i64, i64::from(i32::MIN), 3, i64::from(i32::MAX)] {
            let uri = format!("{route}?{start_index_name}={start_index}");
            assert_eq!(
                fixture.get(&uri, Some(&fixture.user_token)).await.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{uri}"
            );
        }

        let end = body_json(
            fixture
                .get(
                    &format!("{route}?{start_index_name}=2"),
                    Some(&fixture.user_token),
                )
                .await,
        )
        .await;
        assert_page(&end, 2, 2, 0);

        for limit in [0, -1, i32::MIN, i32::MAX] {
            let uri = format!("{route}?{limit_name}={limit}");
            let page = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
            assert_page(&page, 0, 2, 2);
        }
    }

    let channel_item_routes = [
        format!("/Channels/{}/Items", fixture.first_channel_id),
        format!("/channels/{}/items", fixture.first_channel_id),
    ];
    assert_internal_channel_pagination(fixture, &channel_item_routes).await;
    let latest_routes = [
        "/Channels/Items/Latest".to_owned(),
        "/channels/items/latest".to_owned(),
    ];
    assert_internal_channel_pagination(fixture, &latest_routes).await;

    for route in [
        "/Channels".to_owned(),
        "/channels".to_owned(),
        format!("/Channels/{}/Items", fixture.first_channel_id),
        format!("/channels/{}/items", fixture.first_channel_id),
        "/Channels/Items/Latest".to_owned(),
        "/channels/items/latest".to_owned(),
    ] {
        for query in [
            "startIndex=2147483648",
            "startindex=-2147483649",
            "limit=2147483648",
            "Limit=-2147483649",
        ] {
            let uri = format!("{route}?{query}");
            assert_eq!(
                fixture.get(&uri, Some(&fixture.user_token)).await.status(),
                StatusCode::BAD_REQUEST,
                "{uri}"
            );
        }
    }
}

async fn assert_internal_channel_pagination(fixture: &Fixture, routes: &[String]) {
    for (index, route) in routes.iter().enumerate() {
        let (start_index_name, limit_name) = if index == 0 {
            ("StartIndex", "Limit")
        } else {
            ("startindex", "limit")
        };

        for start_index in [-1, i32::MIN] {
            let uri = format!("{route}?{start_index_name}={start_index}&{limit_name}=1");
            let page = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
            assert_page(&page, start_index, 3, 1);
        }

        let maximum_start = format!("{route}?{start_index_name}={}&{limit_name}=1", i32::MAX);
        let page = body_json(fixture.get(&maximum_start, Some(&fixture.user_token)).await).await;
        assert_page(&page, i32::MAX, 3, 0);

        for (limit, expected_items) in [(0, 0), (-1, 3), (i32::MIN, 3), (i32::MAX, 3)] {
            let uri = format!("{route}?{limit_name}={limit}");
            let page = body_json(fixture.get(&uri, Some(&fixture.user_token)).await).await;
            assert_page(&page, 0, 3, expected_items);
        }
    }
}

fn assert_page(page: &Value, start_index: i32, total_record_count: u64, item_count: usize) {
    assert_eq!(page["StartIndex"], start_index);
    assert_eq!(page["TotalRecordCount"], total_record_count);
    assert_eq!(page["Items"].as_array().expect("items").len(), item_count);
}

async fn assert_channel_features(fixture: &Fixture) {
    assert_eq!(
        fixture.get("/Channels/Features", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let all = body_json(
        fixture
            .get("/Channels/Features", Some(&fixture.user_token))
            .await,
    )
    .await;
    let features = all.as_array().expect("channel features");
    assert_eq!(features.len(), 2);
    assert_eq!(features[0]["Name"], "A Channel");
    assert_eq!(
        features[0]["Id"],
        fixture.first_channel_id.hyphenated().to_string()
    );
    assert_default_channel_features(&features[0]);
    assert_eq!(features[1]["Name"], "B Channel");
    assert_eq!(
        features[1]["Id"],
        fixture.second_channel_id.hyphenated().to_string()
    );

    let single = body_json(
        fixture
            .get(
                &format!("/Channels/{}/Features", fixture.second_channel_id),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(single["Name"], "B Channel");
    assert_eq!(
        single["Id"],
        fixture.second_channel_id.hyphenated().to_string()
    );
    assert_default_channel_features(&single);

    assert_eq!(
        fixture
            .get(
                &format!("/Channels/{}/Features", Uuid::new_v4()),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/{}/Features", fixture.movie_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_channel_items(fixture: &Fixture) {
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/{}/Items", fixture.first_channel_id),
                None,
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Channels/{}/Items?userId={}",
                    fixture.first_channel_id, fixture.admin_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Channels/{}/Items?userId={}",
                    fixture.first_channel_id,
                    Uuid::new_v4()
                ),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/{}/Items", Uuid::new_v4()),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/{}/Items", fixture.movie_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!(
                    "/Channels/{}/Items?folderId={}",
                    fixture.first_channel_id, fixture.outside_folder_id
                ),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let page = body_json(
        fixture
            .get(
                &format!(
                    "/Channels/{}/Items?startIndex=1&limit=1",
                    fixture.first_channel_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(page["TotalRecordCount"], 3);
    assert_eq!(page["StartIndex"], 1);
    let items = page["Items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["Name"], "B Channel Video");
    assert_eq!(items[0]["Type"], "Video");
    assert_eq!(
        items[0]["ParentId"],
        fixture.first_channel_id.simple().to_string()
    );

    let folder_page = body_json(
        fixture
            .get(
                &format!(
                    "/Channels/{}/Items?folderId={}",
                    fixture.first_channel_id, fixture.channel_folder_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(folder_page["TotalRecordCount"], 1);
    assert_eq!(folder_page["StartIndex"], 0);
    let folder_items = folder_page["Items"].as_array().expect("folder items");
    assert_eq!(folder_items.len(), 1);
    assert_eq!(
        folder_items[0]["Id"],
        fixture.channel_folder_item_id.simple().to_string()
    );
    assert_eq!(
        folder_items[0]["ParentId"],
        fixture.channel_folder_id.simple().to_string()
    );

    let lowercase = body_json(
        fixture
            .get(
                &format!(
                    "/channels/{}/items?folderid={}&startindex=0&sortby=name&sortorder=ascending",
                    fixture.first_channel_id, fixture.channel_folder_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase["TotalRecordCount"], 1);
}

async fn assert_latest_channel_items(fixture: &Fixture) {
    assert_eq!(
        fixture.get("/Channels/Items/Latest", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/Items/Latest?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/Items/Latest?userId={}", Uuid::new_v4()),
                Some(&fixture.admin_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Channels/Items/Latest?channelIds={}", fixture.movie_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let latest = body_json(
        fixture
            .get(
                "/Channels/Items/Latest?startIndex=1&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(latest["TotalRecordCount"], 3);
    assert_eq!(latest["StartIndex"], 1);
    let items = latest["Items"].as_array().expect("latest items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["Type"], "Video");
    assert_eq!(items[0]["Name"], "B Channel Video");

    let scoped_latest = body_json(
        fixture
            .get(
                &format!(
                    "/Channels/Items/Latest?channelIds={}&limit=3",
                    fixture.first_channel_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(scoped_latest["TotalRecordCount"], 3);
    let scoped_items = scoped_latest["Items"].as_array().expect("scoped items");
    assert_eq!(scoped_items.len(), 3);
    assert!(scoped_items.iter().all(|item| item["Type"] != "Folder"));
    assert!(
        scoped_items
            .iter()
            .any(|item| item["Id"] == fixture.channel_folder_item_id.simple().to_string())
    );

    let empty_channel = body_json(
        fixture
            .get(
                &format!(
                    "/Channels/Items/Latest?channelIds={}",
                    fixture.second_channel_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(empty_channel["TotalRecordCount"], 0);
    assert_eq!(empty_channel["Items"].as_array().expect("items").len(), 0);

    let lowercase = body_json(
        fixture
            .get(
                &format!(
                    "/channels/items/latest?channelids={}&startindex=0&limit=3",
                    fixture.first_channel_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase["TotalRecordCount"], 3);
}

fn assert_default_channel_features(features: &Value) {
    assert_eq!(features["CanSearch"], false);
    assert_eq!(features["MediaTypes"], Value::Array(Vec::new()));
    assert_eq!(features["ContentTypes"], Value::Array(Vec::new()));
    assert_eq!(features["MaxPageSize"], Value::Null);
    assert_eq!(features["AutoRefreshLevels"], Value::Null);
    assert_eq!(features["DefaultSortFields"], Value::Array(Vec::new()));
    assert_eq!(features["SupportsSortOrderToggle"], false);
    assert_eq!(features["SupportsLatestMedia"], false);
    assert_eq!(features["CanFilter"], true);
    assert_eq!(features["SupportsContentDownloading"], false);
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
