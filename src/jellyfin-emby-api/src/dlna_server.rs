//! Emby's protocol-local UPnP/DLNA HTTP server surface.
//!
//! Emby 4.10 exposes this service without HTTP authentication. Its controller
//! does not compare the route `UuId` with the configured server id: when the
//! DLNA server is enabled, it carries that value into the device description
//! and otherwise ignores it. Keeping that slightly surprising behavior is
//! important because SSDP clients may preserve an older spelling of the UDN.
//!
//! This first server-transport stage deliberately returns an empty DIDL page
//! for `Browse`, `Search`, and `X_BrowseByLetter`. Real media browsing remains
//! a separate compatibility gap until `AppState` exposes a DLNA-specific,
//! profile-aware and policy-filtered item projection; using the user-less
//! Jellyfin query here would leak library contents from an unauthenticated
//! UPnP route.

use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Datelike, Timelike, Utc};
use jellyfin_api::AppState;

const XML_CONTENT_TYPE: &str = "text/xml; charset=UTF-8";
const MAX_SOAP_BODY_BYTES: usize = 1024 * 1024;
const CACHE_CONTROL: &str = "public, max-age=31536000";
const DEFAULT_PROTOCOL_INFO: &str = "http-get:*:video/mpeg:*,http-get:*:video/mp4:*,http-get:*:video/vnd.dlna.mpeg-tts:*,http-get:*:video/avi:*,http-get:*:video/x-matroska:*,http-get:*:video/x-ms-wmv:*,http-get:*:video/wtv:*,http-get:*:audio/mpeg:*,http-get:*:audio/mp3:*,http-get:*:audio/mp4:*,http-get:*:audio/x-ms-wma*,http-get:*:audio/wav:*,http-get:*:audio/L16:*,http-get:*image/jpeg:*,http-get:*image/png:*,http-get:*image/gif:*,http-get:*image/tiff:*";

