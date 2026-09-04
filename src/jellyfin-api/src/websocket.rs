use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use axum::{
    extract::{OriginalUri, State, WebSocketUpgrade, ws::Message},
    http::HeaderMap,
    response::Response,
};
use jellyfin_server_implementations::{
    SyncPlayDeparture, WebSocketMessageType, deserialize_websocket_message,
};
use serde_json::json;
use tokio::{
    sync::{RwLock, broadcast, watch},
    time::{Duration, Instant, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{ApiError, AppState, SystemCommand, authentication, session::jellyfin_session_id};

const LOST_TIMEOUT: Duration = Duration::from_secs(60);
const FORCE_KEEP_ALIVE_AFTER: Duration = Duration::from_secs(45);
const WATCH_INTERVAL: Duration = Duration::from_secs(12);

#[derive(Debug)]
pub(crate) struct WebSocketHub {
    sessions: RwLock<HashMap<Uuid, WebSocketSession>>,
    shutdown: watch::Sender<Option<SystemCommand>>,
}

#[derive(Debug)]
struct WebSocketSession {
    session_id: String,
    sender: broadcast::Sender<Arc<str>>,
    user_ids: HashSet<Uuid>,
    authenticated_user_id: Uuid,
    is_administrator: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionRecipient {
    user_id: Uuid,
    is_administrator: bool,
}

impl WebSocketHub {
    pub(crate) fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            shutdown: watch::channel(None).0,
        }
    }

    pub(crate) fn subscribe_shutdown(&self) -> watch::Receiver<Option<SystemCommand>> {
        self.shutdown.subscribe()
    }

    pub(crate) fn notify_shutdown(&self, command: SystemCommand) {
        self.shutdown.send_replace(Some(command));
    }

    async fn subscribe(
        &self,
        session_id: &str,
        authenticated_user_id: Uuid,
        is_administrator: bool,
        additional_user_ids: impl IntoIterator<Item = Uuid>,
    ) -> (Uuid, broadcast::Receiver<Arc<str>>) {
        let mut sessions = self.sessions.write().await;
        let connection_id = Uuid::new_v4();
        let mut user_ids = additional_user_ids.into_iter().collect::<HashSet<_>>();
        user_ids.insert(authenticated_user_id);
        let sender = broadcast::channel(64).0;
        let receiver = sender.subscribe();
        sessions.insert(
            connection_id,
            WebSocketSession {
                session_id: session_id.to_owned(),
                sender,
                user_ids,
                authenticated_user_id,
                is_administrator,
            },
        );
        (connection_id, receiver)
    }

    async fn unsubscribe(&self, connection_id: Uuid) -> bool {
        let mut sessions = self.sessions.write().await;
        let session_id = sessions
            .remove(&connection_id)
            .map(|session| session.session_id);
        session_id.is_some_and(|removed_session_id| {
            !sessions
                .values()
                .any(|session| session.session_id == removed_session_id)
        })
    }

    pub(crate) async fn is_connected(&self, session_id: &str) -> bool {
        self.sessions
            .read()
            .await
            .values()
            .any(|session| session.session_id == session_id && session.sender.receiver_count() > 0)
    }

    pub(crate) async fn send<T: serde::Serialize>(
        &self,
        session_ids: &[String],
        message_type: &str,
        data: &T,
    ) {
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        let message = Arc::<str>::from(websocket_message(message_type, &data));
        let sessions = self.sessions.read().await;
        for session in sessions.values() {
            if session_ids.contains(&session.session_id) {
                let _ = session.sender.send(Arc::clone(&message));
            }
        }
    }

    async fn send_to_users<T: serde::Serialize>(
        &self,
        user_ids: &[Uuid],
        message_type: &str,
        data: &T,
    ) {
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        let message = Arc::<str>::from(websocket_message(message_type, &data));
        let sessions = self.sessions.read().await;
        for session in sessions.values() {
            if user_ids
                .iter()
                .any(|user_id| session.user_ids.contains(user_id))
            {
                let _ = session.sender.send(Arc::clone(&message));
            }
        }
    }

    pub(crate) async fn send_to_administrators<T: serde::Serialize>(
        &self,
        message_type: &str,
        data: &T,
    ) {
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        let message = Arc::<str>::from(websocket_message(message_type, &data));
        let sessions = self.sessions.read().await;
        for session in sessions.values().filter(|session| session.is_administrator) {
            let _ = session.sender.send(Arc::clone(&message));
        }
    }

    async fn set_user_administrator(&self, user_id: Uuid, is_administrator: bool) {
        for session in self
            .sessions
            .write()
            .await
            .values_mut()
            .filter(|session| session.authenticated_user_id == user_id)
        {
            session.is_administrator = is_administrator;
        }
    }

    pub(crate) async fn add_session_user(&self, session_id: &str, user_id: Uuid) {
        for session in self
            .sessions
            .write()
            .await
            .values_mut()
            .filter(|session| session.session_id == session_id)
        {
            session.user_ids.insert(user_id);
        }
    }

    pub(crate) async fn remove_session_user(&self, session_id: &str, user_id: Uuid) {
        for session in self
            .sessions
            .write()
            .await
            .values_mut()
            .filter(|session| session.session_id == session_id)
        {
            if session.authenticated_user_id != user_id {
                session.user_ids.remove(&user_id);
            }
        }
    }

    async fn recipients(&self) -> Vec<(Uuid, SessionRecipient)> {
        self.sessions
            .read()
            .await
            .iter()
            .filter(|(_, session)| session.sender.receiver_count() > 0)
            .map(|(connection_id, session)| {
                (
                    *connection_id,
                    SessionRecipient {
                        user_id: session.authenticated_user_id,
                        is_administrator: session.is_administrator,
                    },
                )
            })
            .collect()
    }

    async fn send_connection<T: serde::Serialize>(
        &self,
        connection_id: Uuid,
        message_type: &str,
        data: &T,
    ) {
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        let message = Arc::<str>::from(websocket_message(message_type, &data));
        if let Some(session) = self.sessions.read().await.get(&connection_id) {
            let _ = session.sender.send(message);
        }
    }

    pub(crate) async fn send_command(
        &self,
        session_id: &str,
        message_type: &str,
        data: &serde_json::Value,
    ) -> bool {
        let message = websocket_message(message_type, data);
        let sessions = self.sessions.read().await;
        let mut sent = false;
        for session in sessions
            .values()
            .filter(|session| session.session_id == session_id)
        {
            sent |= session.sender.send(Arc::from(message.clone())).is_ok();
        }
        sent
    }
}

fn websocket_message(message_type: &str, data: &serde_json::Value) -> String {
    json!({
        "MessageType": message_type,
        "MessageId": Uuid::new_v4().simple().to_string(),
        "Data": data
    })
    .to_string()
}

pub(crate) async fn connect(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let authenticated =
        authentication::authenticated_session_for_uri(&state, &headers, &uri).await?;
    let session_id = jellyfin_session_id(
        &authenticated.device.app_name,
        &authenticated.device.device_id,
    );
    let user_id = authenticated.user.id;
    let is_administrator = authenticated.user.is_administrator;
    let additional_user_ids = serde_json::from_value::<Vec<jellyfin_model::SessionUserInfo>>(
        authenticated.device.additional_users,
    )
    .unwrap_or_default()
    .into_iter()
    .map(|user| user.user_id)
    .collect::<Vec<_>>();

    Ok(upgrade.on_upgrade(move |socket| {
        serve(
            socket,
            state,
            session_id,
            user_id,
            is_administrator,
            additional_user_ids,
        )
    }))
}

async fn serve(
    mut socket: axum::extract::ws::WebSocket,
    state: Arc<AppState>,
    session_id: String,
    user_id: Uuid,
    is_administrator: bool,
    additional_user_ids: Vec<Uuid>,
) {
    let mut shutdown = state.web_sockets.subscribe_shutdown();
    let (connection_id, mut outbound) = state
        .web_sockets
        .subscribe(&session_id, user_id, is_administrator, additional_user_ids)
        .await;
    state.sync_play.websocket_connected(&session_id).await;
    if send_force_keep_alive(&mut socket).await.is_err() {
        drop(outbound);
        disconnect(&state, &session_id, connection_id).await;
        return;
    }
    if !drain_session_commands(&mut socket, &state, &session_id).await {
        drop(outbound);
        disconnect(&state, &session_id, connection_id).await;
        return;
    }
    broadcast_sessions(&state).await;
    broadcast_scheduled_tasks_info(&state).await;

    // Web clients treat the three `*Info` snapshots as drop-after-connection:
    // they always expect Sessions / ScheduledTasksInfo right away, so opt the
    // session into those subscriptions eagerly rather than waiting for a
    // `SessionsStart` text.
    let mut subscriptions: HashSet<String> = SUBSCRIPTION_MESSAGE_TYPES
        .iter()
        .map(|kind| (*kind).to_owned())
        .collect();

    let mut last_keep_alive = Instant::now();
    let mut forced = false;
    let mut watchdog = tokio::time::interval(WATCH_INTERVAL);
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    watchdog.tick().await;
    loop {
        tokio::select! {
            command = wait_for_shutdown(&mut shutdown) => {
                let message_type = match command {
                    Some(SystemCommand::Restart) => "ServerRestarting",
                    Some(SystemCommand::Shutdown) => "ServerShuttingDown",
                    None => break,
                };
                let message = websocket_message(message_type, &serde_json::Value::Null);
                let _ = socket.send(Message::Text(message.into())).await;
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            _ = watchdog.tick() => {
                let elapsed = last_keep_alive.elapsed();
                if elapsed >= LOST_TIMEOUT {
                    break;
                }
                if elapsed >= FORCE_KEEP_ALIVE_AFTER && !forced {
                    if send_force_keep_alive(&mut socket).await.is_err() {
                        break;
                    }
                    forced = true;
                }
            }
            message = socket.recv() => { match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(Message::Ping(payload))) => {
                    last_keep_alive = Instant::now();
                    forced = false;
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Pong(_))) => {
                    last_keep_alive = Instant::now();
                    forced = false;
                }
                Some(Ok(Message::Text(payload))) if is_keep_alive(payload.as_bytes()) => {
                    last_keep_alive = Instant::now();
                    forced = false;
                }
                Some(Ok(Message::Binary(payload))) if is_keep_alive(&payload) => {
                    last_keep_alive = Instant::now();
                    forced = false;
                }
                Some(Ok(Message::Text(payload))) => {
                    handle_inbound(payload.as_bytes(), &mut subscriptions, &state).await;
                }
                Some(Ok(Message::Binary(payload))) => {
                    handle_inbound(&payload, &mut subscriptions, &state).await;
                }
            } }
            message = outbound.recv() => { match message {
                Ok(message) => {
                    if should_deliver_message(&message, &subscriptions)
                        && socket
                            .send(Message::Text(message.as_ref().into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            } }
        }
    }
    drop(outbound);
    disconnect(&state, &session_id, connection_id).await;
}

async fn wait_for_shutdown(
    shutdown: &mut watch::Receiver<Option<SystemCommand>>,
) -> Option<SystemCommand> {
    shutdown
        .wait_for(Option::is_some)
        .await
        .ok()
        .and_then(|command| *command)
}

async fn drain_session_commands(
    socket: &mut axum::extract::ws::WebSocket,
    state: &AppState,
    session_id: &str,
) -> bool {
    let Ok(commands) = state.session_commands.list_for_session(session_id).await else {
        return true;
    };
    if commands.is_empty() {
        return true;
    }
    for command in commands {
        let message = websocket_message(&command.message_type, &command.payload);
        if socket.send(Message::Text(message.into())).await.is_err() {
            return false;
        }
        let _ = state.session_commands.delete(&[command.id]).await;
    }
    true
}

async fn disconnect(state: &AppState, session_id: &str, connection_id: Uuid) {
    if !state.web_sockets.unsubscribe(connection_id).await {
        return;
    }
    let Some(departure) = state
        .sync_play
        .websocket_disconnected_with_departure(session_id)
        .await
    else {
        return;
    };
    broadcast_sync_play_departure(state, departure).await;
}

pub(crate) async fn broadcast_sync_play_departure(state: &AppState, departure: SyncPlayDeparture) {
    if let Some((session_ids, command)) = departure.playback_command {
        state
            .web_sockets
            .send(&session_ids, "SyncPlayCommand", &command)
            .await;
    }
    if let Some(update) = departure.state_update {
        state
            .web_sockets
            .send(&update.session_ids, "SyncPlayGroupUpdate", &update.payload)
            .await;
    }
    for update in departure.membership_updates {
        state
            .web_sockets
            .send(&update.session_ids, "SyncPlayGroupUpdate", &update.payload)
            .await;
    }
}

pub(crate) async fn broadcast_sessions(state: &AppState) {
    if let Ok(sessions) = crate::session::all_session_infos(state).await {
        for (connection_id, recipient) in state.web_sockets.recipients().await {
            let visible = sessions
                .iter()
                .filter(|session| session_visible_to(session, recipient))
                .collect::<Vec<_>>();
            state
                .web_sockets
                .send_connection(connection_id, "Sessions", &visible)
                .await;
        }
    }
}

fn session_visible_to(
    session: &jellyfin_model::SessionInfoDto,
    recipient: SessionRecipient,
) -> bool {
    recipient.is_administrator
        || session.user_id == recipient.user_id
        || session
            .additional_users
            .iter()
            .any(|user| user.user_id == recipient.user_id)
}

pub(crate) async fn broadcast_scheduled_tasks_info(state: &AppState) {
    let tasks = state.scheduled_tasks.list(None, None).await;
    state
        .web_sockets
        .send_to_administrators("ScheduledTasksInfo", &tasks)
        .await;
}

pub(crate) async fn broadcast_library_changed(
    state: &AppState,
    added: &[Uuid],
    removed: &[Uuid],
    updated: &[Uuid],
) {
    let empty_change = added.is_empty() && removed.is_empty() && updated.is_empty();
    for (connection_id, recipient) in state.web_sockets.recipients().await {
        let (visible_added, visible_removed, visible_updated) = if recipient.is_administrator {
            (added.to_vec(), removed.to_vec(), updated.to_vec())
        } else {
            (
                visible_library_item_ids(state, recipient.user_id, added).await,
                // Once an item has been removed there is no reliable policy
                // context left with which to prove that it was visible. Do not
                // disclose its identifier to ordinary users.
                Vec::new(),
                visible_library_item_ids(state, recipient.user_id, updated).await,
            )
        };
        if !empty_change
            && visible_added.is_empty()
            && visible_removed.is_empty()
            && visible_updated.is_empty()
        {
            continue;
        }
        state
            .web_sockets
            .send_connection(
                connection_id,
                "LibraryChanged",
                &json!({
                    "ItemsAdded": guid_strings(&visible_added),
                    "ItemsRemoved": guid_strings(&visible_removed),
                    "ItemsUpdated": guid_strings(&visible_updated),
                }),
            )
            .await;
    }
}

async fn visible_library_item_ids(state: &AppState, user_id: Uuid, ids: &[Uuid]) -> Vec<Uuid> {
    let mut visible = Vec::with_capacity(ids.len());
    for item_id in ids {
        if state
            .user_data
            .visible_item(user_id, *item_id)
            .await
            .is_ok()
        {
            visible.push(*item_id);
        }
    }
    visible
}

fn guid_strings(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(|id| id.simple().to_string()).collect()
}

pub(crate) async fn broadcast_user_data_changed(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    user_data: &serde_json::Value,
) {
    state
        .web_sockets
        .send_to_users(
            &[user_id],
            "UserDataChanged",
            &json!({
                "UserId": user_id.simple().to_string(),
                "ItemId": item_id.simple().to_string(),
                "UserData": user_data,
            }),
        )
        .await;
}

pub(crate) async fn broadcast_user_updated(state: &AppState, user: &serde_json::Value) {
    let Some(user_id) = user
        .get("Id")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return;
    };
    let is_administrator = state
        .users
        .get(user_id)
        .await
        .is_ok_and(|user| user.is_administrator);
    state
        .web_sockets
        .set_user_administrator(user_id, is_administrator)
        .await;
    state
        .web_sockets
        .send_to_users(&[user_id], "UserUpdated", user)
        .await;
}

pub(crate) async fn broadcast_user_deleted(state: &AppState, user_id: Uuid) {
    state
        .web_sockets
        .set_user_administrator(user_id, false)
        .await;
    state
        .web_sockets
        .send_to_users(&[user_id], "UserDeleted", &user_deleted_data(user_id))
        .await;
}

fn user_deleted_data(user_id: Uuid) -> String {
    user_id.simple().to_string()
}

pub(crate) async fn broadcast_refresh_progress(state: &AppState, item_id: Uuid, progress: f64) {
    state
        .web_sockets
        .send_to_administrators(
            "RefreshProgress",
            &json!({
                "ItemId": item_id.simple().to_string(),
                "Progress": progress,
            }),
        )
        .await;
}

async fn send_force_keep_alive(
    socket: &mut axum::extract::ws::WebSocket,
) -> Result<(), axum::Error> {
    let message = json!({
        "MessageType": "ForceKeepAlive",
        "MessageId": "00000000000000000000000000000000",
        "Data": LOST_TIMEOUT.as_secs()
    });
    socket.send(Message::Text(message.to_string().into())).await
}

fn is_keep_alive(payload: &[u8]) -> bool {
    deserialize_websocket_message([payload])
        .is_ok_and(|parsed| parsed.message.message_type == WebSocketMessageType::KeepAlive)
}

const SUBSCRIPTION_MESSAGE_TYPES: &[&str] = &["Sessions", "ScheduledTasksInfo", "ActivityLogEntry"];

fn should_deliver_message(message: &str, subscriptions: &HashSet<String>) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(message) else {
        return true;
    };
    let Some(message_type) = value.get("MessageType").and_then(serde_json::Value::as_str) else {
        return true;
    };
    !SUBSCRIPTION_MESSAGE_TYPES
        .iter()
        .any(|subscribed| message_type.eq_ignore_ascii_case(subscribed))
        || subscriptions
            .iter()
            .any(|subscribed| message_type.eq_ignore_ascii_case(subscribed))
}

