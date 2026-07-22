use std::io::{self, Read};

use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

/// A typed inbound Jellyfin WebSocket message.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundWebSocketMessage {
    pub message_type: WebSocketMessageType,
    pub message_id: Option<Uuid>,
    pub server_id: Option<String>,
    pub data: Option<Value>,
}

/// Session message types supported by Jellyfin's WebSocket protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketMessageType {
    ForceKeepAlive,
    GeneralCommand,
    UserDataChanged,
    Sessions,
    Play,
    SyncPlayCommand,
    SyncPlayGroupUpdate,
    Playstate,
    RestartRequired,
    ServerShuttingDown,
    ServerRestarting,
    LibraryChanged,
    UserDeleted,
    UserUpdated,
    SeriesTimerCreated,
    TimerCreated,
    SeriesTimerCancelled,
    TimerCancelled,
    RefreshProgress,
    ScheduledTaskEnded,
    PackageInstallationCancelled,
    PackageInstallationFailed,
    PackageInstallationCompleted,
    PackageInstalling,
    PackageUninstalled,
    ActivityLogEntry,
    ScheduledTasksInfo,
    ActivityLogEntryStart,
    ActivityLogEntryStop,
    SessionsStart,
    SessionsStop,
    ScheduledTasksInfoStart,
    ScheduledTasksInfoStop,
    KeepAlive,
}

impl WebSocketMessageType {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "ForceKeepAlive" => Self::ForceKeepAlive,
            "GeneralCommand" => Self::GeneralCommand,
            "UserDataChanged" => Self::UserDataChanged,
            "Sessions" => Self::Sessions,
            "Play" => Self::Play,
            "SyncPlayCommand" => Self::SyncPlayCommand,
            "SyncPlayGroupUpdate" => Self::SyncPlayGroupUpdate,
            "Playstate" => Self::Playstate,
            "RestartRequired" => Self::RestartRequired,
            "ServerShuttingDown" => Self::ServerShuttingDown,
            "ServerRestarting" => Self::ServerRestarting,
            "LibraryChanged" => Self::LibraryChanged,
            "UserDeleted" => Self::UserDeleted,
            "UserUpdated" => Self::UserUpdated,
            "SeriesTimerCreated" => Self::SeriesTimerCreated,
            "TimerCreated" => Self::TimerCreated,
            "SeriesTimerCancelled" => Self::SeriesTimerCancelled,
            "TimerCancelled" => Self::TimerCancelled,
            "RefreshProgress" => Self::RefreshProgress,
            "ScheduledTaskEnded" => Self::ScheduledTaskEnded,
            "PackageInstallationCancelled" => Self::PackageInstallationCancelled,
            "PackageInstallationFailed" => Self::PackageInstallationFailed,
            "PackageInstallationCompleted" => Self::PackageInstallationCompleted,
            "PackageInstalling" => Self::PackageInstalling,
            "PackageUninstalled" => Self::PackageUninstalled,
            "ActivityLogEntry" => Self::ActivityLogEntry,
            "ScheduledTasksInfo" => Self::ScheduledTasksInfo,
            "ActivityLogEntryStart" => Self::ActivityLogEntryStart,
            "ActivityLogEntryStop" => Self::ActivityLogEntryStop,
            "SessionsStart" => Self::SessionsStart,
            "SessionsStop" => Self::SessionsStop,
            "ScheduledTasksInfoStart" => Self::ScheduledTasksInfoStart,
            "ScheduledTasksInfoStop" => Self::ScheduledTasksInfoStop,
            "KeepAlive" => Self::KeepAlive,
            _ => return None,
        })
    }
}

/// One parsed WebSocket message and its exact byte extent in the input.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedWebSocketMessage {
    pub message: InboundWebSocketMessage,
    pub bytes_consumed: usize,
}

/// Errors produced while parsing a segmented WebSocket JSON message.
#[derive(Debug, Error)]
pub enum WebSocketJsonError {
    #[error("invalid WebSocket JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid WebSocket message: {0}")]
    Message(String),
}