// Small, bounded, generated server icons. They intentionally are not loaded
// from the web directory: a missing or replaced dashboard asset must not make
// UPnP discovery perform filesystem I/O or grow an unbounded response buffer.
const LOGO_48_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAADAAAAAwCAIAAADYYG7QAAAACXBIWXMAAAABAAAAAQBPJcTWAAAAS0lEQVR4nO3OMRHAIAAAMbirfxHYwgx7lx9hSBRkjrXHS77bgT+hIlSEilARKkJFqAgVoSJUhIpQESpCRagIFaEiVISKUBEqQkWoHKeJAjtCCVR1AAAAAElFTkSuQmCC";
const LOGO_120_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAHgAAAB4CAIAAAC2BqGFAAAACXBIWXMAAAABAAAAAQBPJcTWAAABIUlEQVR4nO3QQQ3AIADAQEjmXwS2MIOKlQd3CprOsfbgf9/tgFcYHTE6YnTE6IjREaMjRkeMjhgdMTpidMToiNERoyNGR4yOGB0xOmJ0xOiI0RGjI0ZHjI4YHTE6YnTE6IjREaMjRkeMjhgdMTpidMToiNERoyNGR4yOGB0xOmJ0xOiI0RGjI0ZHjI4YHTE6YnTE6IjREaMjRkeMjhgdMTpidMToiNERoyNGR4yOGB0xOmJ0xOiI0RGjI0ZHjI4YHTE6YnTE6IjREaMjRkeMjhgdMTpidMToiNERoyNGR4yOGB0xOmJ0xOiI0RGjI0ZHjI4YHTE6YnTE6IjREaMjRkeMjhgdMTpidMToiNERoyNGR4yOGB0xOmJ0xOiI0RGjIweRCQNbmm5TFQAAAABJRU5ErkJggg==";
const LOGO_240_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAPAAAADwCAIAAACxN37FAAAACXBIWXMAAAABAAAAAQBPJcTWAAACl0lEQVR4nO3SQQkAIADAQAX7h7CWZSwhCOMuwR6bY58BFet3ALxkaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkGJoUQ5NiaFIMTYqhSTE0KYYmxdCkXN+KBTtJkwi7AAAAAElFTkSuQmCC";
const LOGO_48_JPG: &str = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAxAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAE0AAQEAAAAAAAAAAAAAAAAAAAADAQEBAQAAAAAAAAAAAAAAAAAABgcQAQAAAAAAAAAAAAAAAAAAAAARAQAAAAAAAAAAAAAAAAAAAAD/wAARCAAwADADASIAAhEAAxEA/9oADAMBAAIRAxEAPwCAC3ZQAAAAAAAAAAAAAAAA/9k=";
const LOGO_120_JPG: &str = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAxAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAE0AAQEAAAAAAAAAAAAAAAAAAAADAQEBAQAAAAAAAAAAAAAAAAAABgcQAQAAAAAAAAAAAAAAAAAAAAARAQAAAAAAAAAAAAAAAAAAAAD/wAARCAB4AHgDASIAAhEAAxEA/9oADAMBAAIRAxEAPwCAC3ZQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA//2Q==";
const LOGO_240_JPG: &str = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAPTGF2YzYzLjEuMTAxAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAE0AAQEAAAAAAAAAAAAAAAAAAAADAQEBAQAAAAAAAAAAAAAAAAAABgcQAQAAAAAAAAAAAAAAAAAAAAARAQAAAAAAAAAAAAAAAAAAAAD/wAARCADwAPADASIAAhEAAxEA/9oADAMBAAIRAxEAPwCAC3ZQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/9k=";

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/Dlna/{uuid}/description.xml",
            get(description).head(xml_head),
        )
        .route("/Dlna/{uuid}/description", get(description).head(xml_head))
        .route(
            "/Dlna/{uuid}/contentdirectory/contentdirectory.xml",
            get(content_directory_description).head(xml_head),
        )
        .route(
            "/Dlna/{uuid}/contentdirectory/contentdirectory",
            get(content_directory_description).head(xml_head),
        )
        .route(
            "/Dlna/{uuid}/connectionmanager/connectionmanager.xml",
            get(connection_manager_description).head(xml_head),
        )
        .route(
            "/Dlna/{uuid}/connectionmanager/connectionmanager",
            get(connection_manager_description).head(xml_head),
        )
        .route(
            "/Dlna/{uuid}/contentdirectory/control",
            post(content_directory_control),
        )
        .route(
            "/Dlna/{uuid}/connectionmanager/control",
            post(connection_manager_control),
        )
        .route("/Dlna/icons/{filename}", get(icon_without_uuid))
        .route("/Dlna/{uuid}/icons/{filename}", get(icon_with_uuid))
        .route(
            "/dlna/{uuid}/description.xml",
            get(description).head(xml_head),
        )
        .route("/dlna/{uuid}/description", get(description).head(xml_head))
        .route(
            "/dlna/{uuid}/contentdirectory/contentdirectory.xml",
            get(content_directory_description).head(xml_head),
        )
        .route(
            "/dlna/{uuid}/contentdirectory/contentdirectory",
            get(content_directory_description).head(xml_head),
        )
        .route(
            "/dlna/{uuid}/connectionmanager/connectionmanager.xml",
            get(connection_manager_description).head(xml_head),
        )
        .route(
            "/dlna/{uuid}/connectionmanager/connectionmanager",
            get(connection_manager_description).head(xml_head),
        )
        .route(
            "/dlna/{uuid}/contentdirectory/control",
            post(content_directory_control),
        )
        .route(
            "/dlna/{uuid}/connectionmanager/control",
            post(connection_manager_control),
        )
        .route("/dlna/icons/{filename}", get(icon_without_uuid))
        .route("/dlna/{uuid}/icons/{filename}", get(icon_with_uuid))
}

async fn description(
    State(state): State<Arc<AppState>>,
    Path(uuid): Path<String>,
) -> Result<Response, StatusCode> {
    let info = state.public_system_info().await?;
    let server_name = info.server_name.as_deref().unwrap_or("Jellyfin");
    let presentation_url = info.local_address.as_deref().unwrap_or("/emby");
    Ok(xml_response(device_description(
        &uuid,
        server_name,
        presentation_url,
    )))
}

async fn content_directory_description(Path(_uuid): Path<String>) -> Response {
    xml_response(service_description(
        CONTENT_DIRECTORY_ACTIONS,
        CONTENT_DIRECTORY_STATE_VARIABLES,
    ))
}

async fn connection_manager_description(Path(_uuid): Path<String>) -> Response {
    xml_response(service_description(
        CONNECTION_MANAGER_ACTIONS,
        CONNECTION_MANAGER_STATE_VARIABLES,
    ))
}

