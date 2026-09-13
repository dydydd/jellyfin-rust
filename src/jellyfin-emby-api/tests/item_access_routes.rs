use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ApiKeyRepository, BaseItemRepository, DatabaseConfig, DeviceRepository,
    EmbyItemAccessRepository, NewBaseItem, NewDevice,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Item Access Tests\", DeviceId=\"emby-item-access-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_item_access_";

#[tokio::test]
async fn item_access_is_authenticated_atomic_postgres_backed_and_protocol_local() {
    let administrator = jellyfin_data::connect(&DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!("{DATABASE_PREFIX}{}", Uuid::new_v4().simple());
    assert_temporary_database_name(&database_name);
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary database creation");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_routes(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary database cleanup");
    administrator.close().await.expect("administrator cleanup");
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
    .expect("temporary PostgreSQL database");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations");

    let fixture = Fixture::new(database.clone()).await;
    assert_authentication_precedes_body_validation(&fixture).await;
    assert_binding_and_cartesian_update(&fixture).await;
    assert_missing_targets_roll_back(&fixture).await;
    assert_none_delete_and_restart_persistence(&fixture).await;
    assert_shared_leave_authorization_binding_and_idempotency(&fixture).await;
    assert_protocol_isolation(&fixture).await;

    drop(fixture);
    database.close().await.expect("database pool cleanup");
}

struct Fixture {
    database: DatabaseConnection,
    emby: Router,
    jellyfin: Router,
    first_user: Uuid,
    second_user: Uuid,
    first_item: Uuid,
    second_item: Uuid,
    user_token: String,
    api_key: String,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Self {
        let suffix = Uuid::new_v4().simple().to_string();
        let users = UserService::new(database.clone());
        let first_user = users
            .create_initial_administrator(&format!("item-access-admin-{suffix}"))
            .await
            .expect("first user");
        let second_user = users
            .create(&format!("item-access-user-{suffix}"))
            .await
            .expect("second user");
        let devices = DeviceRepository::new(database.clone());
        let user_token = devices
            .create_session(NewDevice::new(
                second_user.id,
                "Emby Item Access Tests",
                "1.0",
                "Test",
                "emby-item-access-user",
            ))
            .await
            .expect("ordinary user session")
            .access_token;
        let api_key = ApiKeyRepository::new(database.clone())
            .create(&format!("item-access-key-{suffix}"))
            .await
            .expect("API key")
            .access_token;

        let items = BaseItemRepository::new(database.clone());
        let first_item = Uuid::new_v4();
        let second_item = Uuid::new_v4();
        items
            .create(NewBaseItem::new(first_item, "Movie"))
            .await
            .expect("first item");
        items
            .create(NewBaseItem::new(second_item, "Episode"))
            .await
            .expect("second item");

        let state = AppState::new(
            database.clone(),
            "Emby Item Access Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        );
        let emby = jellyfin_emby_api::router(state.clone());
        let jellyfin = jellyfin_api::router(state);
        Self {
            database,
            emby,
            jellyfin,
            first_user: first_user.id,
            second_user: second_user.id,
            first_item,
            second_item,
            user_token,
            api_key,
        }
    }

    fn repository(&self) -> EmbyItemAccessRepository {
        EmbyItemAccessRepository::new(self.database.clone())
    }
}

async fn assert_authentication_precedes_body_validation(fixture: &Fixture) {
    assert_eq!(
        request(&fixture.emby, "/emby/Items/Access", Some("{"), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
    for body in [
        "{",
        "[]",
        "true",
        r#"{"ItemAccess":0}"#,
        r#"{"ItemAccess":6}"#,
        r#"{"ItemIds":"bad"}"#,
    ] {
        assert_eq!(
            request(
                &fixture.emby,
                "/emby/Items/Access",
                Some(body),
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "invalid body must be rejected: {body}",
        );
    }
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/Items/Access",
            Some("{}"),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
}

async fn assert_binding_and_cartesian_update(fixture: &Fixture) {
    let body = format!(
        r#"{{"ItemIds":["{}"],"iTeMiDs":["{}","{}","{}"],"UserIds":["{}"],"USERIDS":["{}","{}","{}"],"ItemAccess":1,"itemACCESS":"manageDelete","Unknown":{{"ignored":true}}}}"#,
        Uuid::new_v4(),
        fixture.first_item,
        fixture.second_item,
        fixture.first_item,
        Uuid::new_v4(),
        fixture.first_user,
        fixture.second_user,
        fixture.first_user,
    );
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/iTeMs/aCcEsS",
            Some(&body),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;

    for user_id in [fixture.first_user, fixture.second_user] {
        for item_id in [fixture.first_item, fixture.second_item] {
            assert_eq!(stored(fixture, user_id, item_id).await, Some(4));
        }
    }

    let numeric = body_for(&[fixture.second_item], &[fixture.first_user], "3");
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/items/access",
            Some(&numeric),
            Some(&fixture.api_key),
        )
        .await,
    )
    .await;
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.second_item).await,
        Some(2)
    );

