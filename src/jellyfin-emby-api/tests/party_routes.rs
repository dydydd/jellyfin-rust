use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{ApiKeyRepository, DatabaseConfig, DeviceRepository, NewDevice};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"Emby Party Tests\", DeviceId=\"emby-party-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_emby_parties_";

#[tokio::test]
async fn parties_match_emby_session_behavior_and_remain_protocol_local() {
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

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let host = users
        .create_initial_administrator(&format!("party-host-{suffix}"))
        .await
        .expect("host user");
    let guest = users
        .create(&format!("party-guest-{suffix}"))
        .await
        .expect("guest user");
    let devices = DeviceRepository::new(database.clone());
    let host_token = devices
        .create_session(NewDevice::new(
            host.id,
            "Emby Party Host",
            "1.0",
            "Host Device",
            "party-host-device",
        ))
        .await
        .expect("host session")
        .access_token;
    let guest_token = devices
        .create_session(NewDevice::new(
            guest.id,
            "Emby Party Guest",
            "1.0",
            "Guest Device",
            "party-guest-device",
        ))
        .await
        .expect("guest session")
        .access_token;
    let api_key = ApiKeyRepository::new(database.clone())
        .create(&format!("party-key-{suffix}"))
        .await
        .expect("API key")
        .access_token;

    let state = AppState::new(
        database.clone(),
        "Emby Party Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    );
    let emby = jellyfin_emby_api::router(state.clone());
    let jellyfin = jellyfin_api::router(state);

    assert_authentication_and_empty_session_contract(&emby, &host_token).await;
    let party_id = assert_create_list_and_info(&emby, &host_token, &host.username).await;
    assert_join_and_messages(&emby, &party_id, &host_token, &guest_token, &guest.username).await;
    assert_leave_and_api_key_semantics(&emby, &party_id, &host_token, &guest_token, &api_key).await;
    assert_protocol_isolation(&emby, &jellyfin, &api_key).await;

    drop(emby);
    drop(jellyfin);
    database.close().await.expect("database pool cleanup");
}

async fn assert_authentication_and_empty_session_contract(app: &Router, token: &str) {
    for (method, path, body) in [
        (Method::GET, "/emby/Parties", None),
        (Method::POST, "/emby/Parties", None),
        (Method::GET, "/emby/Parties/Info", None),
        (Method::GET, "/emby/Parties/Messages", None),
        (Method::POST, "/emby/Parties/Messages", Some("{")),
        (Method::POST, "/emby/Parties/Leave", None),
        (Method::POST, "/emby/Parties/unknown/Join", None),
    ] {
        assert_eq!(
            request(app, method, path, body, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}",
        );
    }

    assert_eq!(
        json_response(request(app, Method::GET, "/emby/Parties", None, Some(token)).await).await,
        json!({"Items": [], "TotalRecordCount": 0}),
    );
    assert_eq!(
        json_response(request(app, Method::GET, "/emby/Parties/Info", None, Some(token),).await,)
            .await,
        json!({}),
    );
    assert_eq!(
        json_response(
            request(
                app,
                Method::GET,
                "/emby/Parties/Messages",
                None,
                Some(token),
            )
            .await,
        )
        .await,
        json!({"Items": [], "TotalRecordCount": 0}),
    );
    assert_empty_ok(request(app, Method::POST, "/emby/Parties/Leave", None, Some(token)).await)
        .await;
    assert_eq!(
        request(
            app,
            Method::POST,
            "/emby/Parties/Messages",
            Some(r#"{"Message":"not joined"}"#),
            Some(token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        request(
            app,
            Method::POST,
            "/emby/Parties/Messages",
            Some("{"),
            Some(token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        request(
            app,
            Method::GET,
            "/emby/Parties/Messages?limit=2147483648",
            None,
            Some(token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
}

async fn assert_create_list_and_info(app: &Router, token: &str, username: &str) -> String {
    let response = request(
        app,
        Method::POST,
        "/emby/pArTiEs?Name=discarded&NAME=Movie+Night",
        None,
        Some(token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = json_response(response).await;
    let party = &created["PartyInfo"];
    let party_id = party["Id"].as_str().expect("party id").to_owned();
    assert_eq!(party_id.len(), 32);
    assert!(party_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(party["Name"], "Movie Night");
    assert_eq!(party["Sessions"].as_array().unwrap().len(), 1);
    assert_eq!(party["Sessions"][0]["User"]["Name"], username);
    assert_eq!(party["Sessions"][0]["IsHost"], true);

    let listed =
        json_response(request(app, Method::GET, "/emby/PARTIES", None, Some(token)).await).await;
    assert_eq!(listed["TotalRecordCount"], 1);
    assert_eq!(listed["Items"][0]["Id"], party_id);
    assert_eq!(listed["Items"][0]["Name"], "Movie Night");

    let info =
        json_response(request(app, Method::GET, "/emby/parties/info", None, Some(token)).await)
            .await;
    assert_eq!(info, created);
    party_id
}

async fn assert_join_and_messages(
    app: &Router,
    party_id: &str,
    host_token: &str,
    guest_token: &str,
    guest_name: &str,
) {
    assert_eq!(
        request(
            app,
            Method::POST,
            "/emby/Parties/not-found/Join",
            None,
            Some(guest_token),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );

    let joined = json_response(
        request(
            app,
            Method::POST,
            &format!("/emby/pArTiEs/{}/jOiN", party_id.to_uppercase()),
            None,
            Some(guest_token),
        )
        .await,
    )
    .await;
    let sessions = joined["PartyInfo"]["Sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["IsHost"], true);
    assert_eq!(sessions[1]["IsHost"], false);
    assert_eq!(sessions[1]["User"]["Name"], guest_name);

    assert_empty_ok(
        request(
            app,
            Method::POST,
            "/emby/PaRtIeS/MeSsAgEs",
            Some(
                r#"{"UserId":1,"USERID":"9223372036854775807","DateTime":"2026-09-14T01:02:03+00:00","Message":"discarded","MESSAGE":"hello","Unknown":true}"#,
            ),
            Some(guest_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        request(
            app,
            Method::POST,
            "/emby/Parties/Messages",
            Some(r#"{"UserId":"bad"}"#),
            Some(guest_token),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_empty_ok(
        request(
            app,
            Method::POST,
            "/emby/Parties/Messages",
            Some("{}"),
            Some(guest_token),
        )
        .await,
    )
    .await;

    let messages = json_response(
        request(
            app,
            Method::GET,
            "/emby/Parties/Messages?StartIndex=99&STARTINDEX=-1&Limit=1",
            None,
            Some(host_token),
        )
        .await,
    )
    .await;
    assert_eq!(messages["TotalRecordCount"], 2);
    assert_eq!(messages["Items"].as_array().unwrap().len(), 1);
    assert_eq!(messages["Items"][0]["Message"], "hello");
    assert_eq!(messages["Items"][0]["User"]["Name"], guest_name);
    assert_eq!(messages["Items"][0]["DateTime"], "2026-09-14T01:02:03Z");
    // Emby 4.10.0.40 registers a global PartyMessage -> PartyMessageDto
    // serializer, replacing numeric UserId with a User DTO on the wire.
    assert!(messages["Items"][0].get("UserId").is_none());
    assert!(messages["Items"][0].get("User").is_some());

    let negative_limit = json_response(
        request(
            app,
            Method::GET,
            "/emby/Parties/Messages?limit=-1",
            None,
            Some(host_token),
        )
        .await,
    )
    .await;
    assert_eq!(negative_limit, json!({"Items": [], "TotalRecordCount": 2}));
}

async fn assert_leave_and_api_key_semantics(
    app: &Router,
    party_id: &str,
    host_token: &str,
    guest_token: &str,
    api_key: &str,
) {
    for _ in 0..2 {
        assert_empty_ok(
            request(
                app,
                Method::POST,
                "/emby/Parties/Leave",
                None,
                Some(host_token),
            )
            .await,
        )
        .await;
    }
    let host_info = json_response(
        request(
            app,
            Method::GET,
            "/emby/Parties/Info",
            None,
            Some(host_token),
        )
        .await,
    )
    .await;
    assert_eq!(host_info, json!({}));
    let guest_info = json_response(
        request(
            app,
            Method::GET,
            "/emby/Parties/Info",
            None,
            Some(guest_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        guest_info["PartyInfo"]["Sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(guest_info["PartyInfo"]["Sessions"][0]["IsHost"], false);

    assert_empty_ok(
        request(
            app,
            Method::POST,
            "/emby/Parties/Leave",
            None,
            Some(guest_token),
        )
        .await,
    )
    .await;
    assert_eq!(
        json_response(request(app, Method::GET, "/emby/Parties", None, Some(api_key)).await).await,
        json!({"Items": [], "TotalRecordCount": 0}),
    );

    assert_eq!(
        request(app, Method::POST, "/emby/Parties", None, Some(api_key))
            .await
            .status(),
        StatusCode::BAD_REQUEST,
    );
    let parties =
        json_response(request(app, Method::GET, "/emby/Parties", None, Some(api_key)).await).await;
    assert_eq!(parties["TotalRecordCount"], 1);
    assert_eq!(parties["Items"][0]["Sessions"], json!([]));
    let orphan_id = parties["Items"][0]["Id"].as_str().unwrap();
    assert_ne!(orphan_id, party_id);
    assert_eq!(
        request(
            app,
            Method::POST,
            &format!("/emby/Parties/{orphan_id}/Join"),
            None,
            Some(api_key),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        json_response(request(app, Method::GET, "/emby/Parties/Info", None, Some(api_key),).await,)
            .await,
        json!({}),
    );
    assert_eq!(
        request(
            app,
            Method::POST,
            "/emby/Parties/Messages",
            Some("{}"),
            Some(api_key),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
    );
    assert_empty_ok(
        request(
            app,
            Method::POST,
            "/emby/Parties/Leave",
            None,
            Some(api_key),
        )
        .await,
    )
    .await;
}

async fn assert_protocol_isolation(emby: &Router, jellyfin: &Router, token: &str) {
    for (method, path, body) in [
        (Method::GET, "/Parties", None),
        (Method::POST, "/Parties", None),
        (Method::GET, "/api/Parties/Info", None),
        (Method::GET, "/api/Parties/Messages", None),
        (Method::POST, "/Parties/Messages", Some("{}")),
        (Method::POST, "/api/Parties/Leave", None),
        (Method::POST, "/Parties/id/Join", None),
    ] {
        assert_ne!(
            request(jellyfin, method, path, body, Some(token))
                .await
                .status(),
            StatusCode::OK,
            "Emby Party route leaked at {path}",
        );
    }
    let parties =
        json_response(request(emby, Method::GET, "/emby/Parties", None, Some(token)).await).await;
    assert_eq!(
        parties["TotalRecordCount"], 1,
        "root/API requests must not mutate the Emby registry",
    );
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(uri);
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

async fn json_response(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body"),
    )
    .expect("JSON response")
}

async fn assert_empty_ok(response: axum::response::Response) {
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