async fn handle_inbound(payload: &[u8], subscriptions: &mut HashSet<String>, state: &AppState) {
    let Ok(parsed) = deserialize_websocket_message([payload]) else {
        return;
    };
    match parsed.message.message_type {
        WebSocketMessageType::SessionsStart => {
            subscriptions.insert("Sessions".to_owned());
            broadcast_sessions(state).await;
        }
        WebSocketMessageType::SessionsStop => {
            subscriptions.remove("Sessions");
        }
        WebSocketMessageType::ScheduledTasksInfoStart => {
            subscriptions.insert("ScheduledTasksInfo".to_owned());
            broadcast_scheduled_tasks_info(state).await;
        }
        WebSocketMessageType::ScheduledTasksInfoStop => {
            subscriptions.remove("ScheduledTasksInfo");
        }
        WebSocketMessageType::ActivityLogEntryStart => {
            subscriptions.insert("ActivityLogEntry".to_owned());
        }
        WebSocketMessageType::ActivityLogEntryStop => {
            subscriptions.remove("ActivityLogEntry");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SessionRecipient, WebSocketHub, is_keep_alive, session_visible_to, user_deleted_data,
    };
    use crate::SystemCommand;
    use chrono::Utc;
    use jellyfin_model::{ClientCapabilitiesDto, PlayerStateInfo, SessionInfoDto, SessionUserInfo};
    use std::collections::HashSet;
    use tokio::time::{Duration, timeout};
    use uuid::Uuid;

    use super::should_deliver_message;

    #[test]
    fn only_jellyfin_keep_alive_messages_refresh_the_watchdog() {
        assert!(is_keep_alive(br#"{"MessageType":"KeepAlive"}"#));
        assert!(!is_keep_alive(br#"{"MessageType":"Sessions"}"#));
        assert!(!is_keep_alive(b"not json"));
    }

    #[test]
    fn subscription_gated_messages_respect_start_and_stop() {
        let session_message = r#"{"MessageType":"Sessions","Data":[]}"#;
        let mut subscriptions = HashSet::new();
        assert!(!should_deliver_message(session_message, &subscriptions));
        subscriptions.insert("Sessions".to_owned());
        assert!(should_deliver_message(session_message, &subscriptions));

        let library_message = r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":[]}}"#;
        assert!(should_deliver_message(library_message, &subscriptions));
    }

    #[test]
    fn user_deleted_data_is_a_compact_guid_string() {
        let user_id = Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();

        assert_eq!(
            serde_json::to_value(user_deleted_data(user_id)).unwrap(),
            serde_json::json!("0123456789abcdef0123456789abcdef")
        );
    }

    #[tokio::test]
    async fn private_user_messages_do_not_cross_user_or_admin_boundaries() {
        let hub = WebSocketHub::new();
        let first_user = Uuid::new_v4();
        let second_user = Uuid::new_v4();
        let administrator = Uuid::new_v4();
        let (_, mut first) = hub.subscribe("first", first_user, false, []).await;
        let (_, mut second) = hub.subscribe("second", second_user, false, []).await;
        let (_, mut admin) = hub.subscribe("admin", administrator, true, []).await;

        hub.send_to_users(
            &[first_user],
            "UserDataChanged",
            &serde_json::json!({ "UserId": first_user }),
        )
        .await;

        let delivered = first.recv().await.expect("target user must receive update");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&delivered).unwrap()["MessageType"],
            "UserDataChanged"
        );
        assert!(
            timeout(Duration::from_millis(25), second.recv())
                .await
                .is_err()
        );
        assert!(
            timeout(Duration::from_millis(25), admin.recv())
                .await
                .is_err()
        );

        hub.add_session_user("second", first_user).await;
        hub.send_to_users(&[first_user], "UserDataChanged", &serde_json::json!({}))
            .await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&second.recv().await.unwrap()).unwrap()["MessageType"],
            "UserDataChanged"
        );
        hub.remove_session_user("second", first_user).await;
        hub.send_to_users(&[first_user], "UserDataChanged", &serde_json::json!({}))
            .await;
        assert!(
            timeout(Duration::from_millis(25), second.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn session_commands_reach_each_socket_and_disconnect_only_after_the_last_one() {
        let hub = WebSocketHub::new();
        let user_id = Uuid::new_v4();
        let (first_connection, mut first) =
            hub.subscribe("shared-session", user_id, false, []).await;
        let (second_connection, mut second) =
            hub.subscribe("shared-session", user_id, false, []).await;

        assert!(
            hub.send_command(
                "shared-session",
                "GeneralCommand",
                &serde_json::json!({ "Name": "GoHome" }),
            )
            .await
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&first.recv().await.unwrap()).unwrap()["MessageType"],
            "GeneralCommand"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&second.recv().await.unwrap()).unwrap()["MessageType"],
            "GeneralCommand"
        );

        drop(first);
        assert!(!hub.unsubscribe(first_connection).await);
        assert!(hub.is_connected("shared-session").await);
        drop(second);
        assert!(hub.unsubscribe(second_connection).await);
        assert!(!hub.is_connected("shared-session").await);
    }

    #[tokio::test]
    async fn administrative_messages_are_only_delivered_to_administrators() {
        let hub = WebSocketHub::new();
        let (_, mut ordinary) = hub.subscribe("ordinary", Uuid::new_v4(), false, []).await;
        let administrator_id = Uuid::new_v4();
        let (_, mut administrator) = hub.subscribe("admin", administrator_id, true, []).await;

        hub.send_to_administrators("ScheduledTasksInfo", &serde_json::json!([]))
            .await;

        let delivered = administrator
            .recv()
            .await
            .expect("administrator must receive task information");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&delivered).unwrap()["MessageType"],
            "ScheduledTasksInfo"
        );
        assert!(
            timeout(Duration::from_millis(25), ordinary.recv())
                .await
                .is_err()
        );

        hub.set_user_administrator(administrator_id, false).await;
        hub.send_to_administrators("ScheduledTasksInfo", &serde_json::json!([]))
            .await;
        assert!(
            timeout(Duration::from_millis(25), administrator.recv())
                .await
                .is_err(),
            "demoted users must immediately lose administrator broadcasts"
        );
    }

    #[tokio::test]
    async fn shutdown_notification_reaches_all_connected_socket_tasks() {
        let hub = WebSocketHub::new();
        let mut first = hub.subscribe_shutdown();
        let mut second = hub.subscribe_shutdown();

        hub.notify_shutdown(SystemCommand::Restart);
        let mut late_subscriber = hub.subscribe_shutdown();

        assert_eq!(
            *first.wait_for(Option::is_some).await.unwrap(),
            Some(SystemCommand::Restart)
        );
        assert_eq!(
            *second.wait_for(Option::is_some).await.unwrap(),
            Some(SystemCommand::Restart)
        );
        assert_eq!(
            *late_subscriber.wait_for(Option::is_some).await.unwrap(),
            Some(SystemCommand::Restart)
        );
    }

    #[test]
    fn ordinary_session_snapshots_are_private_and_administrators_see_all() {
        let first_user = Uuid::new_v4();
        let second_user = Uuid::new_v4();
        let shared_user = Uuid::new_v4();
        let first_session = session_info(first_user, &[shared_user]);
        let second_session = session_info(second_user, &[]);
        let first_recipient = SessionRecipient {
            user_id: first_user,
            is_administrator: false,
        };
        let second_recipient = SessionRecipient {
            user_id: second_user,
            is_administrator: false,
        };
        let shared_recipient = SessionRecipient {
            user_id: shared_user,
            is_administrator: false,
        };
        let administrator = SessionRecipient {
            user_id: Uuid::new_v4(),
            is_administrator: true,
        };

        assert!(session_visible_to(&first_session, first_recipient));
        assert!(!session_visible_to(&second_session, first_recipient));
        assert!(session_visible_to(&second_session, second_recipient));
        assert!(!session_visible_to(&first_session, second_recipient));
        assert!(session_visible_to(&first_session, shared_recipient));
        assert!(session_visible_to(&first_session, administrator));
        assert!(session_visible_to(&second_session, administrator));
    }

    fn session_info(user_id: Uuid, additional_user_ids: &[Uuid]) -> SessionInfoDto {
        SessionInfoDto {
            play_state: PlayerStateInfo::default(),
            additional_users: additional_user_ids
                .iter()
                .map(|user_id| SessionUserInfo {
                    user_id: *user_id,
                    user_name: String::new(),
                })
                .collect(),
            capabilities: ClientCapabilitiesDto::default(),
            playable_media_types: Vec::new(),
            id: None,
            user_id,
            user_name: None,
            client: None,
            last_activity_date: Utc::now(),
            last_playback_check_in: Utc::now(),
            last_paused_date: None,
            device_name: None,
            device_type: None,
            now_playing_item: None,
            device_id: None,
            application_version: None,
            is_active: true,
            supports_media_control: false,
            supports_remote_control: false,
            now_playing_queue: Vec::new(),
            has_custom_device_name: false,
            playlist_item_id: None,
            server_id: None,
            user_primary_image_tag: None,
            now_viewing_item: None,
            supported_commands: Vec::new(),
        }
    }
}