/// Parses the first WebSocket JSON object from one or more byte segments.
///
/// Trailing bytes are left unparsed, including an incomplete following JSON
/// object. `bytes_consumed` ends at the first complete JSON value.
///
/// # Errors
///
/// Returns [`WebSocketJsonError::Json`] when the first JSON value is malformed
/// or incomplete, and [`WebSocketJsonError::Message`] when its fields do not
/// form a valid Jellyfin WebSocket message.
pub fn deserialize_websocket_message<I, B>(
    segments: I,
) -> Result<ParsedWebSocketMessage, WebSocketJsonError>
where
    I: IntoIterator<Item = B>,
    B: AsRef<[u8]>,
{
    let reader = SegmentReader::new(segments.into_iter());
    let mut values = serde_json::Deserializer::from_reader(reader).into_iter::<Value>();
    let value = match values.next() {
        Some(result) => result?,
        None => {
            return Err(WebSocketJsonError::Json(serde_json::Error::io(
                io::Error::new(io::ErrorKind::UnexpectedEof, "expected a JSON value"),
            )));
        }
    };
    let bytes_consumed = values.byte_offset();
    let message = message_from_value(value)?;

    Ok(ParsedWebSocketMessage {
        message,
        bytes_consumed,
    })
}

fn message_from_value(value: Value) -> Result<InboundWebSocketMessage, WebSocketJsonError> {
    let Value::Object(mut object) = value else {
        return Err(WebSocketJsonError::Message(
            "top-level JSON value must be an object".to_owned(),
        ));
    };

    let message_type = object
        .remove("MessageType")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| WebSocketJsonError::Message("MessageType must be a string".to_owned()))?;
    let message_type = WebSocketMessageType::parse(&message_type).ok_or_else(|| {
        WebSocketJsonError::Message(format!("unknown MessageType `{message_type}`"))
    })?;
    let message_id = optional_uuid(&mut object, "MessageId")?;
    let server_id = optional_string(&mut object, "ServerId")?;
    let data = object.remove("Data");

    Ok(InboundWebSocketMessage {
        message_type,
        message_id,
        server_id,
        data,
    })
}

fn optional_uuid(
    object: &mut serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<Uuid>, WebSocketJsonError> {
    match object.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Uuid::parse_str(&value).map(Some).map_err(|error| {
            WebSocketJsonError::Message(format!("{field} is not a UUID: {error}"))
        }),
        Some(_) => Err(WebSocketJsonError::Message(format!(
            "{field} must be a string or null"
        ))),
    }
}

fn optional_string(
    object: &mut serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, WebSocketJsonError> {
    match object.remove(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(WebSocketJsonError::Message(format!(
            "{field} must be a string or null"
        ))),
    }
}

struct SegmentReader<I>
where
    I: Iterator,
{
    segments: I,
    current: Option<I::Item>,
    offset: usize,
}

impl<I> SegmentReader<I>
where
    I: Iterator,
{
    fn new(segments: I) -> Self {
        Self {
            segments,
            current: None,
            offset: 0,
        }
    }
}

impl<I, B> Read for SegmentReader<I>
where
    I: Iterator<Item = B>,
    B: AsRef<[u8]>,
{
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < output.len() {
            let exhausted = self
                .current
                .as_ref()
                .is_none_or(|segment| self.offset >= segment.as_ref().len());
            if exhausted {
                self.current = self.segments.next();
                self.offset = 0;
                if self.current.is_none() {
                    break;
                }
                continue;
            }

            let segment = self
                .current
                .as_ref()
                .expect("segment was checked above")
                .as_ref();
            let available = &segment[self.offset..];
            let count = available.len().min(output.len() - written);
            output[written..written + count].copy_from_slice(&available[..count]);
            self.offset += count;
            written += count;
        }
        Ok(written)
    }
}