async fn xml_head(Path(_uuid): Path<String>) -> Response {
    xml_response(String::new())
}

async fn content_directory_control(Path(_uuid): Path<String>, body: Body) -> Response {
    process_control(body, UpnpService::ContentDirectory).await
}

async fn connection_manager_control(Path(_uuid): Path<String>, body: Body) -> Response {
    process_control(body, UpnpService::ConnectionManager).await
}

async fn process_control(body: Body, service: UpnpService) -> Response {
    let Ok(body) = to_bytes(body, MAX_SOAP_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Some(action) = soap_action(&body) else {
        return soap_response(soap_fault());
    };

    let values = match service {
        UpnpService::ConnectionManager => connection_manager_result(action),
        UpnpService::ContentDirectory => content_directory_result(action),
    };
    let xml = values.map_or_else(soap_fault, |values| {
        soap_success(action, service.namespace(), &values)
    });
    soap_response(xml)
}

async fn icon_without_uuid(Path(filename): Path<String>) -> Response {
    icon_response(&filename)
}

async fn icon_with_uuid(Path((_uuid, filename)): Path<(String, String)>) -> Response {
    icon_response(&filename)
}

fn icon_response(filename: &str) -> Response {
    let Some((encoded, content_type)) = icon_resource(filename) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(bytes) = STANDARD.decode(encoded) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, CACHE_CONTROL)
        .body(Body::from(bytes))
        .expect("static DLNA icon response headers are valid")
}

fn icon_resource(filename: &str) -> Option<(&'static str, &'static str)> {
    if filename.eq_ignore_ascii_case("logo48.png") {
        Some((LOGO_48_PNG, "image/png"))
    } else if filename.eq_ignore_ascii_case("logo120.png") {
        Some((LOGO_120_PNG, "image/png"))
    } else if filename.eq_ignore_ascii_case("logo240.png") {
        Some((LOGO_240_PNG, "image/png"))
    } else if filename.eq_ignore_ascii_case("logo48.jpg") {
        Some((LOGO_48_JPG, "image/jpg"))
    } else if filename.eq_ignore_ascii_case("logo120.jpg") {
        Some((LOGO_120_JPG, "image/jpg"))
    } else if filename.eq_ignore_ascii_case("logo240.jpg") {
        Some((LOGO_240_JPG, "image/jpg"))
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum UpnpService {
    ContentDirectory,
    ConnectionManager,
}

impl UpnpService {
    const fn namespace(self) -> &'static str {
        match self {
            Self::ContentDirectory => "urn:schemas-upnp-org:service:ContentDirectory:1",
            Self::ConnectionManager => "urn:schemas-upnp-org:service:ConnectionManager:1",
        }
    }
}

fn connection_manager_result(action: &str) -> Option<Vec<(&'static str, String)>> {
    if action.eq_ignore_ascii_case("GetProtocolInfo") {
        Some(vec![
            ("Source", DEFAULT_PROTOCOL_INFO.to_owned()),
            ("Sink", String::new()),
        ])
    } else if action.eq_ignore_ascii_case("GetCurrentConnectionIDs") {
        Some(vec![("ConnectionIDs", String::new())])
    } else {
        // Emby advertises GetCurrentConnectionInfo but its 4.10 handler does
        // not implement it; it follows the same Invalid Action path.
        None
    }
}

fn content_directory_result(action: &str) -> Option<Vec<(&'static str, String)>> {
    if action.eq_ignore_ascii_case("GetSearchCapabilities") {
        Some(vec![(
            "SearchCaps",
            "dc:title,dc:creator,upnp:artist,upnp:genre,upnp:album,upnp:class".to_owned(),
        )])
    } else if action.eq_ignore_ascii_case("GetSortCapabilities")
        || action.eq_ignore_ascii_case("GetSortExtensionCapabilities")
    {
        Some(vec![(
            if action.eq_ignore_ascii_case("GetSortCapabilities") {
                "SortCaps"
            } else {
                "SortExtensionCaps"
            },
            "res@duration,dc:date,dc:title,upnp:album,upnp:artist,upnp:albumArtist,upnp:episodeNumber,upnp:originalTrackNumber,upnp:rating".to_owned(),
        )])
    } else if action.eq_ignore_ascii_case("GetSystemUpdateID") {
        let now = Utc::now();
        Some(vec![(
            "Id",
            (now.year()
                + i32::try_from(now.ordinal()).unwrap_or_default()
                + i32::try_from(now.hour()).unwrap_or_default())
            .to_string(),
        )])
    } else if action.eq_ignore_ascii_case("X_GetFeatureList")
        || action.eq_ignore_ascii_case("GetFeatureList")
    {
        Some(vec![("FeatureList", feature_list().to_owned())])
    } else if action.eq_ignore_ascii_case("Browse")
        || action.eq_ignore_ascii_case("Search")
        || action.eq_ignore_ascii_case("X_BrowseByLetter")
    {
        // The Rust server has no DLNA-specific user/profile selection layer
        // yet. Return a valid, bounded empty directory rather than exposing a
        // user's unfiltered library or manufacturing JSON in a SOAP response.
        Some(vec![
            ("Result", empty_didl().to_owned()),
            ("NumberReturned", "0".to_owned()),
            ("TotalMatches", "0".to_owned()),
            ("UpdateID", "0".to_owned()),
        ])
    } else {
        None
    }
}

fn soap_action(body: &[u8]) -> Option<&str> {
    let xml = std::str::from_utf8(body).ok()?;
    if contains_ascii_case_insensitive(xml, "<!doctype") {
        return None;
    }

    let mut cursor = 0;
    while let Some((name, next)) = next_start_tag(xml, cursor) {
        cursor = next;
        if local_name(name).eq_ignore_ascii_case("Body") {
            let (action, _) = next_start_tag(xml, cursor)?;
            return Some(local_name(action));
        }
    }
    None
}

fn next_start_tag(xml: &str, mut cursor: usize) -> Option<(&str, usize)> {
    let bytes = xml.as_bytes();
    loop {
        let relative = xml.get(cursor..)?.find('<')?;
        let start = cursor + relative;
        let marker = *bytes.get(start + 1)?;
        if xml.get(start..)?.starts_with("<!--") {
            cursor = start + xml.get(start..)?.find("-->")? + 3;
            continue;
        }
        if xml.get(start..)?.starts_with("<![CDATA[") {
            cursor = start + xml.get(start..)?.find("]]>")? + 3;
            continue;
        }
        if marker == b'?' {
            cursor = start + xml.get(start..)?.find("?>")? + 2;
            continue;
        }
        if matches!(marker, b'/' | b'!') {
            cursor = start + xml.get(start..)?.find('>')? + 1;
            continue;
        }
        let mut end = start + 1;
        while let Some(byte) = bytes.get(end) {
            if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
                break;
            }
            end += 1;
        }
        if end == start + 1 {
            return None;
        }
        let close = end + xml.get(end..)?.find('>')? + 1;
        return Some((xml.get(start + 1..end)?, close));
    }
}

