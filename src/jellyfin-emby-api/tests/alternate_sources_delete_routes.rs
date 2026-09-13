use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository, LinkedChildRepository,
    LinkedChildType, NewBaseItem, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Alternate Sources Tests\", DeviceId=\"emby-alternate-sources-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_alternate_sources_";

#[tokio::test]
async fn post_delete_reuses_real_alternate_source_mutation_with_emby_contract() {
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
    administrator
        .close()
        .await
        .expect("administrator database pool cleanup");
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
        max_connections: 16,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let fixture = Fixture::new(database.clone()).await;
    assert_authorization_and_typed_not_found(&fixture).await;
    assert_emby_mutations(&fixture).await;
    assert_jellyfin_isolation(&fixture, database.clone()).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct VersionGroup {
    primary: Uuid,
    ids: [Uuid; 2],
}

struct Fixture {
    database: DatabaseConnection,
    repository: BaseItemRepository,
    app: Router,
    administrator_token: String,
    user_token: String,
    api_key: String,
    non_video_id: Uuid,
    canonical_group: VersionGroup,
    lowercase_group: VersionGroup,
    mixed_group: VersionGroup,
    jellyfin_group: VersionGroup,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let administrator = users
            .create_initial_administrator(&format!("alternate-admin-{suffix}"))
            .await
            .expect("administrator creation");
        let user = users
            .create(&format!("alternate-user-{suffix}"))
            .await
            .expect("user creation");
        let devices = DeviceRepository::new(database.clone());
        let administrator_token = session(&devices, administrator.id, "admin").await;
        let user_token = session(&devices, user.id, "user").await;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("alternate-api-key-{suffix}"))
            .await
            .expect("API key creation")
            .access_token;

        let repository = BaseItemRepository::new(database.clone());
        let canonical_group =
            create_linked_group(&repository, &format!("{suffix}-canonical")).await;
        let lowercase_group =
            create_linked_group(&repository, &format!("{suffix}-lowercase")).await;
        let mixed_group = create_linked_group(&repository, &format!("{suffix}-mixed")).await;
        let jellyfin_group = create_linked_group(&repository, &format!("{suffix}-jellyfin")).await;
        let non_video_id = Uuid::new_v4();
        let mut folder = NewBaseItem::new(non_video_id, "Folder");
        folder.name = Some("Not a video".to_owned());
        folder.is_folder = true;
        repository.create(folder).await.expect("folder creation");

        let app = jellyfin_emby_api::router(AppState::new(
            database.clone(),
            "Emby Alternate Sources Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        ));

        Self {
            database,
            repository,
            app,
            administrator_token,
            user_token,
            api_key,
            non_video_id,
            canonical_group,
            lowercase_group,
            mixed_group,
            jellyfin_group,
        }
    }
}

async fn assert_authorization_and_typed_not_found(fixture: &Fixture) {
    let route = emby_route(fixture.canonical_group.primary);
    assert_eq!(
        request(&fixture.app, Method::POST, &route, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &route,
            Some(&fixture.user_token)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &emby_route(Uuid::new_v4()),
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &emby_route(fixture.non_video_id),
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

async fn assert_emby_mutations(fixture: &Fixture) {
    assert_group_is_linked(fixture, &fixture.canonical_group).await;
    let before = load_group(fixture, &fixture.canonical_group).await;
    let response = request(
        &fixture.app,
        Method::POST,
        &emby_route(fixture.canonical_group.ids[1]),
        Some(&fixture.administrator_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("empty response body")
            .is_empty()
    );
    assert_group_was_detached_without_data_loss(fixture, &fixture.canonical_group, &before).await;

    assert_group_is_linked(fixture, &fixture.lowercase_group).await;
    let before = load_group(fixture, &fixture.lowercase_group).await;
    let lowercase = format!(
        "/emby/videos/{}/alternatesources/delete?api_key={}",
        fixture.lowercase_group.primary, fixture.api_key
    );
    assert_eq!(
        request(&fixture.app, Method::POST, &lowercase, None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_group_was_detached_without_data_loss(fixture, &fixture.lowercase_group, &before).await;

    assert_group_is_linked(fixture, &fixture.mixed_group).await;
    let before = load_group(fixture, &fixture.mixed_group).await;
    let mixed = format!(
        "/emby/vIdEoS/{}/aLtErNaTeSoUrCeS/dElEtE",
        fixture.mixed_group.primary
    );
    assert_eq!(
        request(
            &fixture.app,
            Method::POST,
            &mixed,
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_group_was_detached_without_data_loss(fixture, &fixture.mixed_group, &before).await;
}

async fn assert_jellyfin_isolation(fixture: &Fixture, database: DatabaseConnection) {
    let app = jellyfin_api::router(AppState::new(
        database,
        "Jellyfin Alternate Sources Isolation".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let emby_only = format!(
        "/Videos/{}/AlternateSources/Delete",
        fixture.jellyfin_group.primary
    );
    assert_eq!(
        request(
            &app,
            Method::POST,
            &emby_only,
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "the Emby POST alias must not appear on Jellyfin's root router"
    );

    let jellyfin_delete = format!(
        "/Videos/{}/AlternateSources",
        fixture.jellyfin_group.primary
    );
    assert_eq!(
        request(
            &app,
            Method::DELETE,
            &jellyfin_delete,
            Some(&fixture.administrator_token),
        )
        .await
        .status(),
        StatusCode::NO_CONTENT,
        "the existing Jellyfin method and success status must remain unchanged"
    );
    for item in load_group(fixture, &fixture.jellyfin_group).await {
        assert_eq!(item.primary_version_id, None);
    }
}

async fn create_linked_group(repository: &BaseItemRepository, label: &str) -> VersionGroup {
    let ids = [Uuid::new_v4(), Uuid::new_v4()];
    for (index, id) in ids.into_iter().enumerate() {
        let mut item = NewBaseItem::new(id, if index == 0 { "Movie" } else { "Video" });
        item.name = Some(format!("{label}-{index}"));
        item.path = Some(format!("/media/{label}-{index}.mkv"));
        item.data = Some(json!({"Fixture": label, "Index": index}));
        repository.create(item).await.expect("video creation");
    }
    let primary = repository
        .merge_linked_alternate_versions(&ids)
        .await
        .expect("manual alternate merge");
    VersionGroup { primary, ids }
}

async fn assert_group_is_linked(fixture: &Fixture, group: &VersionGroup) {
    let items = load_group(fixture, group).await;
    assert!(
        items
            .iter()
            .any(|item| item.id != group.primary && item.primary_version_id == Some(group.primary))
    );
    assert!(
        LinkedChildRepository::new(fixture.database.clone())
            .list(group.primary)
            .await
            .expect("alternate links")
            .iter()
            .any(|link| link.child_type == LinkedChildType::LinkedAlternateVersion)
    );
}

async fn assert_group_was_detached_without_data_loss(
    fixture: &Fixture,
    group: &VersionGroup,
    before: &[jellyfin_data::entities::base_item::Model],
) {
    let after = load_group(fixture, group).await;
    assert_eq!(after.len(), before.len());
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(after.id, before.id);
        assert_eq!(after.primary_version_id, None);
        assert_eq!(after.item_type, before.item_type);
        assert_eq!(after.path, before.path);
        assert_eq!(after.data, before.data);
    }
    assert!(
        LinkedChildRepository::new(fixture.database.clone())
            .list(group.primary)
            .await
            .expect("remaining links")
            .iter()
            .all(|link| link.child_type != LinkedChildType::LinkedAlternateVersion)
    );
}

async fn load_group(
    fixture: &Fixture,
    group: &VersionGroup,
) -> Vec<jellyfin_data::entities::base_item::Model> {
    let mut items = Vec::new();
    for id in group.ids {
        items.push(
            fixture
                .repository
                .get(id)
                .await
                .expect("video lookup")
                .expect("video remains persisted"),
        );
    }
    items
}

fn emby_route(item_id: Uuid) -> String {
    format!("/emby/Videos/{item_id}/AlternateSources/Delete")
}

async fn request(
    app: &Router,
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
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("route response")
}

async fn session(devices: &DeviceRepository, user_id: Uuid, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "Emby Alternate Sources Tests",
            "1.0",
            "Test",
            format!("emby-alternate-sources-{suffix}"),
        ))
        .await
        .expect("session creation")
        .access_token
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
