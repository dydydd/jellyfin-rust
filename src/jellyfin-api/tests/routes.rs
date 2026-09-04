use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    ActivityLogRepository, DeviceQuery, DeviceRepository, NewActivityLog, NewDevice,
    entities::{
        activity_log::{self, LogSeverity},
        user,
    },
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    prelude::DateTimeUtc, sea_query::Expr,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_RESPONSE_SIZE: usize = 1024 * 1024;
const USER_MANAGEMENT_DATABASE_PREFIX: &str = "jellyfin_user_management_routes_";
const CLIENT_AUTHORIZATION: &str = "MediaBrowser Client=\"Jellyfin.Server%20Integration%20Tests\", DeviceId=\"69420\", Device=\"Apple%20II\", Version=\"10.8.0\"";
static ADMIN_API_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn system_routes_follow_the_public_contract() {
    let database = test_database().await;
    let app = jellyfin_api::router(AppState::new(
        database,
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let response = app
        .clone()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "Healthy");

    let response = app
        .clone()
        .oneshot(
            Request::get("/System/Info/Public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = body_json(response).await;
    assert_eq!(body["ServerName"], "Test Server");
    assert_eq!(body["LocalAddress"], "http://127.0.0.1:8096");
    assert_eq!(body["ProductName"], "Jellyfin Server");
    assert_eq!(body["StartupWizardCompleted"], false);
    assert_eq!(body["Id"].as_str().unwrap().len(), 32);
    assert!(body.get("server_name").is_none());

    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/System/Ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_text(response).await, "Test Server");
    }

    let before_time_sync = Utc::now();
    let response = app
        .clone()
        .oneshot(Request::get("/GetUtcTime").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let after_time_sync = Utc::now();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = body_json(response).await;
    assert!(body.get("request_reception_time").is_none());
    let request_reception_time =
        chrono::DateTime::parse_from_rfc3339(body["RequestReceptionTime"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
    let response_transmission_time =
        chrono::DateTime::parse_from_rfc3339(body["ResponseTransmissionTime"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
    assert!(request_reception_time >= before_time_sync);
    assert!(response_transmission_time >= request_reception_time);
    assert!(response_transmission_time <= after_time_sync);
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the assertions cover the stateful official query matrix"
)]
async fn activity_log_routes_match_the_official_controller_contract() {
    let _test_guard = ADMIN_API_TEST_LOCK.lock().await;
    let database = test_database().await;
    user::Entity::delete_many()
        .filter(user::Column::Username.like("api-admin-activity-%"))
        .exec(&database)
        .await
        .expect("stale activity test administrators must be removed");
    let suffix = Uuid::new_v4().simple().to_string();
    let marker = format!("ActivityApi{suffix}");
    let users = UserService::new(database.clone());
    let devices = DeviceRepository::new(database.clone());
    let administrator = users
        .create_initial_administrator(&format!("api-admin-activity-{suffix}"))
        .await
        .expect("activity test administrator must be created");
    let regular_name = format!("activity-api-user-{suffix}");
    let regular_user = users
        .create(&regular_name)
        .await
        .expect("activity test user must be created");
    let administrator_session = devices
        .create_session(NewDevice::new(
            administrator.id,
            "integration tests",
            "1.0.0",
            "test runner",
            format!("activity-admin-{suffix}"),
        ))
        .await
        .expect("administrator session must be created");
    let regular_session = devices
        .create_session(NewDevice::new(
            regular_user.id,
            "integration tests",
            "1.0.0",
            "test runner",
            format!("activity-user-{suffix}"),
        ))
        .await
        .expect("regular session must be created");
    let repository = ActivityLogRepository::new(database.clone());
    let base_date = DateTimeUtc::from_timestamp(978_307_200, 0).unwrap();
    let middle_date = DateTimeUtc::from_timestamp(978_310_800, 0).unwrap();
    let newest_date = DateTimeUtc::from_timestamp(978_314_400, 0).unwrap();
    let item_id = Uuid::new_v4();

    let mut oldest = NewActivityLog::new(
        format!("Zulu-{marker}"),
        format!("PrimaryType-{marker}"),
        regular_user.id,
    );
    oldest.overview = Some(format!("Overview-{marker}"));
    oldest.short_overview = Some(format!("Short-{marker}"));
    oldest.item_id = Some(item_id.simple().to_string());
    oldest.date_created = Some(base_date);
    oldest.log_severity = LogSeverity::Warning;
    let oldest = repository.create(oldest).await.unwrap();

    let mut middle = NewActivityLog::new(format!("Middle-{marker}"), "PlaybackStart", Uuid::nil());
    middle.date_created = Some(middle_date);
    middle.log_severity = LogSeverity::Debug;
    let middle = repository.create(middle).await.unwrap();

    let mut newest =
        NewActivityLog::new(format!("Alpha-{marker}"), "SessionEnded", regular_user.id);
    newest.date_created = Some(newest_date);
    newest.log_severity = LogSeverity::Information;
    let newest = repository.create(newest).await.unwrap();

    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let route = "/System/ActivityLog/Entries";
    let response = get_response(&app, route).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            route,
            &regular_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            route,
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let all_entries = body_json(response).await;
    assert_eq!(all_entries["StartIndex"], 0);
    assert!(all_entries["TotalRecordCount"].as_u64().unwrap() >= 3);
    assert!(all_entries.get("items").is_none());

    let filtered_uri = format!(
        "{route}?startIndex=0&limit=10&minDate=2001-01-01T00%3A00%3A00Z\
         &maxDate=2001-01-01T00%3A00%3A00Z&hasUserId=true&name={marker}\
         &overview={marker}&shortOverview={marker}&type=PrimaryType-{marker}\
         &itemId={item_id}&username={regular_name}&severity=Warning"
    )
    .replace(' ', "");
    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &filtered_uri,
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let filtered = body_json(response).await;
    assert_eq!(filtered["StartIndex"], 0);
    assert_eq!(filtered["TotalRecordCount"], 1);
    let entry = &filtered["Items"][0];
    assert_eq!(entry["Id"], oldest.id);
    assert_eq!(entry["Name"], format!("Zulu-{marker}"));
    assert_eq!(entry["Overview"], format!("Overview-{marker}"));
    assert_eq!(entry["ShortOverview"], format!("Short-{marker}"));
    assert_eq!(entry["Type"], format!("PrimaryType-{marker}"));
    assert_eq!(entry["ItemId"], item_id.simple().to_string());
    assert_eq!(entry["Date"], "2001-01-01T00:00:00.0000000Z");
    assert_eq!(entry["UserId"], regular_user.id.simple().to_string());
    assert_eq!(entry["Severity"], "Warning");
    assert!(entry.get("UserPrimaryImageTag").is_none());
    assert!(entry.get("user_id").is_none());

    let pagination_uri = format!(
        "{route}?name={marker}&startIndex=1&limit=1\
         &sortBy=Name%2CDateCreated&sortOrder=Descending"
    )
    .replace(' ', "");
    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &pagination_uri,
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = body_json(response).await;
    assert_eq!(page["StartIndex"], 1);
    assert_eq!(page["TotalRecordCount"], 3);
    assert_eq!(page["Items"].as_array().unwrap().len(), 1);
    assert_eq!(page["Items"][0]["Id"], middle.id);

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &format!("{route}?name={marker}&hasUserId=false&severity=1"),
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let without_user = body_json(response).await;
    assert_eq!(without_user["TotalRecordCount"], 1);
    assert_eq!(without_user["Items"][0]["Id"], middle.id);
    assert_eq!(
        without_user["Items"][0]["UserId"],
        Uuid::nil().simple().to_string()
    );

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &format!("{route}?name={marker}&limit=0&sortBy=Overiew&sortOrder=0"),
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let empty_page = body_json(response).await;
    assert_eq!(empty_page["TotalRecordCount"], 3);
    assert!(empty_page["Items"].as_array().unwrap().is_empty());

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &format!(
                "{route}?name={marker}&limit=1&sortBy=Username%2CDateCreated&sortOrder=Ascending"
            ),
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["Items"][0]["Id"], oldest.id);

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            &format!("{route}?name={marker}&limit=1&sortBy=LogSeverity&sortOrder=Descending"),
            &administrator_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["Items"][0]["Id"], oldest.id);

    for invalid_query in [
        "severity=Verbose",
        "sortBy=Item",
        "sortBy=Name&sortOrder=Ascending%2CDescending",
        "minDate=not-a-date",
        "startIndex=-1",
        "limit=2147483648",
    ] {
        let response = app
            .clone()
            .oneshot(authenticated_request(
                "GET",
                &format!("{route}?{invalid_query}"),
                &administrator_session.access_token,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{invalid_query}"
        );
    }

    activity_log::Entity::delete_many()
        .filter(activity_log::Column::Id.is_in([oldest.id, middle.id, newest.id]))
        .exec(&database)
        .await
        .expect("activity route fixtures must be deleted");
    user::Entity::delete_many()
        .filter(user::Column::Id.is_in([administrator.id, regular_user.id]))
        .exec(&database)
        .await
        .expect("activity route users must be deleted");
}

#[tokio::test]
async fn user_management_matches_the_official_controller_flow() {
    let administrator = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    let database_name = format!(
        "{USER_MANAGEMENT_DATABASE_PREFIX}{}",
        Uuid::new_v4().simple()
    );
    assert!(
        database_name.starts_with(USER_MANAGEMENT_DATABASE_PREFIX)
            && database_name[USER_MANAGEMENT_DATABASE_PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "temporary database name must be generated locally"
    );
    administrator
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await
        .expect("temporary PostgreSQL database creation must succeed");

    let task_database_name = database_name.clone();
    let outcome = tokio::spawn(async move {
        exercise_user_management_flow(&task_database_name).await;
    })
    .await;

    administrator
        .execute_unprepared(&format!("DROP DATABASE {database_name} WITH (FORCE)"))
        .await
        .expect("temporary PostgreSQL database cleanup must succeed");
    if let Err(error) = outcome {
        if error.is_panic() {
            std::panic::resume_unwind(error.into_panic());
        }
        panic!("temporary database test task was cancelled: {error}");
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the assertions follow the stateful official controller test order"
)]
async fn exercise_user_management_flow(database_name: &str) {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    user::Entity::delete_many()
        .filter(user::Column::Username.like("api-admin-%"))
        .exec(&database)
        .await
        .expect("stale administrators from interrupted test runs must be removed");
    let users = UserService::new(database.clone());
    let devices = DeviceRepository::new(database.clone());
    let admin_name = format!("api-admin-{}", Uuid::new_v4().simple());
    let admin = users
        .create_initial_administrator(&admin_name)
        .await
        .expect("test administrator must be created");
    let admin_session = devices
        .create_session(NewDevice::new(
            admin.id,
            "integration tests",
            "1.0.0",
            "test runner",
            Uuid::new_v4().simple().to_string(),
        ))
        .await
        .expect("administrator session must be created");
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));
    let username = format!("api-user-{}", Uuid::new_v4().simple());

    let response = get_response(&app, "/Users").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(authenticated_request(
            "GET",
            "/Users",
            &admin_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_json(response)
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["Id"] == admin.id.simple().to_string())
    );

    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": username, "Password": "initial password" }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    assert_eq!(created["Name"], username);
    assert_eq!(created["HasPassword"], true);
    assert_eq!(created["HasConfiguredPassword"], true);
    assert_eq!(created["Policy"]["IsHidden"], true);
    assert_eq!(created["Policy"]["IsAdministrator"], false);
    assert_eq!(
        created["Policy"]["AuthenticationProviderId"],
        "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
    );
    assert!(created.get("name").is_none());
    let id = Uuid::parse_str(created["Id"].as_str().unwrap()).unwrap();

    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": username }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "response body: {body}");
    assert_eq!(body["Message"], "A user with that name already exists");

    for invalid_name in [json!(null), json!(""), json!("   "), json!("‼️")] {
        let response = app
            .clone()
            .oneshot(authenticated_json_request(
                "POST",
                "/Users/New",
                &json!({ "Name": invalid_name }),
                &admin_session.access_token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let missing_id = Uuid::new_v4();
    for (method, uri) in [
        ("GET", format!("/Users/{missing_id}")),
        ("DELETE", format!("/User/{missing_id}")),
    ] {
        let response = app
            .clone()
            .oneshot(authenticated_request(
                method,
                &uri,
                &admin_session.access_token,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let initial_user_session = devices
        .create_session(NewDevice::new(
            id,
            "integration tests",
            "1.0.0",
            "test runner",
            Uuid::new_v4().simple().to_string(),
        ))
        .await
        .expect("regular user session must be created");
    let reset_preserved_user_session = devices
        .create_session(NewDevice::new(
            id,
            "integration tests",
            "1.0.0",
            "second test runner",
            Uuid::new_v4().simple().to_string(),
        ))
        .await
        .expect("second regular user session must be created");
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{id}/Password"),
            &json!({ "ResetPassword": true }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        users
            .get(id)
            .await
            .expect("reset user lookup must succeed")
            .password_hash
            .is_none()
    );
    for token in [
        &initial_user_session.access_token,
        &reset_preserved_user_session.access_token,
    ] {
        assert!(
            devices
                .find_by_token(token)
                .await
                .expect("reset-preserved token lookup must succeed")
                .is_some()
        );
    }
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": format!("forbidden-{}", Uuid::new_v4().simple()) }),
            &initial_user_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{admin_id}/Password", admin_id = admin.id),
            &json!({ "NewPw": "unauthorized replacement" }),
            &initial_user_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let guard_name = format!("admin-guard-{}", Uuid::new_v4().simple());
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            "/Users/New",
            &json!({ "Name": guard_name }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let guard = body_json(response).await;
    let guard_id = guard["Id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "DELETE",
            &format!("/Users/{admin_id}", admin_id = admin.id),
            &initial_user_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let case_renamed = username.to_uppercase();
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users?userId={id}"),
            &json!({ "Name": case_renamed }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let self_renamed = format!("self-{}", Uuid::new_v4().simple());
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{id}"),
            &json!({ "Name": self_renamed }),
            &initial_user_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{id}/Password"),
            &json!({ "NewPw": "4randomPa$$word" }),
            &admin_session.access_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        devices
            .find_by_token(&initial_user_session.access_token)
            .await
            .expect("revoked token lookup must succeed")
            .is_none()
    );
    assert!(
        devices
            .find_by_token(&reset_preserved_user_session.access_token)
            .await
            .expect("revoked second token lookup must succeed")
            .is_none()
    );

    let response = app
        .clone()
        .oneshot(auth_request(
            "/Users/AuthenticateByName",
            &json!({ "Username": self_renamed, "Pw": "4randomPa$$word" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let authentication = body_json(response).await;
    let user_token = authentication["AccessToken"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{id}/Password"),
            &json!({ "CurrentPw": "wrong", "NewPw": "replacement" }),
            user_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            &format!("/Users/{id}/Password"),
            &json!({ "CurrentPw": "4randomPa$$word" }),
            user_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(authenticated_json_request(
            "POST",
            "/Users/Password",
            &json!({ "CurrentPw": "", "NewPw": "replacement" }),
            user_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        devices
            .find_by_token(user_token)
            .await
            .expect("current token lookup must succeed")
            .is_some()
    );

    let response = app
        .clone()
        .oneshot(Request::get("/Users/Public").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let public_users = body_json(response).await;
    assert!(public_users.as_array().unwrap().iter().all(|item| {
        item["Id"]
            .as_str()
            .is_none_or(|value| value != id.simple().to_string())
    }));

    user::Entity::update_many()
        .col_expr(user::Column::IsHidden, Expr::value(false))
        .col_expr(
            user::Column::Policy,
            Expr::value(json!({
                "AuthenticationProviderId": "test.provider",
                "EnableContentDeletion": true
            })),
        )
        .col_expr(
            user::Column::Preferences,
            Expr::value(json!({ "DisplayMissingEpisodes": true })),
        )
        .filter(user::Column::Id.eq(id))
        .exec(&database)
        .await
        .expect("test user must be made public");

    let response = app
        .clone()
        .oneshot(Request::get("/Users/Public").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let public_users = body_json(response).await;
    let public_user = public_users
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["Id"] == id.simple().to_string())
        .expect("newly visible user must be returned");
    assert_eq!(public_user["Name"], self_renamed);
    assert_eq!(public_user["Policy"]["IsHidden"], false);
    assert_eq!(public_user["Policy"]["EnableContentDeletion"], true);
    assert_eq!(public_user["Configuration"]["DisplayMissingEpisodes"], true);

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "DELETE",
            &format!("/Users/{id}"),
            &admin_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        devices
            .find_by_token(user_token)
            .await
            .expect("deleted user token lookup must succeed")
            .is_none()
    );

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "DELETE",
            &format!("/User/{admin_id}", admin_id = admin.id),
            &admin_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await["Message"],
        "There must be at least one administrator"
    );

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "DELETE",
            &format!("/Users/{guard_id}"),
            &admin_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(authenticated_request(
            "DELETE",
            &format!("/User/{admin_id}", admin_id = admin.id),
            &admin_session.access_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    user::Entity::delete_by_id(admin.id)
        .exec(&database)
        .await
        .expect("test administrator must be removed directly during cleanup");
}

#[tokio::test]
async fn health_reports_service_unavailable_when_database_is_disconnected() {
    let database = test_database().await;
    let closed_database = database.clone();
    database.close().await.unwrap();
    let app = jellyfin_api::router(AppState::new(
        closed_database,
        "Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_text(response).await, "Unhealthy");
}

#[tokio::test]
async fn startup_and_password_authentication_use_persisted_device_sessions() {
    let fixture = startup_auth_fixture().await;
    assert_startup_configuration(&fixture).await;
    assert_startup_user_configuration(&fixture).await;
    let token = assert_password_authentication(&fixture).await;
    assert_current_user_and_complete(&fixture, &token).await;
    cleanup_startup_auth_fixture(&fixture, &token).await;
}

struct StartupAuthFixture {
    database: DatabaseConnection,
    users: UserService,
    devices: DeviceRepository,
    app: axum::Router,
    initial_name: String,
    configured_name: String,
    user_id: Uuid,
}

async fn startup_auth_fixture() -> StartupAuthFixture {
    let database = test_database().await;
    let users = UserService::new(database.clone());
    let initial_name = format!("startup-{}", Uuid::new_v4().simple());
    let startup_user = users
        .create_initial_administrator(&initial_name)
        .await
        .expect("startup user must be created");
    let user_id = startup_user.id;
    let configured_name = format!("configured-{}", Uuid::new_v4().simple());
    let missing_user_app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Missing User Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_startup_user(Uuid::new_v4()),
    );
    let response = missing_user_app
        .oneshot(json_request(
            "POST",
            "/Startup/User",
            &json!({ "Name": "admin", "Password": "first password" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let app = jellyfin_api::router(
        AppState::new(
            database.clone(),
            "Test Server".to_owned(),
            "http://127.0.0.1:8096".to_owned(),
        )
        .with_startup_user(user_id),
    );

    StartupAuthFixture {
        devices: DeviceRepository::new(database.clone()),
        database,
        users,
        app,
        initial_name,
        configured_name,
        user_id,
    }
}

async fn assert_startup_configuration(fixture: &StartupAuthFixture) {
    let response = get_response(&fixture.app, "/Startup/Configuration").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["ServerName"], "Test Server");

    let configuration = startup_configuration();
    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Startup/Configuration",
            &configuration,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = get_response(&fixture.app, "/Startup/Configuration").await;
    assert_eq!(body_json(response).await, configuration);
}

fn startup_configuration() -> Value {
    json!({
        "ServerName": "Configured Server",
        "UICulture": "nl-BE",
        "MetadataCountryCode": "be",
        "PreferredMetadataLanguage": "nl"
    })
}

async fn assert_startup_user_configuration(fixture: &StartupAuthFixture) {
    let response = get_response(&fixture.app, "/Startup/User").await;
    assert_eq!(response.status(), StatusCode::OK);
    let first_user = body_json(response).await;
    assert_eq!(first_user["Name"], fixture.initial_name);
    assert_eq!(first_user["Password"], Value::Null);

    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Startup/User",
            &json!({ "Name": fixture.configured_name, "Password": "correct password" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let persisted_user = fixture
        .users
        .get(fixture.user_id)
        .await
        .expect("configured user must load");
    assert_eq!(persisted_user.username, fixture.configured_name);
    assert!(persisted_user.password_hash.is_some());
    let response = get_response(&fixture.app, "/Startup/User").await;
    let configured_user = body_json(response).await;
    assert_eq!(configured_user["Name"], fixture.configured_name);
    assert_eq!(configured_user["Password"], Value::Null);

    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Startup/User",
            &json!({ "Name": "attacker", "Password": "replacement" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        fixture
            .users
            .get(fixture.user_id)
            .await
            .expect("protected user must load")
            .username,
        fixture.configured_name
    );
}

async fn assert_password_authentication(fixture: &StartupAuthFixture) -> String {
    let response = fixture
        .app
        .clone()
        .oneshot(auth_request(
            "/Users/AuthenticateByName",
            &json!({ "Username": fixture.configured_name, "Pw": "wrong password" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let page = fixture
        .devices
        .query(&DeviceQuery {
            user_id: Some(fixture.user_id),
            ..DeviceQuery::default()
        })
        .await
        .expect("device query must succeed");
    assert_eq!(page.total_record_count, 0);

    let response = fixture
        .app
        .clone()
        .oneshot(auth_request(
            "/Users/AuthenticateByName",
            &json!({ "Username": fixture.configured_name, "Pw": "correct password" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let authentication = body_json(response).await;
    let token = authentication["AccessToken"]
        .as_str()
        .expect("access token must be text")
        .to_owned();
    assert_eq!(token.len(), 32);
    assert_eq!(
        authentication["User"]["Id"],
        fixture.user_id.simple().to_string()
    );
    let session = fixture
        .devices
        .find_by_token(&token)
        .await
        .expect("token lookup must succeed")
        .expect("token must have a persisted device session");
    assert_eq!(session.user_id, fixture.user_id);
    assert_eq!(session.app_name, "Jellyfin.Server Integration Tests");
    assert_eq!(session.device_name, "Apple II");
    assert_eq!(session.device_id, "69420");
    assert!(session.is_active);
    token
}

async fn assert_current_user_and_complete(fixture: &StartupAuthFixture, token: &str) {
    let response = get_response(&fixture.app, "/Users/Me").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::get("/Users/Me")
                .header(
                    header::AUTHORIZATION,
                    format!("{CLIENT_AUTHORIZATION}, Token=\"{token}\""),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let current_user = body_json(response).await;
    assert_eq!(current_user["Id"], fixture.user_id.simple().to_string());
    assert_eq!(current_user["Name"], fixture.configured_name);

    let response = post_empty_response(&fixture.app, "/Startup/Complete").await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    for uri in ["/Startup/User", "/Startup/Configuration"] {
        let response = get_response(&fixture.app, uri).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            "POST",
            "/Startup/Configuration",
            &startup_configuration(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = get_response(&fixture.app, "/System/Info/Public").await;
    let public_info = body_json(response).await;
    assert_eq!(public_info["ServerName"], "Configured Server");
    assert_eq!(public_info["StartupWizardCompleted"], true);
}

async fn cleanup_startup_auth_fixture(fixture: &StartupAuthFixture, token: &str) {
    user::Entity::delete_by_id(fixture.user_id)
        .exec(&fixture.database)
        .await
        .expect("startup test user must be removed");
    assert!(
        fixture
            .devices
            .find_by_token(token)
            .await
            .expect("cascade token lookup must succeed")
            .is_none()
    );
}

async fn get_response(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn post_empty_response(app: &axum::Router, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::post(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn test_database() -> DatabaseConnection {
    let database = jellyfin_data::connect(&jellyfin_data::DatabaseConfig::default())
        .await
        .expect("local PostgreSQL must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");
    database
}

fn json_request(method: &str, uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn authenticated_json_request(method: &str, uri: &str, body: &Value, token: &str) -> Request<Body> {
    authenticated_request(
        method,
        uri,
        token,
        Body::from(serde_json::to_vec(body).unwrap()),
    )
}

fn authenticated_request(method: &str, uri: &str, token: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::AUTHORIZATION,
            format!("{CLIENT_AUTHORIZATION}, Token=\"{token}\""),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

fn auth_request(uri: &str, body: &Value) -> Request<Body> {
    Request::post(uri)
        .header(header::AUTHORIZATION, CLIENT_AUTHORIZATION)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), MAX_RESPONSE_SIZE)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
