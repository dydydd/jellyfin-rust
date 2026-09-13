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
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Missing Shows Tests\", DeviceId=\"emby-missing-shows\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_missing_shows_";

#[tokio::test]
async fn missing_shows_reuse_authorized_items_query_without_leaking_to_jellyfin() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert!(database_name.starts_with(DATABASE_PREFIX));
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move { exercise(&task_database_name).await }).await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup");
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
        .expect("temporary database migrations");

    let fixture = Fixture::new(database.clone()).await;
    fixture.assert_auth_policy_and_forced_filters().await;
    fixture.assert_signed_paging_and_binding().await;
    fixture.assert_protocol_isolation().await;

    database.close().await.expect("database close");
}

struct Fixture {
    emby: axum::Router,
    jellyfin: axum::Router,
    user_id: Uuid,
    other_user_id: Uuid,
    visible_missing_ids: Vec<Uuid>,
    user_token: String,
    admin_token: String,
    api_key_token: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("missing-shows-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("missing-shows-user-{suffix}"))
            .await
            .expect("user creation");
        let other_user = users
            .create(&format!("missing-shows-other-{suffix}"))
            .await
            .expect("other user creation");

        let items = BaseItemRepository::new(database.clone());
        let root = items.ensure_user_root().await.expect("user root");
        let allowed =
            create_item(&items, "CollectionFolder", "Allowed", root.id, true, false).await;
        let blocked =
            create_item(&items, "CollectionFolder", "Blocked", root.id, true, false).await;
        let first_missing =
            create_item(&items, "Episode", "Missing Alpha", allowed.id, false, true).await;
        let second_missing =
            create_item(&items, "Episode", "Missing Bravo", allowed.id, false, true).await;
        let _present = create_item(
            &items,
            "Episode",
            "Present Episode",
            allowed.id,
            false,
            false,
        )
        .await;
        let _missing_movie =
            create_item(&items, "Movie", "Missing Movie", allowed.id, false, true).await;
        let blocked_missing = create_item(
            &items,
            "Episode",
            "Blocked Missing",
            blocked.id,
            false,
            true,
        )
        .await;
        ItemValueRepository::new(database.clone())
            .link(
                blocked_missing.id,
                item_value::ItemValueType::Tags,
                "Blocked Missing Tag",
            )
            .await
            .expect("blocked tag relation");

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
                    blocked_tags: vec!["Blocked Missing Tag".to_owned()],
                    ..UserPolicy::default()
                },
            )
            .await
            .expect("user policy");

        let devices = DeviceRepository::new(database.clone());
        let user_token = session(&devices, user.id, &format!("user-{suffix}")).await;
        let admin_token = session(
            &devices,
            administrator.id,
            &format!("administrator-{suffix}"),
        )
        .await;
        let api_key_token = ApiKeyRepository::new(database.clone())
            .create(&format!("missing-shows-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;
        let state = AppState::new(
            database,
            "Emby Missing Shows Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        Self {
            emby: jellyfin_emby_api::router(state.clone()),
            jellyfin: jellyfin_api::router(state),
            user_id: user.id,
            other_user_id: other_user.id,
            visible_missing_ids: vec![first_missing.id, second_missing.id],
            user_token,
            admin_token,
            api_key_token,
        }
    }

    async fn assert_auth_policy_and_forced_filters(&self) {
        let unscoped = response_json(
            request(&self.emby, "/emby/Shows/Missing", Some(&self.api_key_token)).await,
        )
        .await;
        assert_eq!(unscoped["TotalRecordCount"], 3, "{unscoped}");

        assert_eq!(
            request(&self.emby, "/emby/Shows/Missing", None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &self.emby,
                &format!("/emby/Shows/Missing?UserId={}", self.other_user_id),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );

        let route = format!(
            "/emby/sHoWs/mIsSiNg?userid={}&recursive=true&includeitemtypes=Movie&ISMISSING=false&%49ncludeItemTypes=Series&Fields=ProviderIds",
            self.user_id
        );
        let page = response_json(request(&self.emby, &route, Some(&self.user_token)).await).await;
        assert_eq!(page["TotalRecordCount"], 2, "{page}");
        let returned = page["Items"]
            .as_array()
            .expect("missing episode page")
            .iter()
            .map(|item| {
                assert_eq!(item["Type"], "Episode");
                Uuid::parse_str(item["Id"].as_str().expect("item id")).expect("UUID item id")
            })
            .collect::<Vec<_>>();
        assert!(
            self.visible_missing_ids
                .iter()
                .all(|id| returned.contains(id))
        );

        let administrator_page = response_json(
            request(
                &self.emby,
                &format!(
                    "/emby/Shows/Missing?UserId={}&Recursive=true",
                    self.other_user_id
                ),
                Some(&self.admin_token),
            )
            .await,
        )
        .await;
        assert_eq!(administrator_page["TotalRecordCount"], 3);

        let api_key_page = response_json(
            request(&self.emby, "/emby/Shows/Missing", Some(&self.api_key_token)).await,
        )
        .await;
        assert_eq!(api_key_page["TotalRecordCount"], 3);
    }

    async fn assert_signed_paging_and_binding(&self) {
        for (query, expected_items) in [
            ("startindex=-1&limit=1", 1),
            ("StartIndex=0&Limit=0", 0),
            ("StartIndex=0&Limit=-1", 2),
        ] {
            let page = response_json(
                request(
                    &self.emby,
                    &format!(
                        "/emby/shows/missing?USERID={}&Recursive=true&{query}",
                        self.user_id
                    ),
                    Some(&self.user_token),
                )
                .await,
            )
            .await;
            assert_eq!(page["TotalRecordCount"], 2, "{query}");
            assert_eq!(
                page["Items"].as_array().expect("paged items").len(),
                expected_items,
                "{query}"
            );
        }

        assert_eq!(
            request(
                &self.emby,
                &format!(
                    "/emby/Shows/Missing?UserId={}&Recursive=true&Limit=2147483648",
                    self.user_id
                ),
                Some(&self.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_protocol_isolation(&self) {
        for path in ["/Shows/Missing", "/api/Shows/Missing"] {
            assert_eq!(
                request(&self.jellyfin, path, Some(&self.admin_token))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Emby-only route leaked at {path}"
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
    is_virtual_item: bool,
) -> jellyfin_data::entities::base_item::Model {
    let mut item = NewBaseItem::new(Uuid::new_v4(), item_type);
    item.name = Some(name.to_owned());
    item.sort_name = item.name.clone();
    item.parent_id = Some(parent_id);
    item.is_folder = is_folder;
    item.is_virtual_item = is_virtual_item;
    repository.create(item).await.expect("base item creation")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Missing Shows Tests",
            "1.0",
            "Test",
            format!("emby-missing-shows-{suffix}"),
        ))
        .await
        .expect("device session")
        .access_token
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
        .expect("route response")
}

async fn response_json(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}
