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
    sync::{RwLock, broadcast},
    time::{Duration, Instant, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{ApiError, AppState, authentication, session::jellyfin_session_id};

const LOST_TIMEOUT: Duration = Duration::from_secs(60);
const FORCE_KEEP_ALIVE_AFTER: Duration = Duration::from_secs(45);
const WATCH_INTERVAL: Duration = Duration::from_secs(12);

#[derive(Debug, Default)]
pub(crate) struct WebSocketHub {
    sessions: RwLock<HashMap<String, broadcast::Sender<String>>>,
}

impl WebSocketHub {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    async fn subscribe(&self, session_id: &str) -> broadcast::Receiver<String> {
        let mut sessions = self.sessions.write().await;
        sessions
            .entry(session_id.to_owned())
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe()
    }

    async fn unsubscribe(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        if sessions
            .get(session_id)
            .is_some_and(|sender| sender.receiver_count() == 0)
        {
            sessions.remove(session_id);
        }
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
        let message = websocket_message(message_type, &data);
        let sessions = self.sessions.read().await;
        for session_id in session_ids {
            if let Some(sender) = sessions.get(session_id) {
                let _ = sender.send(message.clone());
            }
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
        let Some(sender) = sessions.get(session_id) else {
            return false;
        };
        if sender.receiver_count() == 0 {
            return false;
        }
        sender.send(message).is_ok()
    }

    pub(crate) async fn send_all<T: serde::Serialize>(
        &self,
        message_type: &str,
        data: &T,
    ) {
        let Ok(data) = serde_json::to_value(data) else {
            return;
        };
        let message = websocket_message(message_type, &data);
        let sessions = self.sessions.read().await;
        for sender in sessions.values() {
            let _ = sender.send(message.clone());
        }
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

    Ok(upgrade.on_upgrade(move |socket| serve(socket, state, session_id)))
}

async fn serve(mut socket: axum::extract::ws::WebSocket, state: Arc<AppState>, session_id: String) {
    let mut outbound = state.web_sockets.subscribe(&session_id).await;
    state.sync_play.websocket_connected(&session_id).await;
    if send_force_keep_alive(&mut socket).await.is_err() {
        drop(outbound);
        disconnect(&state, &session_id).await;
        return;
    }
    if !drain_session_commands(&mut socket, &state, &session_id).await {
        drop(outbound);
        disconnect(&state, &session_id).await;
        return;
    }
    broadcast_sessions(&state).await;
    broadcast_scheduled_tasks_info(&state).await;

    let mut last_keep_alive = Instant::now();
    let mut forced = false;
    let mut subscriptions = HashSet::new();
    let mut watchdog = tokio::time::interval(WATCH_INTERVAL);
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    watchdog.tick().await;
    loop {
        tokio::select! {
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
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
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
                        && socket.send(Message::Text(message.into())).await.is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            } }
        }
    }
    drop(outbound);
    disconnect(&state, &session_id).await;
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

async fn disconnect(state: &AppState, session_id: &str) {
    state.web_sockets.unsubscribe(&session_id).await;
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
        state.web_sockets.send_all("Sessions", &sessions).await;
    }
}

pub(crate) async fn broadcast_scheduled_tasks_info(state: &AppState) {
    let tasks = state.scheduled_tasks.list(None, None).await;
    state
        .web_sockets
        .send_all("ScheduledTasksInfo", &tasks)
        .await;
}

pub(crate) async fn broadcast_library_changed(
    state: &AppState,
    added: &[Uuid],
    removed: &[Uuid],
    updated: &[Uuid],
) {
    let strings = |ids: &[Uuid]| {
        ids.iter()
            .map(|id| id.simple().to_string())
            .collect::<Vec<_>>()
    };
    state
        .web_sockets
        .send_all(
            "LibraryChanged",
            &json!({
                "ItemsAdded": strings(added),
                "ItemsRemoved": strings(removed),
                "ItemsUpdated": strings(updated),
            }),
        )
        .await;
}

pub(crate) async fn broadcast_user_data_changed(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    user_data: &serde_json::Value,
) {
    state
        .web_sockets
        .send_all(
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
    state
        .web_sockets
        .send_all("UserUpdated", user)
        .await;
}

pub(crate) async fn broadcast_user_deleted(state: &AppState, user_id: Uuid) {
    state
        .web_sockets
        .send_all(
            "UserDeleted",
            &json!({ "Id": user_id.simple().to_string() }),
        )
        .await;
}

pub(crate) async fn broadcast_refresh_progress(state: &AppState, item_id: Uuid, progress: f64) {
    state
        .web_sockets
        .send_all(
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

async fn handle_inbound(
    payload: &[u8],
    subscriptions: &mut HashSet<String>,
    state: &AppState,
) {
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
    use super::is_keep_alive;
    use std::collections::HashSet;

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
}
