use jellyfin_server_implementations::{
    WebSocketJsonError, WebSocketMessageType, deserialize_websocket_message,
};
use serde_json::json;
use uuid::Uuid;

const FORCE_KEEP_ALIVE: &[u8] = include_bytes!(
    "../../../jellyfin/tests/Jellyfin.Server.Implementations.Tests/Test Data/HttpServer/ForceKeepAlive.json"
);
const VALID_PARTIAL: &[u8] = include_bytes!(
    "../../../jellyfin/tests/Jellyfin.Server.Implementations.Tests/Test Data/HttpServer/ValidPartial.json"
);
const PARTIAL: &[u8] = include_bytes!(
    "../../../jellyfin/tests/Jellyfin.Server.Implementations.Tests/Test Data/HttpServer/Partial.json"
);

#[test]
fn official_single_segment_consumes_first_109_bytes() {
    let parsed = deserialize_websocket_message([FORCE_KEEP_ALIVE]).unwrap();

    assert_force_keep_alive(&parsed.message);
    assert_eq!(parsed.bytes_consumed, 109);
}

#[test]
fn official_multiple_segments_consumes_first_109_bytes() {
    let bytes_without_newline = &FORCE_KEEP_ALIVE[..FORCE_KEEP_ALIVE.len() - 1];
    let parsed =
        deserialize_websocket_message([&bytes_without_newline[..64], &bytes_without_newline[64..]])
            .unwrap();

    assert_force_keep_alive(&parsed.message);
    assert_eq!(parsed.bytes_consumed, 109);
}

#[test]
fn official_complete_message_followed_by_partial_consumes_only_first_109_bytes() {
    let parsed = deserialize_websocket_message([
        &VALID_PARTIAL[..1],
        &VALID_PARTIAL[1..64],
        &VALID_PARTIAL[64..109],
        &VALID_PARTIAL[109..],
    ])
    .unwrap();

    assert_force_keep_alive(&parsed.message);
    assert_eq!(parsed.bytes_consumed, 109);
}

#[test]
fn official_incomplete_first_message_returns_typed_json_error() {
    let error = deserialize_websocket_message([&PARTIAL[..17], &PARTIAL[17..]]).unwrap_err();

    assert!(matches!(error, WebSocketJsonError::Json(_)), "{error:?}");
}

#[test]
fn malformed_first_message_returns_typed_json_error() {
    let error = deserialize_websocket_message([br#"{"MessageType":]"#]).unwrap_err();

    assert!(matches!(error, WebSocketJsonError::Json(ref error) if !error.is_eof()));
}

#[test]
fn segment_boundary_inside_utf8_codepoint_is_supported() {
    let bytes = r#"{"MessageType":"KeepAlive","Data":"你好"}"#.as_bytes();
    let split = bytes
        .windows(3)
        .position(|window| window == "你".as_bytes())
        .unwrap()
        + 1;

    let parsed = deserialize_websocket_message([&bytes[..split], &bytes[split..]]).unwrap();

    assert_eq!(parsed.message.message_type, WebSocketMessageType::KeepAlive);
    assert_eq!(parsed.message.data, Some(json!("你好")));
    assert_eq!(parsed.bytes_consumed, bytes.len());
}

fn assert_force_keep_alive(message: &jellyfin_server_implementations::InboundWebSocketMessage) {
    assert_eq!(message.message_type, WebSocketMessageType::ForceKeepAlive);
    assert_eq!(message.message_id, Some(Uuid::nil()));
    assert_eq!(message.server_id, None);
    assert_eq!(message.data, Some(json!(60)));
}
