#![allow(clippy::too_many_lines)]
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use futures_util::StreamExt;
use jellyfin_api::AppState;
use jellyfin_controller::UserService;
use jellyfin_data::{
    BaseItemRepository, DatabaseConfig, DeviceRepository, NewBaseItem, NewDevice, entities::user,
};
use jellyfin_model::{SyncPlayUserAccessType, UserPolicy};
use sea_orm::{ConnectionTrait, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const AUTHORIZATION: &str = "MediaBrowser Client=\"SyncPlay Tests\", DeviceId=\"sync-play-tests\", Device=\"Test\", Version=\"1.0\"";
const DATABASE_PREFIX: &str = "jellyfin_sync_play_routes_";

#[tokio::test]
async fn sync_play_group_lifecycle_matches_official_postgres_policy_contract() {
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
        exercise_sync_play_routes(&task_database_name).await;
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

async fn exercise_sync_play_routes(database_name: &str) {
    let database = jellyfin_data::connect(&DatabaseConfig {
        url: format!("postgres://postgres:123456@127.0.0.1:5432/{database_name}"),
        max_connections: 8,
        min_connections: 1,
    })
    .await
    .expect("temporary PostgreSQL database must be available");
    jellyfin_data::migrate(&database)
        .await
        .expect("PostgreSQL migrations must succeed");

    let suffix = Uuid::new_v4().simple().to_string();
    let users = UserService::new(database.clone());
    let creator = users
        .create_initial_administrator(&format!("creator-{suffix}"))
        .await
        .unwrap();
    let joiner = users.create(&format!("joiner-{suffix}")).await.unwrap();
    let blocked = users.create(&format!("blocked-{suffix}")).await.unwrap();
    set_sync_play_access(&users, joiner.id, SyncPlayUserAccessType::JoinGroups).await;
    set_sync_play_access(&users, blocked.id, SyncPlayUserAccessType::None).await;
    let items = BaseItemRepository::new(database.clone());
    let mut first = NewBaseItem::new(Uuid::new_v4(), "Movie");
    first.runtime_ticks = Some(50_000);
    let first_item = items.create(first).await.unwrap();
    let second_item = items
        .create(NewBaseItem::new(Uuid::new_v4(), "Movie"))
        .await
        .unwrap();

    let devices = DeviceRepository::new(database.clone());
    let creator_token = session(&devices, creator.id, "creator", &suffix).await;
    let joiner_token = session(&devices, joiner.id, "joiner", &suffix).await;
    let blocked_token = session(&devices, blocked.id, "blocked", &suffix).await;
    let app = jellyfin_api::router(AppState::new(
        database.clone(),
        "SyncPlay Test Server".to_owned(),
        "http://127.0.0.1:8096".to_owned(),
    ));

    assert_eq!(
        request(&app, "GET", "/SyncPlay/List", None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/SyncPlay/New",
            Some(&joiner_token),
            Some(json!({ "GroupName": "Denied" })),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "GET", "/SyncPlay/List", Some(&blocked_token), None,)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/SyncPlay/New",
            Some(&creator_token),
            Some(json!({ "GroupName": "x".repeat(201) })),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let created = response_json(
        request(
            &app,
            "POST",
            "/SyncPlay/New",
            Some(&creator_token),
            Some(json!({ "GroupName": "  Living Room  " })),
        )
        .await,
    )
    .await;
    assert_eq!(created["GroupName"], "Living Room");
    assert_eq!(created["State"], "Idle");
    assert_eq!(created["Participants"], json!([creator.username]));
    let group_id = created["GroupId"].as_str().unwrap().to_owned();
    assert_eq!(group_id.len(), 32);

    let listed =
        response_json(request(&app, "GET", "/SyncPlay/List", Some(&joiner_token), None).await)
            .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["GroupId"], group_id);
    let fetched = response_json(
        request(
            &app,
            "GET",
            &format!("/SyncPlay/{group_id}"),
            Some(&joiner_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(fetched["GroupName"], "Living Room");
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/SyncPlay/{}", Uuid::new_v4()),
            Some(&joiner_token),
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let join_response = request(
        &app,
        "POST",
        "/SyncPlay/Join",
        Some(&joiner_token),
        Some(json!({ "GroupId": group_id })),
    )
    .await;
    assert_eq!(join_response.status(), StatusCode::NO_CONTENT);
    assert!(
        to_bytes(join_response.into_body(), 1)
            .await
            .unwrap()
            .is_empty()
    );
    let after_join = response_json(
        request(
            &app,
            "GET",
            &format!("/SyncPlay/{group_id}"),
            Some(&creator_token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        after_join["Participants"],
        json!([creator.username, joiner.username])
    );

    assert_eq!(
        request(
            &app,
            "POST",
            "/SyncPlay/SetNewQueue",
            Some(&blocked_token),
            Some(json!({
                "PlayingQueue": [first_item.id],
                "PlayingItemPosition": 0,
                "StartPositionTicks": 10
            })),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/SetNewQueue",
            Some(&creator_token),
            Some(json!({
                "PlayingQueue": [first_item.id, second_item.id, first_item.id],
                "PlayingItemPosition": 1,
                "StartPositionTicks": 12345
            })),
        )
        .await,
    )
    .await;
    assert_eq!(
        response_json(
            request(
                &app,
                "GET",
                &format!("/SyncPlay/{group_id}"),
                Some(&joiner_token),
                None,
            )
            .await,
        )
        .await["State"],
        "Waiting"
    );

    assert_eq!(
        request(
            &app,
            "POST",
            "/SyncPlay/Ping",
            None,
            Some(json!({ "Ping": 12 })),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Ping",
            Some(&blocked_token),
            Some(json!({ "Ping": 12 })),
        )
        .await,
    )
    .await;
    assert_eq!(
        request(
            &app,
            "POST",
            "/SyncPlay/Buffering",
            Some(&blocked_token),
            Some(json!({
                "When": "2026-07-25T08:30:00Z",
                "PositionTicks": 10,
                "IsPlaying": false,
                "PlaylistItemId": Uuid::new_v4()
            })),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Buffering",
            Some(&creator_token),
            Some(json!({
                "When": "2026-07-25T08:30:00Z",
                "PositionTicks": 10,
                "IsPlaying": false,
                "PlaylistItemId": Uuid::new_v4()
            })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Ready",
            Some(&creator_token),
            Some(json!({
                "When": "2026-07-25T08:30:00Z",
                "PositionTicks": 10,
                "IsPlaying": false,
                "PlaylistItemId": Uuid::new_v4()
            })),
        )
        .await,
    )
    .await;
    assert_group_state(&app, &group_id, &creator_token, "Waiting").await;
    for token in [&creator_token, &joiner_token] {
        assert_no_content(
            request(
                &app,
                "POST",
                "/SyncPlay/SetIgnoreWait",
                Some(token),
                Some(json!({ "IgnoreWait": true })),
            )
            .await,
        )
        .await;
    }
    assert_group_state(&app, &group_id, &creator_token, "Playing").await;

    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Unpause",
            Some(&creator_token),
            None,
        )
        .await,
    )
    .await;
    assert_group_state(&app, &group_id, &creator_token, "Playing").await;
    assert_no_content(request(&app, "POST", "/SyncPlay/Pause", Some(&joiner_token), None).await)
        .await;
    assert_group_state(&app, &group_id, &creator_token, "Paused").await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Seek",
            Some(&creator_token),
            Some(json!({ "PositionTicks": 999_999 })),
        )
        .await,
    )
    .await;
    assert_group_state(&app, &group_id, &creator_token, "Waiting").await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/SetRepeatMode",
            Some(&creator_token),
            Some(json!({ "Mode": "RepeatAll" })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/SetShuffleMode",
            Some(&creator_token),
            Some(json!({ "Mode": "Shuffle" })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/NextItem",
            Some(&creator_token),
            Some(json!({ "PlaylistItemId": Uuid::new_v4() })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/PreviousItem",
            Some(&creator_token),
            Some(json!({ "PlaylistItemId": Uuid::new_v4() })),
        )
        .await,
    )
    .await;
    assert_eq!(
        request(&app, "POST", "/SyncPlay/Pause", Some(&blocked_token), None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_no_content(request(&app, "POST", "/SyncPlay/Stop", Some(&creator_token), None).await)
        .await;
    assert_group_state(&app, &group_id, &creator_token, "Idle").await;

    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/SetNewQueue",
            Some(&joiner_token),
            Some(json!({
                "PlayingQueue": [Uuid::new_v4()],
                "PlayingItemPosition": 0,
                "StartPositionTicks": 0
            })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/Queue",
            Some(&joiner_token),
            Some(json!({ "ItemIds": [second_item.id], "Mode": "QueueNext" })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/SetPlaylistItem",
            Some(&creator_token),
            Some(json!({ "PlaylistItemId": Uuid::new_v4() })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/MovePlaylistItem",
            Some(&creator_token),
            Some(json!({ "PlaylistItemId": Uuid::new_v4(), "NewIndex": -10 })),
        )
        .await,
    )
    .await;
    assert_no_content(
        request(
            &app,
            "POST",
            "/SyncPlay/RemoveFromPlaylist",
            Some(&creator_token),
            Some(json!({
                "PlaylistItemIds": [],
                "ClearPlaylist": true,
                "ClearPlayingItem": true
            })),
        )
        .await,
    )
    .await;
    assert_eq!(
        response_json(
            request(
                &app,
                "GET",
                &format!("/SyncPlay/{group_id}"),
                Some(&creator_token),
                None,
            )
            .await,
        )
        .await["State"],
        "Idle"
    );

    assert_eq!(
        request(&app, "POST", "/SyncPlay/Leave", Some(&blocked_token), None,)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", "/SyncPlay/Leave", Some(&joiner_token), None,)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&app, "POST", "/SyncPlay/Leave", Some(&creator_token), None,)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        response_json(request(&app, "GET", "/SyncPlay/List", Some(&joiner_token), None,).await,)
            .await,
        json!([])
    );

    exercise_websocket_commands_and_disconnect(
        &app,
        &creator_token,
        &joiner_token,
        &joiner.username,
        first_item.id,
    )
    .await;

    user::Entity::delete_many()
        .exec(&database)
        .await
        .expect("test user cleanup");
    database.close().await.unwrap();
}

async fn exercise_websocket_commands_and_disconnect(
    app: &Router,
    creator_token: &str,
    joiner_token: &str,
    joiner_username: &str,
    item_id: Uuid,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, server_app).await });
    let (mut creator_socket, response) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/websocket?api_key={creator_token}"
    ))
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let keep_alive = websocket_json(&mut creator_socket).await;
    assert_eq!(keep_alive["MessageType"], "ForceKeepAlive");
    let (mut joiner_socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/websocket?api_key={joiner_token}"
    ))
    .await
    .unwrap();
    assert_eq!(
        websocket_json(&mut joiner_socket).await["MessageType"],
        "ForceKeepAlive"
    );

    let created = response_json(
        request(
            app,
            "POST",
            "/SyncPlay/New",
            Some(creator_token),
            Some(json!({ "GroupName": "Socket Group" })),
        )
        .await,
    )
    .await;
    assert_eq!(created["Participants"].as_array().unwrap().len(), 1);
    let creator_joined = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(creator_joined["Data"]["Type"], "GroupJoined");
    assert_eq!(creator_joined["Data"]["GroupId"], created["GroupId"]);
    assert_no_content(
        request(
            app,
            "POST",
            "/SyncPlay/Join",
            Some(joiner_token),
            Some(json!({ "GroupId": created["GroupId"] })),
        )
        .await,
    )
    .await;
    let joiner_joined = websocket_message_of(&mut joiner_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(joiner_joined["Data"]["Type"], "GroupJoined");
    assert_eq!(joiner_joined["Data"]["GroupId"], created["GroupId"]);
    let user_joined = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(user_joined["Data"]["Type"], "UserJoined");
    assert_no_content(
        request(
            app,
            "POST",
            "/SyncPlay/SetNewQueue",
            Some(creator_token),
            Some(json!({
                "PlayingQueue": [item_id],
                "PlayingItemPosition": 0,
                "StartPositionTicks": 0
            })),
        )
        .await,
    )
    .await;
    let queue_update = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(queue_update["Data"]["Type"], "PlayQueue");
    assert_eq!(queue_update["Data"]["Data"]["Reason"], "NewPlaylist");
    assert_eq!(
        queue_update["Data"]["Data"]["Playlist"][0]["ItemId"],
        item_id.simple().to_string()
    );
    assert_eq!(queue_update["Data"]["Data"]["PlayingItemIndex"], 0);
    assert_eq!(
        websocket_message_of(&mut joiner_socket, "SyncPlayGroupUpdate").await,
        queue_update
    );
    assert_no_content(request(app, "POST", "/SyncPlay/Unpause", Some(creator_token), None).await)
        .await;
    let command = websocket_message_of(&mut creator_socket, "SyncPlayCommand").await;
    assert_eq!(command["Data"]["Command"], "Unpause");
    assert_eq!(command["Data"]["GroupId"], created["GroupId"]);
    assert_eq!(command["Data"]["PositionTicks"], 0);
    assert_eq!(
        websocket_message_of(&mut joiner_socket, "SyncPlayCommand").await,
        command
    );
    let state_update = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(state_update["Data"]["Type"], "StateUpdate");
    assert_eq!(state_update["Data"]["GroupId"], created["GroupId"]);
    assert_eq!(state_update["Data"]["Data"]["State"], "Playing");
    assert_eq!(state_update["Data"]["Data"]["Reason"], "Unpause");
    assert_eq!(
        websocket_message_of(&mut joiner_socket, "SyncPlayGroupUpdate").await,
        state_update
    );

    assert_no_content(request(app, "POST", "/SyncPlay/Unpause", Some(creator_token), None).await)
        .await;
    let current_session = websocket_message_of(&mut creator_socket, "SyncPlayCommand").await;
    assert_eq!(current_session["Data"]["Command"], "Unpause");

    assert_no_content(
        request(
            app,
            "POST",
            "/SyncPlay/Seek",
            Some(creator_token),
            Some(json!({ "PositionTicks": 0 })),
        )
        .await,
    )
    .await;
    let seek = websocket_message_of(&mut creator_socket, "SyncPlayCommand").await;
    assert_eq!(seek["Data"]["Command"], "Seek");
    assert_eq!(
        websocket_message_of(&mut joiner_socket, "SyncPlayCommand").await,
        seek
    );
    let waiting = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(waiting["Data"]["Type"], "StateUpdate");
    assert_eq!(waiting["Data"]["Data"]["State"], "Waiting");
    assert_eq!(waiting["Data"]["Data"]["Reason"], "Seek");
    assert_eq!(
        websocket_message_of(&mut joiner_socket, "SyncPlayGroupUpdate").await,
        waiting
    );
    assert_no_content(
        request(
            app,
            "POST",
            "/SyncPlay/Ready",
            Some(creator_token),
            Some(json!({
                "When": "2026-07-25T08:30:00Z",
                "PositionTicks": 0,
                "IsPlaying": false,
                "PlaylistItemId": queue_update["Data"]["Data"]["Playlist"][0]["PlaylistItemId"]
            })),
        )
        .await,
    )
    .await;
    let pause_when_ready = websocket_message_of(&mut creator_socket, "SyncPlayCommand").await;
    assert_eq!(pause_when_ready["Data"]["Command"], "Pause");

    joiner_socket.close(None).await.unwrap();
    let resumed = websocket_message_of(&mut creator_socket, "SyncPlayCommand").await;
    assert_eq!(resumed["Data"]["Command"], "Unpause");
    let resumed_state = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(resumed_state["Data"]["Type"], "StateUpdate");
    assert_eq!(resumed_state["Data"]["Data"]["State"], "Playing");
    assert_eq!(resumed_state["Data"]["Data"]["Reason"], "Unpause");
    let user_left = websocket_message_of(&mut creator_socket, "SyncPlayGroupUpdate").await;
    assert_eq!(user_left["Data"]["Type"], "UserLeft");
    assert_eq!(user_left["Data"]["GroupId"], created["GroupId"]);
    assert_eq!(user_left["Data"]["Data"], joiner_username);
    creator_socket.close(None).await.unwrap();
    for _ in 0..100 {
        let groups =
            response_json(request(app, "GET", "/SyncPlay/List", Some(creator_token), None).await)
                .await;
        if groups.as_array().unwrap().is_empty() {
            server.abort();
            return;
        }
        tokio::task::yield_now().await;
    }
    server.abort();
    panic!("last WebSocket disconnect did not remove the empty SyncPlay group");
}

async fn websocket_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let message = socket.next().await.unwrap().unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

async fn websocket_message_of(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message_type: &str,
) -> Value {
    for _ in 0..16 {
        let message = websocket_json(socket).await;
        if message["MessageType"] == message_type {
            return message;
        }
    }
    panic!("expected a {message_type} WebSocket message");
}

async fn set_sync_play_access(users: &UserService, user_id: Uuid, access: SyncPlayUserAccessType) {
    let policy = UserPolicy {
        authentication_provider_id: Some(UserPolicy::DEFAULT_AUTHENTICATION_PROVIDER_ID.into()),
        password_reset_provider_id: Some(UserPolicy::DEFAULT_PASSWORD_RESET_PROVIDER_ID.into()),
        sync_play_access: access,
        ..UserPolicy::default()
    };
    users.update_policy(user_id, &policy).await.unwrap();
}

async fn session(devices: &DeviceRepository, user_id: Uuid, role: &str, suffix: &str) -> String {
    devices
        .create_session(NewDevice::new(
            user_id,
            "SyncPlay Tests",
            "1.0",
            "Test",
            format!("{role}-{suffix}"),
        ))
        .await
        .unwrap()
        .access_token
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(
            header::AUTHORIZATION,
            format!("{AUTHORIZATION}, Token=\"{token}\""),
        );
    }
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).unwrap())
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

async fn assert_no_content(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(to_bytes(response.into_body(), 1).await.unwrap().is_empty());
}

async fn assert_group_state(app: &Router, group_id: &str, token: &str, expected: &str) {
    let group = response_json(
        request(
            app,
            "GET",
            &format!("/SyncPlay/{group_id}"),
            Some(token),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(group["State"], expected);
}

fn assert_temporary_database_name(database_name: &str) {
    assert!(database_name.starts_with(DATABASE_PREFIX));
    assert!(
        database_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
}