    let numeric_string = body_for(&[fixture.second_item], &[fixture.first_user], r#""2""#);
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/items/access",
            Some(&numeric_string),
            Some(&fixture.api_key),
        )
        .await,
    )
    .await;
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.second_item).await,
        Some(1)
    );
}

async fn assert_missing_targets_roll_back(fixture: &Fixture) {
    for (users, items) in [
        (
            vec![fixture.first_user, Uuid::new_v4()],
            vec![fixture.first_item],
        ),
        (
            vec![fixture.first_user],
            vec![fixture.first_item, Uuid::new_v4()],
        ),
    ] {
        let body = body_for(&items, &users, r#""Read""#);
        assert_eq!(
            request(
                &fixture.emby,
                "/emby/Items/Access",
                Some(&body),
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND,
        );
        assert_eq!(
            stored(fixture, fixture.first_user, fixture.first_item).await,
            Some(4)
        );
    }

    let invalid_uuid = format!(
        r#"{{"ItemIds":["not-a-uuid"],"UserIds":["{}"],"ItemAccess":"Read"}}"#,
        fixture.first_user
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Access",
            Some(&invalid_uuid),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
}

async fn assert_none_delete_and_restart_persistence(fixture: &Fixture) {
    let delete = body_for(&[fixture.first_item], &[fixture.second_user], r#""None""#);
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/Items/Access",
            Some(&delete),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        stored(fixture, fixture.second_user, fixture.first_item).await,
        None
    );

    let restarted = jellyfin_emby_api::router(AppState::new(
        fixture.database.clone(),
        "Restarted Emby Item Access Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.first_item).await,
        Some(4)
    );
    let update = body_for(&[fixture.first_item], &[fixture.first_user], r#""Manage""#);
    assert_ok_empty(
        request(
            &restarted,
            "/emby/Items/Access",
            Some(&update),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.first_item).await,
        Some(3)
    );
}

async fn assert_shared_leave_authorization_binding_and_idempotency(fixture: &Fixture) {
    for body in ["{", "[]", "null", r#"{"ItemIds":"bad"}"#] {
        assert_eq!(
            request(&fixture.emby, "/emby/Items/Shared/Leave", Some(body), None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED,
            "authentication must precede invalid body handling: {body}",
        );
        assert_eq!(
            request(
                &fixture.emby,
                "/emby/Items/Shared/Leave",
                Some(body),
                Some(&fixture.user_token),
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "authenticated invalid body must be rejected: {body}",
        );
    }

    // A device session cannot remove another user's share. Authorize the
    // target before binding item ids so malformed ids do not weaken the
    // self-only boundary or expose target state.
    let foreign_target = format!(
        r#"{{"UserId":"{}","ItemIds":["not-a-uuid"]}}"#,
        fixture.first_user
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(&foreign_target),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );

    let missing_target = format!(
        r#"{{"UserId":"{}","ItemIds":["not-a-uuid"]}}"#,
        Uuid::new_v4()
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(&missing_target),
            Some(&fixture.api_key),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );

    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(r#"{"UserId":"not-a-uuid","ItemIds":[]}"#),
            Some(&fixture.api_key),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(r#"{"ItemIds":["not-a-uuid"]}"#),
            Some(&fixture.api_key),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "an API key has no implicit current user",
    );

    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(r#"{"ItemIds":["not-a-uuid"]}"#),
            Some(&fixture.user_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );

    // A missing item aborts the entire leave operation without deleting a
    // valid assignment in the same request.
    let missing_item = format!(
        r#"{{"UserId":"{}","ItemIds":["{}","{}"]}}"#,
        fixture.first_user,
        fixture.first_item,
        Uuid::new_v4()
    );
    assert_eq!(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(&missing_item),
            Some(&fixture.api_key),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.first_item).await,
        Some(3),
    );

    // An omitted UserId targets the authenticated device user. ASP.NET JSON
    // binding is case-insensitive and the last duplicate property wins.
    let self_leave = format!(
        r#"{{"ItemIds":["{}"],"iTeMiDs":["{}","{}"],"Unknown":true}}"#,
        fixture.first_item, fixture.second_item, fixture.second_item
    );
    for path in ["/emby/iTeMs/sHaReD/lEaVe", "/emby/items/shared/leave"] {
        assert_ok_empty(
            request(
                &fixture.emby,
                path,
                Some(&self_leave),
                Some(&fixture.user_token),
            )
            .await,
        )
        .await;
    }
    assert_eq!(
        stored(fixture, fixture.second_user, fixture.second_item).await,
        None,
        "leaving a share repeatedly must be idempotent",
    );
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.second_item).await,
        Some(1),
        "leaving must affect only the resolved target user",
    );

    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some("{}"),
            Some(&fixture.user_token),
        )
        .await,
    )
    .await;

    let api_key_leave = format!(
        r#"{{"UserId":"{}","USERID":"{}","ItemIds":["{}"]}}"#,
        Uuid::new_v4(),
        fixture.first_user,
        fixture.first_item,
    );
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(&api_key_leave),
            Some(&fixture.api_key),
        )
        .await,
    )
    .await;
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.first_item).await,
        None,
    );

    let empty = format!(r#"{{"UserId":"{}","ItemIds":[]}}"#, fixture.first_user);
    assert_ok_empty(
        request(
            &fixture.emby,
            "/emby/Items/Shared/Leave",
            Some(&empty),
            Some(&fixture.api_key),
        )
        .await,
    )
    .await;
}

async fn assert_protocol_isolation(fixture: &Fixture) {
    let body = body_for(&[fixture.first_item], &[fixture.first_user], r#""Read""#);
    for path in ["/Items/Access", "/api/Items/Access"] {
        let response = request(
            &fixture.jellyfin,
            path,
            Some(&body),
            Some(&fixture.user_token),
        )
        .await;
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "Emby route leaked at {path}"
        );
    }

    let leave = format!(
        r#"{{"UserId":"{}","ItemIds":["{}"]}}"#,
        fixture.first_user, fixture.second_item
    );
    for path in ["/Items/Shared/Leave", "/api/Items/Shared/Leave"] {
        let response = request(
            &fixture.jellyfin,
            path,
            Some(&leave),
            Some(&fixture.api_key),
        )
        .await;
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "Emby shared-leave route leaked at {path}",
        );
    }
    assert_eq!(
        stored(fixture, fixture.first_user, fixture.second_item).await,
        Some(1),
        "Jellyfin root requests must not mutate Emby's private access table",
    );
}

async fn stored(fixture: &Fixture, user_id: Uuid, item_id: Uuid) -> Option<i16> {
    fixture
        .repository()
        .get(user_id, item_id)
        .await
        .expect("item-access lookup")
        .map(|row| row.access_level)
}

fn body_for(item_ids: &[Uuid], user_ids: &[Uuid], access_json: &str) -> String {
    let item_ids = item_ids
        .iter()
        .map(|id| format!(r#""{id}""#))
        .collect::<Vec<_>>()
        .join(",");
    let user_ids = user_ids
        .iter()
        .map(|id| format!(r#""{id}""#))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"ItemIds":[{item_ids}],"UserIds":[{user_ids}],"ItemAccess":{access_json}}}"#)
}

async fn request(
    app: &Router,
    uri: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(Method::POST).uri(uri);
    if let Some(token) = token {
        request = request.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(body.unwrap_or_default().to_owned()))
                .expect("request"),
        )
        .await
        .expect("route response")
}

async fn assert_ok_empty(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .expect("response body")
            .is_empty()
    );
}

fn assert_temporary_database_name(name: &str) {
    assert!(name.starts_with(DATABASE_PREFIX));
    assert!(
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
}
