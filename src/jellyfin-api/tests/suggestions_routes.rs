#![allow(clippy::too_many_lines)]
use std::collections::BTreeSet;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Suggestions Tests\", DeviceId=\"suggestions-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_suggestions_routes_";
const MAX_RESPONSE_SIZE: usize = 1024 * 1024;

#[tokio::test]
async fn suggestions_routes_match_official_auth_filters_and_count_contract() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.get("/Items/Suggestions", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture.get("/items/suggestions", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Items/Suggestions?userId={}", fixture.admin_id),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let suggestions = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&type=Movie&enableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(suggestions["StartIndex"], 0);
    assert_eq!(suggestions["TotalRecordCount"], 2);
    assert_eq!(
        item_names(&suggestions),
        BTreeSet::from([
            format!("Nested Movie {}", fixture.suffix),
            format!("Root Movie {}", fixture.suffix),
        ])
    );
    assert!(
        suggestions["Items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["Type"] == "Movie" && item["MediaType"] == "Video")
    );

    let lowercase_suggestions = body_json(
        fixture
            .get(
                "/items/suggestions?mediatype=Video&type=Movie&enabletotalrecordcount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(
        lowercase_suggestions["StartIndex"],
        suggestions["StartIndex"]
    );
    assert_eq!(
        lowercase_suggestions["TotalRecordCount"],
        suggestions["TotalRecordCount"]
    );
    assert_eq!(item_names(&lowercase_suggestions), item_names(&suggestions));

    let limited = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&type=Movie&limit=1",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(limited["Items"].as_array().unwrap().len(), 1);
    assert_eq!(
        limited["TotalRecordCount"], 1,
        "official QueryResult uses the returned item count when total counts are disabled"
    );

    let legacy = body_json(
        fixture
            .get(
                &format!(
                    "/Users/{}/Suggestions?MediaType=Audio&Type=Audio",
                    fixture.user_id
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(legacy["TotalRecordCount"], 1);
    assert_eq!(
        item_names(&legacy),
        BTreeSet::from([format!("Audio {}", fixture.suffix)])
    );

    let lowercase_legacy = body_json(
        fixture
            .get(
                &format!(
                    "/users/{}/suggestions?mediatype=Audio&type=Audio",
                    fixture.user_id
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(lowercase_legacy["StartIndex"], legacy["StartIndex"]);
    assert_eq!(
        lowercase_legacy["TotalRecordCount"],
        legacy["TotalRecordCount"]
    );
    assert_eq!(item_names(&lowercase_legacy), item_names(&legacy));

    let legacy_route = format!("/Users/{}/Suggestions", fixture.user_id);
    let enum_routes = [
        ("/Items/Suggestions".to_owned(), fixture.user_token.as_str()),
        (legacy_route.clone(), fixture.admin_token.as_str()),
    ];
    let expected_movies = BTreeSet::from([
        format!("Nested Movie {}", fixture.suffix),
        format!("Root Movie {}", fixture.suffix),
    ]);
    for (route, token) in &enum_routes {
        for query in [
            "MediaType=video&Type=movie&EnableTotalRecordCount=true",
            "mediaType=VIDEO&type=MOVIE&enableTotalRecordCount=true",
            "mediatype=video&type=movie&enabletotalrecordcount=true",
        ] {
            let page = body_json(fixture.get(&format!("{route}?{query}"), Some(token)).await).await;
            assert_eq!(page["TotalRecordCount"], 2);
            assert_eq!(item_names(&page), expected_movies);
        }

        let numeric = body_json(
            fixture
                .get(
                    &format!("{route}?mediaType=1&type=13&enableTotalRecordCount=true"),
                    Some(token),
                )
                .await,
        )
        .await;
        assert_eq!(numeric["TotalRecordCount"], 2);
        assert_eq!(item_names(&numeric), expected_movies);
    }

    let expected_all = BTreeSet::from([
        format!("Audio {}", fixture.suffix),
        format!("Episode {}", fixture.suffix),
        format!("Folder {}", fixture.suffix),
        format!("Nested Movie {}", fixture.suffix),
        format!("Root Movie {}", fixture.suffix),
    ]);
    let mut expected_global = expected_all.clone();
    expected_global.insert("Root".to_owned());
    for (route, expected) in [
        (
            "/Items/Suggestions?mediaType=invalid&type=invalid&enableTotalRecordCount=true"
                .to_owned(),
            &expected_global,
        ),
        (
            format!(
                "/Users/{}/Suggestions?MediaType=99&Type=99&EnableTotalRecordCount=true",
                fixture.user_id
            ),
            &expected_all,
        ),
    ] {
        let page = body_json(fixture.get(&route, Some(&fixture.admin_token)).await).await;
        assert_eq!(page["TotalRecordCount"], expected.len());
        assert_eq!(&item_names(&page), expected);
    }

    let comma_delimited = body_json(
        fixture
            .get(
                &format!(
                    "/Users/{}/Suggestions?MediaType=Video%2CAudio&Type=Movie%2CAudio&EnableTotalRecordCount=true",
                    fixture.user_id
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(comma_delimited["TotalRecordCount"], 3);
    assert_eq!(
        item_names(&comma_delimited),
        BTreeSet::from([
            format!("Audio {}", fixture.suffix),
            format!("Nested Movie {}", fixture.suffix),
            format!("Root Movie {}", fixture.suffix),
        ])
    );

    let repeated_boundary = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video%2CAudio&mediaType=Audio&type=Movie%2CAudio&type=Audio&enableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(repeated_boundary["TotalRecordCount"], 1);
    assert_eq!(
        item_names(&repeated_boundary),
        BTreeSet::from([format!("Audio {}", fixture.suffix)])
    );

    let sdk_repeated = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&mediaType=Audio&type=Movie&type=Audio&enableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(sdk_repeated["TotalRecordCount"], 3);

    for route in ["/Items/Suggestions", &legacy_route] {
        for query in [
            "MediaType=Video&Type=Movie&StartIndex=-2&Limit=1&EnableTotalRecordCount=true",
            "mediaType=Video&type=Movie&startIndex=-2&limit=1&enableTotalRecordCount=true",
            "mediatype=Video&type=Movie&startindex=-2&limit=1&enabletotalrecordcount=true",
        ] {
            let page = body_json(
                fixture
                    .get(&format!("{route}?{query}"), Some(&fixture.admin_token))
                    .await,
            )
            .await;
            assert_eq!(page["StartIndex"], -2);
            assert_eq!(page["TotalRecordCount"], 2);
            assert_eq!(page["Items"].as_array().unwrap().len(), 1);
        }
    }

    let zero_limit = body_json(
        fixture
            .get(
                "/Items/Suggestions?mediaType=Video&type=Movie&limit=0&enableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(zero_limit["Items"].as_array().unwrap().len(), 0);
    assert_eq!(zero_limit["TotalRecordCount"], 2);
    assert_eq!(zero_limit["StartIndex"], 0);

    let negative_limit = body_json(
        fixture
            .get(
                &format!(
                    "/Users/{}/Suggestions?MediaType=Video&Type=Movie&Limit=-1",
                    fixture.user_id
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert_eq!(negative_limit["Items"].as_array().unwrap().len(), 2);
    assert_eq!(negative_limit["TotalRecordCount"], 2);
    assert_eq!(negative_limit["StartIndex"], 0);

    for route in [
        "/Items/Suggestions?startIndex=2147483648",
        "/Items/Suggestions?startIndex=-2147483649",
        "/Items/Suggestions?limit=2147483648",
        "/Items/Suggestions?limit=-2147483649",
        &format!(
            "/Users/{}/Suggestions?StartIndex=2147483648",
            fixture.user_id
        ),
        &format!("/Users/{}/Suggestions?Limit=-2147483649", fixture.user_id),
    ] {
        assert_eq!(
            fixture
                .get(route, Some(&fixture.admin_token))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "route {route} must reject values outside Int32"
        );
    }

    assert_eq!(
        fixture
            .get(
                "/Items/Suggestions?enableTotalRecordCount=not-bool",
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    fixture.cleanup().await;
}

#[tokio::test]
async fn suggestions_optional_user_matches_global_lookup_and_authorization_semantics() {
    let fixture = Fixture::new().await;
    let items = BaseItemRepository::new(fixture.database.clone());
    let root = items.ensure_user_root().await.expect("user root");
    let global_only_name = format!("Global Movie {}", fixture.suffix);
    create_item(
        &items,
        "Movie",
        &global_only_name,
        None,
        Some("Video"),
        false,
    )
    .await;

    let grouped_name = format!("Grouped Movie {}", fixture.suffix);
    let presentation_key = format!("suggestions-group-{}", fixture.suffix);
    for _ in 0..2 {
        let mut item = NewBaseItem::new(Uuid::new_v4(), "Movie");
        item.name = Some(grouped_name.clone());
        item.sort_name = Some(grouped_name.clone());
        item.parent_id = Some(root.id);
        item.media_type = Some("Video".to_owned());
        item.presentation_unique_key = Some(presentation_key.clone());
        items.create(item).await.expect("grouped suggestion");
    }

    let omitted = body_json(
        fixture
            .get(
                "/Items/Suggestions?MediaType=Video&Type=Movie&EnableTotalRecordCount=true",
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(item_names(&omitted).contains(&global_only_name));
    assert_eq!(named_item_count(&omitted, &grouped_name), 2);
    assert_no_user_data(&omitted);

    let empty = body_json(
        fixture
            .get(
                &format!(
                    "/items/suggestions?userid={}&mediatype=Video&type=Movie&enabletotalrecordcount=true",
                    Uuid::nil()
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert_eq!(item_names(&empty), item_names(&omitted));
    assert_eq!(named_item_count(&empty, &grouped_name), 2);
    assert_no_user_data(&empty);

    let explicit_user = body_json(
        fixture
            .get(
                &format!(
                    "/Items/Suggestions?userId={}&mediaType=Video&type=Movie&enableTotalRecordCount=true",
                    fixture.user_id
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(!item_names(&explicit_user).contains(&global_only_name));
    assert_eq!(named_item_count(&explicit_user, &grouped_name), 1);
    assert!(
        explicit_user["Items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["UserData"].is_object())
    );

    let unknown_user_id = Uuid::new_v4();
    assert_eq!(
        fixture
            .get(
                &format!("/Items/Suggestions?userid={unknown_user_id}&mediaType=Video&type=Movie"),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture
            .get(
                &format!("/Users/{unknown_user_id}/Suggestions?MediaType=Video&Type=Movie"),
                Some(&fixture.user_token),
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    let admin_unknown = body_json(
        fixture
            .get(
                &format!(
                    "/Items/Suggestions?UserId={unknown_user_id}&MediaType=Video&Type=Movie&EnableTotalRecordCount=true"
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert!(item_names(&admin_unknown).contains(&global_only_name));
    assert_eq!(named_item_count(&admin_unknown, &grouped_name), 2);
    assert_no_user_data(&admin_unknown);

    let legacy_empty = body_json(
        fixture
            .get(
                &format!(
                    "/users/{}/suggestions?mediatype=Video&type=Movie&enabletotalrecordcount=true",
                    Uuid::nil()
                ),
                Some(&fixture.user_token),
            )
            .await,
    )
    .await;
    assert!(item_names(&legacy_empty).contains(&global_only_name));
    assert_eq!(named_item_count(&legacy_empty, &grouped_name), 2);
    assert_no_user_data(&legacy_empty);

    let legacy_admin_unknown = body_json(
        fixture
            .get(
                &format!(
                    "/users/{unknown_user_id}/suggestions?mediatype=Video&type=Movie&enabletotalrecordcount=true"
                ),
                Some(&fixture.admin_token),
            )
            .await,
    )
    .await;
    assert!(item_names(&legacy_admin_unknown).contains(&global_only_name));
    assert_eq!(named_item_count(&legacy_admin_unknown, &grouped_name), 2);
    assert_no_user_data(&legacy_admin_unknown);

    fixture.cleanup().await;
}

fn item_names(response: &Value) -> BTreeSet<String> {
    response["Items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| item["Name"].as_str().expect("name").to_owned())
        .collect()
}

fn named_item_count(response: &Value, name: &str) -> usize {
    response["Items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["Name"] == name)
        .count()
}

fn assert_no_user_data(response: &Value) {
    assert!(
        response["Items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item.get("UserData").is_none())
    );
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

struct Fixture {
    database_name: String,
    database: DatabaseConnection,
    app: Router,
    suffix: String,
    admin_id: Uuid,
    admin_token: String,
    user_id: Uuid,
    user_token: String,
}

impl Fixture {
    async fn new() -> Self {
        let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
        assert_temporary_database_name(&database_name);
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
            .await
            .expect("temporary PostgreSQL database creation must succeed");
        administrator.close().await.unwrap();

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
            .create_initial_administrator(&format!("suggestions-admin-{suffix}"))
            .await
            .unwrap();
        let user = users
            .create(&format!("suggestions-user-{suffix}"))
            .await
            .unwrap();
        let devices = DeviceRepository::new(database.clone());
        let admin_token = session(&devices, admin.id, &format!("admin-{suffix}")).await;
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let root_movie = create_item(
            &items,
            "Movie",
            &format!("Root Movie {suffix}"),
            Some(root.id),
            Some("Video"),
            false,
        )
        .await;
        let folder = create_item(
            &items,
            "Folder",
            &format!("Folder {suffix}"),
            Some(root.id),
            None,
            false,
        )
        .await;
        create_item(
            &items,
            "Movie",
            &format!("Nested Movie {suffix}"),
            Some(folder.id),
            Some("Video"),
            false,
        )
        .await;
        create_item(
            &items,
            "Episode",
            &format!("Episode {suffix}"),
            Some(root.id),
            Some("Video"),
            false,
        )
        .await;
        create_item(
            &items,
            "Audio",
            &format!("Audio {suffix}"),
            Some(root.id),
            Some("Audio"),
            false,
        )
        .await;
        create_item(
            &items,
            "Movie",
            &format!("Virtual Movie {suffix}"),
            Some(root.id),
            Some("Video"),
            true,
        )
        .await;
        assert_ne!(root_movie.id, folder.id);

        let app = jellyfin_api::router(AppState::new(
            database.clone(),
            "Suggestions Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database_name,
            database,
            app,
            suffix,
            admin_id: admin.id,
            admin_token,
            user_id: user.id,
            user_token,
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
        let Self {
            database_name,
            database,
            app,
            ..
        } = self;
        drop(app);
        database.close().await.unwrap();
        let administrator = jellyfin_data::connect(&DatabaseConfig::default())
            .await
            .expect("local PostgreSQL must be available");
        administrator
            .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
            .await
            .expect("temporary PostgreSQL database cleanup must succeed");
        administrator.close().await.unwrap();
    }
}

async fn create_item(
    repository: &BaseItemRepository,
    item_type: &str,
    name: &str,
    parent_id: Option<Uuid>,
    media_type: Option<&str>,
    is_virtual_item: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = Some(name.to_owned());
    item.parent_id = parent_id;
    item.media_type = media_type.map(ToOwned::to_owned);
    item.is_folder = item_type == "Folder";
    item.is_virtual_item = is_virtual_item;
    repository.create(item).await.expect("item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Suggestions Tests",
            "1.0",
            "Test",
            format!("suggestions-tests-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    let suffix = name
        .strip_prefix(DATABASE_PREFIX)
        .expect("temporary database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
}