fn local_name(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, local)| local)
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn soap_success(action: &str, namespace: &str, values: &[(&str, String)]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:",
    );
    push_xml_name(&mut xml, action);
    xml.push_str("Response xmlns:u=\"");
    push_xml_text(&mut xml, namespace);
    xml.push_str("\">");
    for (name, value) in values {
        xml.push('<');
        push_xml_name(&mut xml, name);
        xml.push('>');
        push_xml_text(&mut xml, value);
        xml.push_str("</");
        push_xml_name(&mut xml, name);
        xml.push('>');
    }
    xml.push_str("</u:");
    push_xml_name(&mut xml, action);
    xml.push_str("Response></s:Body></s:Envelope>");
    xml
}

fn soap_fault() -> String {
    "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>401</errorCode><errorDescription>Invalid Action</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>".to_owned()
}

fn soap_response(xml: String) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, XML_CONTENT_TYPE)
        .header("EXT", HeaderValue::from_static(""))
        .body(Body::from(xml))
        .expect("static SOAP response headers are valid")
}

fn xml_response(xml: String) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, XML_CONTENT_TYPE)
        .body(Body::from(xml))
        .expect("static XML response headers are valid")
}

fn device_description(uuid: &str, server_name: &str, presentation_url: &str) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><root xmlns=\"urn:schemas-upnp-org:device-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><device><UDN>uuid:",
    );
    push_xml_text(&mut xml, uuid);
    xml.push_str("</UDN><friendlyName>");
    push_xml_text(&mut xml, server_name);
    xml.push_str("</friendlyName><deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType><manufacturer>Emby</manufacturer><manufacturerURL>https://emby.media</manufacturerURL><modelName>Windows Media Player Sharing</modelName><modelNumber>12.0</modelNumber><modelURL>https://emby.media</modelURL><serialNumber></serialNumber><dlna:X_DLNADOC xmlns:dlna=\"urn:schemas-dlna-org:device-1-0\">DMS-1.50</dlna:X_DLNADOC><iconList>");
    for (mime, size, name) in [
        ("image/png", 240, "logo240.png"),
        ("image/jpeg", 240, "logo240.jpg"),
        ("image/png", 120, "logo120.png"),
        ("image/jpeg", 120, "logo120.jpg"),
        ("image/png", 48, "logo48.png"),
        ("image/jpeg", 48, "logo48.jpg"),
    ] {
        xml.push_str("<icon><mimetype>");
        xml.push_str(mime);
        xml.push_str("</mimetype><width>");
        xml.push_str(&size.to_string());
        xml.push_str("</width><height>");
        xml.push_str(&size.to_string());
        xml.push_str("</height><depth>24</depth><url>/emby/dlna/");
        push_xml_text(&mut xml, uuid);
        xml.push_str("/icons/");
        xml.push_str(name);
        xml.push_str("</url></icon>");
    }
    xml.push_str("</iconList><presentationURL>");
    push_xml_text(&mut xml, presentation_url);
    xml.push_str("</presentationURL><serviceList>");
    for (kind, service_id) in [
        ("ContentDirectory", "ContentDirectory"),
        ("ConnectionManager", "ConnectionManager"),
    ] {
        let path = kind.to_ascii_lowercase();
        xml.push_str("<service><serviceType>urn:schemas-upnp-org:service:");
        xml.push_str(kind);
        xml.push_str(":1</serviceType><serviceId>urn:upnp-org:serviceId:");
        xml.push_str(service_id);
        xml.push_str("</serviceId><SCPDURL>/emby/dlna/");
        push_xml_text(&mut xml, uuid);
        xml.push('/');
        xml.push_str(&path);
        xml.push('/');
        xml.push_str(&path);
        xml.push_str(".xml</SCPDURL><controlURL>/emby/dlna/");
        push_xml_text(&mut xml, uuid);
        xml.push('/');
        xml.push_str(&path);
        xml.push_str("/control</controlURL><eventSubURL>/emby/dlna/");
        push_xml_text(&mut xml, uuid);
        xml.push('/');
        xml.push_str(&path);
        xml.push_str("/events</eventSubURL></service>");
    }
    xml.push_str("</serviceList></device></root>");
    xml
}

