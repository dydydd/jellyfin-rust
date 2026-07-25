use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{OriginalUri, State, WebSocketUpgrade, ws::Message},
    http::HeaderMap,
    response::Response,
};
use jellyfin_server_implementations::{WebSocketMessageType, deserialize_websocket_message};
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
        let message = json!({
            "MessageType": message_type,
            "MessageId": Uuid::new_v4().simple().to_string(),
            "Data": data
        })
        .to_string();
        let sessions = self.sessions.read().await;
        for session_id in session_ids {
            if let Some(sender) = sessions.get(session_id) {
                let _ = sender.send(message.clone());
            }
        }
    }
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
        state.web_sockets.unsubscribe(&session_id).await;
        state.sync_play.websocket_disconnected(&session_id).await;
        return;
    }

    let mut last_keep_alive = Instant::now();
    let mut forced = false;
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
                Some(Ok(Message::Text(_) | Message::Binary(_))) => {}
            } }
            message = outbound.recv() => { match message {
                Ok(message) => {
                    if socket.send(Message::Text(message.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            } }
        }
    }
    drop(outbound);
    state.web_sockets.unsubscribe(&session_id).await;
    state.sync_play.websocket_disconnected(&session_id).await;
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

#[cfg(test)]
mod tests {
    use super::is_keep_alive;

    #[test]
    fn only_jellyfin_keep_alive_messages_refresh_the_watchdog() {
        assert!(is_keep_alive(br#"{"MessageType":"KeepAlive"}"#));
        assert!(!is_keep_alive(br#"{"MessageType":"Sessions"}"#));
        assert!(!is_keep_alive(b"not json"));
    }
}