struct ActionDescription {
    name: &'static str,
    arguments: &'static [(&'static str, &'static str, &'static str)],
}

struct StateVariableDescription {
    name: &'static str,
    data_type: &'static str,
    sends_events: bool,
    allowed_values: &'static [&'static str],
}

fn service_description(
    actions: &[ActionDescription],
    variables: &[StateVariableDescription],
) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\"?><scpd xmlns=\"urn:schemas-upnp-org:service-1-0\"><specVersion><major>1</major><minor>0</minor></specVersion><actionList>",
    );
    for action in actions {
        xml.push_str("<action><name>");
        xml.push_str(action.name);
        xml.push_str("</name><argumentList>");
        for (name, direction, variable) in action.arguments {
            xml.push_str("<argument><name>");
            xml.push_str(name);
            xml.push_str("</name><direction>");
            xml.push_str(direction);
            xml.push_str("</direction><relatedStateVariable>");
            xml.push_str(variable);
            xml.push_str("</relatedStateVariable></argument>");
        }
        xml.push_str("</argumentList></action>");
    }
    xml.push_str("</actionList><serviceStateTable>");
    for variable in variables {
        xml.push_str("<stateVariable sendEvents=\"");
        xml.push_str(if variable.sends_events { "yes" } else { "no" });
        xml.push_str("\"><name>");
        xml.push_str(variable.name);
        xml.push_str("</name><dataType>");
        xml.push_str(variable.data_type);
        xml.push_str("</dataType>");
        if !variable.allowed_values.is_empty() {
            xml.push_str("<allowedValueList>");
            for value in variable.allowed_values {
                xml.push_str("<allowedValue>");
                xml.push_str(value);
                xml.push_str("</allowedValue>");
            }
            xml.push_str("</allowedValueList>");
        }
        xml.push_str("</stateVariable>");
    }
    xml.push_str("</serviceStateTable></scpd>");
    xml
}

fn push_xml_text(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            character if character.is_control() && !matches!(character, '\t' | '\n' | '\r') => {}
            character => output.push(character),
        }
    }
}

fn push_xml_name(output: &mut String, value: &str) {
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
            output.push(character);
        }
    }
}

fn feature_list() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Features xmlns=\"urn:schemas-upnp-org:av:avs\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:schemas-upnp-org:av:avs http://www.upnp.org/schemas/av/avs.xsd\"><Feature name=\"samsung.com_BASICVIEW\" version=\"1\"><container id=\"I\" type=\"object.item.imageItem\"/><container id=\"A\" type=\"object.item.audioItem\"/><container id=\"V\" type=\"object.item.videoItem\"/></Feature></Features>"
}

fn empty_didl() -> &'static str {
    "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"></DIDL-Lite>"
}

const CONTENT_DIRECTORY_ACTIONS: &[ActionDescription] = &[
    ActionDescription {
        name: "GetSearchCapabilities",
        arguments: &[("SearchCaps", "out", "SearchCapabilities")],
    },
    ActionDescription {
        name: "GetSortCapabilities",
        arguments: &[("SortCaps", "out", "SortCapabilities")],
    },
    ActionDescription {
        name: "GetSystemUpdateID",
        arguments: &[("Id", "out", "SystemUpdateID")],
    },
    ActionDescription {
        name: "Browse",
        arguments: &[
            ("ObjectID", "in", "A_ARG_TYPE_ObjectID"),
            ("BrowseFlag", "in", "A_ARG_TYPE_BrowseFlag"),
            ("Filter", "in", "A_ARG_TYPE_Filter"),
            ("StartingIndex", "in", "A_ARG_TYPE_Index"),
            ("RequestedCount", "in", "A_ARG_TYPE_Count"),
            ("SortCriteria", "in", "A_ARG_TYPE_SortCriteria"),
            ("Result", "out", "A_ARG_TYPE_Result"),
            ("NumberReturned", "out", "A_ARG_TYPE_Count"),
            ("TotalMatches", "out", "A_ARG_TYPE_Count"),
            ("UpdateID", "out", "A_ARG_TYPE_UpdateID"),
        ],
    },
    ActionDescription {
        name: "Search",
        arguments: &[
            ("ContainerID", "in", "A_ARG_TYPE_ObjectID"),
            ("SearchCriteria", "in", "A_ARG_TYPE_SearchCriteria"),
            ("Filter", "in", "A_ARG_TYPE_Filter"),
            ("StartingIndex", "in", "A_ARG_TYPE_Index"),
            ("RequestedCount", "in", "A_ARG_TYPE_Count"),
            ("SortCriteria", "in", "A_ARG_TYPE_SortCriteria"),
            ("Result", "out", "A_ARG_TYPE_Result"),
            ("NumberReturned", "out", "A_ARG_TYPE_Count"),
            ("TotalMatches", "out", "A_ARG_TYPE_Count"),
            ("UpdateID", "out", "A_ARG_TYPE_UpdateID"),
        ],
    },
    ActionDescription {
        name: "X_GetFeatureList",
        arguments: &[("FeatureList", "out", "A_ARG_TYPE_Featurelist")],
    },
    ActionDescription {
        name: "X_SetBookmark",
        arguments: &[
            ("CategoryType", "in", "A_ARG_TYPE_CategoryType"),
            ("RID", "in", "A_ARG_TYPE_RID"),
            ("ObjectID", "in", "A_ARG_TYPE_ObjectID"),
            ("PosSecond", "in", "A_ARG_TYPE_PosSec"),
        ],
    },
    ActionDescription {
        name: "X_BrowseByLetter",
        arguments: &[
            ("ObjectID", "in", "A_ARG_TYPE_ObjectID"),
            ("BrowseFlag", "in", "A_ARG_TYPE_BrowseFlag"),
            ("Filter", "in", "A_ARG_TYPE_Filter"),
            ("StartingLetter", "in", "A_ARG_TYPE_BrowseLetter"),
            ("RequestedCount", "in", "A_ARG_TYPE_Count"),
            ("SortCriteria", "in", "A_ARG_TYPE_SortCriteria"),
            ("Result", "out", "A_ARG_TYPE_Result"),
            ("NumberReturned", "out", "A_ARG_TYPE_Count"),
            ("TotalMatches", "out", "A_ARG_TYPE_Count"),
            ("UpdateID", "out", "A_ARG_TYPE_UpdateID"),
            ("StartingIndex", "out", "A_ARG_TYPE_Index"),
        ],
    },
];

const CONTENT_DIRECTORY_STATE_VARIABLES: &[StateVariableDescription] = &[
    state("A_ARG_TYPE_Filter", "string", false, &[]),
    state("A_ARG_TYPE_SortCriteria", "string", false, &[]),
    state("A_ARG_TYPE_Index", "ui4", false, &[]),
    state("A_ARG_TYPE_Count", "ui4", false, &[]),
    state("A_ARG_TYPE_UpdateID", "ui4", false, &[]),
    state("SearchCapabilities", "string", false, &[]),
    state("SortCapabilities", "string", false, &[]),
    state("SystemUpdateID", "ui4", true, &[]),
    state("A_ARG_TYPE_SearchCriteria", "string", false, &[]),
    state("A_ARG_TYPE_Result", "string", false, &[]),
    state("A_ARG_TYPE_ObjectID", "string", false, &[]),
    state(
        "A_ARG_TYPE_BrowseFlag",
        "string",
        false,
        &["BrowseMetadata", "BrowseDirectChildren"],
    ),
    state("A_ARG_TYPE_BrowseLetter", "string", false, &[]),
    state("A_ARG_TYPE_CategoryType", "ui4", false, &[]),
    state("A_ARG_TYPE_RID", "ui4", false, &[]),
    state("A_ARG_TYPE_PosSec", "ui4", false, &[]),
    state("A_ARG_TYPE_Featurelist", "string", false, &[]),
];

const CONNECTION_MANAGER_ACTIONS: &[ActionDescription] = &[
    ActionDescription {
        name: "GetCurrentConnectionInfo",
        arguments: &[
            ("ConnectionID", "in", "A_ARG_TYPE_ConnectionID"),
            ("RcsID", "out", "A_ARG_TYPE_RcsID"),
            ("AVTransportID", "out", "A_ARG_TYPE_AVTransportID"),
            ("ProtocolInfo", "out", "A_ARG_TYPE_ProtocolInfo"),
            (
                "PeerConnectionManager",
                "out",
                "A_ARG_TYPE_ConnectionManager",
            ),
            ("PeerConnectionID", "out", "A_ARG_TYPE_ConnectionID"),
            ("Direction", "out", "A_ARG_TYPE_Direction"),
            ("Status", "out", "A_ARG_TYPE_ConnectionStatus"),
        ],
    },
    ActionDescription {
        name: "GetProtocolInfo",
        arguments: &[
            ("Source", "out", "SourceProtocolInfo"),
            ("Sink", "out", "SinkProtocolInfo"),
        ],
    },
    ActionDescription {
        name: "GetCurrentConnectionIDs",
        arguments: &[("ConnectionIDs", "out", "CurrentConnectionIDs")],
    },
];

const CONNECTION_MANAGER_STATE_VARIABLES: &[StateVariableDescription] = &[
    state("SourceProtocolInfo", "string", true, &[]),
    state("SinkProtocolInfo", "string", true, &[]),
    state("CurrentConnectionIDs", "string", true, &[]),
    state(
        "A_ARG_TYPE_ConnectionStatus",
        "string",
        false,
        &[
            "OK",
            "ContentFormatMismatch",
            "InsufficientBandwidth",
            "UnreliableChannel",
            "Unknown",
        ],
    ),
    state("A_ARG_TYPE_ConnectionManager", "string", false, &[]),
    state(
        "A_ARG_TYPE_Direction",
        "string",
        false,
        &["Output", "Input"],
    ),
    state("A_ARG_TYPE_ProtocolInfo", "string", false, &[]),
    state("A_ARG_TYPE_ConnectionID", "ui4", false, &[]),
    state("A_ARG_TYPE_AVTransportID", "ui4", false, &[]),
    state("A_ARG_TYPE_RcsID", "ui4", false, &[]),
];

const fn state(
    name: &'static str,
    data_type: &'static str,
    sends_events: bool,
    allowed_values: &'static [&'static str],
) -> StateVariableDescription {
    StateVariableDescription {
        name,
        data_type,
        sends_events,
        allowed_values,
    }
}
