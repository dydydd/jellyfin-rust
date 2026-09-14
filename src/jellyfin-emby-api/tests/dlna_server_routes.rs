use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jellyfin_api::AppState;
use sea_orm::DatabaseConnection;
use tower::ServiceExt;

const UDN: &str = "01234567-89ab-cdef-0123-456789abcdef";

fn app() -> axum::Router {
    jellyfin_emby_api::router(
        AppState::new(
            DatabaseConnection::Disconnected,
            "Living Room & Music".to_owned(),
            "http://127.0.0.1:18096".to_owned(),
        )
        .with_server_id("0123456789abcdef0123456789abcdef".to_owned()),
    )
}

#[tokio::test]
async fn descriptions_are_public_xml_and_keep_emby_paths_isolated() {
    let response = app()
        .oneshot(
            Request::get(format!("/emby/Dlna/{UDN}/description.xml"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/xml; charset=UTF-8"
    );
    let body = String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(&format!("<UDN>uuid:{UDN}</UDN>")));
    assert!(body.contains("<friendlyName>Living Room &amp; Music</friendlyName>"));
    assert!(body.contains(&format!(
        "<SCPDURL>/emby/dlna/{UDN}/contentdirectory/contentdirectory.xml</SCPDURL>"
    )));
    assert!(body.contains(&format!(
        "<controlURL>/emby/dlna/{UDN}/connectionmanager/control</controlURL>"
    )));

    // Emby 4.10 does not validate UuId against the configured server id.
    let mismatched = app()
        .oneshot(
            Request::get("/emby/dlna/a-stale-udn/description")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::OK);

    let mixed_case = app()
        .oneshot(
            Request::get(format!("/emby/dLnA/{UDN}/DeScRiPtIoN.XmL"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mixed_case.status(), StatusCode::OK);

    for path in [
        format!("/Dlna/{UDN}/description.xml"),
        format!("/api/Dlna/{UDN}/description.xml"),
    ] {
        let response = app()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn head_and_both_scpd_documents_match_upnp_shapes() {
    for suffix in [
        "description",
        "description.xml",
        "contentdirectory/contentdirectory",
        "contentdirectory/contentdirectory.xml",
        "connectionmanager/connectionmanager",
        "connectionmanager/connectionmanager.xml",
    ] {
        let response = app()
            .oneshot(
                Request::head(format!("/emby/Dlna/{UDN}/{suffix}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
        assert_eq!(
            response.headers()["content-type"],
            "text/xml; charset=UTF-8"
        );
        assert!(to_bytes(response.into_body(), 1).await.unwrap().is_empty());
    }

    let cases = [
        (
            format!("/emby/Dlna/{UDN}/contentdirectory/contentdirectory.xml"),
            ["GetSearchCapabilities", "Browse", "X_BrowseByLetter"],
        ),
        (
            format!("/emby/dlna/{UDN}/connectionmanager/connectionmanager"),
            [
                "GetCurrentConnectionInfo",
                "GetProtocolInfo",
                "GetCurrentConnectionIDs",
            ],
        ),
    ];
    for (path, actions) in cases {
        let response = app()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("urn:schemas-upnp-org:service-1-0"));
        for action in actions {
            assert!(body.contains(&format!("<name>{action}</name>")));
        }
    }
}

#[tokio::test]
async fn connection_manager_control_returns_real_soap_and_invalid_action_faults() {
    let request = soap_request(
        "GetProtocolInfo",
        "urn:schemas-upnp-org:service:ConnectionManager:1",
    );
    let response = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/connectionmanager/control"))
                .header("content-type", "text/xml; charset=utf-8")
                .body(Body::from(request))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/xml; charset=UTF-8"
    );
    assert_eq!(response.headers()["ext"], "");
    let body = body_text(response).await;
    assert!(body.contains("<u:GetProtocolInfoResponse"));
    assert!(body.contains("<Source>http-get:*:video/mpeg:*"));
    assert!(body.contains("<Sink></Sink>"));

    let invalid = app()
        .oneshot(
            Request::post(format!("/emby/dlna/{UDN}/connectionmanager/control"))
                .body(Body::from(soap_request(
                    "GetCurrentConnectionInfo",
                    "urn:schemas-upnp-org:service:ConnectionManager:1",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::OK);
    let body = body_text(invalid).await;
    assert!(body.contains("<errorCode>401</errorCode>"));
    assert!(body.contains("<errorDescription>Invalid Action</errorDescription>"));
}

#[tokio::test]
async fn content_directory_controls_return_capabilities_and_bounded_empty_didl() {
    let capabilities = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/contentdirectory/control"))
                .body(Body::from(soap_request(
                    "GetSearchCapabilities",
                    "urn:schemas-upnp-org:service:ContentDirectory:1",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(capabilities.status(), StatusCode::OK);
    let body = body_text(capabilities).await;
    assert!(body.contains("<u:GetSearchCapabilitiesResponse"));
    assert!(body.contains("dc:title,dc:creator,upnp:artist"));

    let browse = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/contentdirectory/control"))
                .body(Body::from(soap_request(
                    "Browse",
                    "urn:schemas-upnp-org:service:ContentDirectory:1",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(browse.status(), StatusCode::OK);
    let body = body_text(browse).await;
    assert!(body.contains("<u:BrowseResponse"));
    assert!(body.contains("&lt;DIDL-Lite"));
    assert!(body.contains("<NumberReturned>0</NumberReturned>"));
    assert!(body.contains("<TotalMatches>0</TotalMatches>"));

    let malformed = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/contentdirectory/control"))
                .body(Body::from("<!DOCTYPE x><not-soap/>"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::OK);
    assert!(
        body_text(malformed)
            .await
            .contains("<errorCode>401</errorCode>")
    );

    // Comments are ignored like XmlReader; a fake Body inside one must not
    // replace the real SOAP action.
    let commented = format!(
        "<!-- <s:Body><u:Unknown/></s:Body> -->{}",
        soap_request(
            "GetSortCapabilities",
            "urn:schemas-upnp-org:service:ContentDirectory:1",
        )
    );
    let response = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/contentdirectory/control"))
                .body(Body::from(commented))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        body_text(response)
            .await
            .contains("<u:GetSortCapabilitiesResponse")
    );

    let oversized = app()
        .oneshot(
            Request::post(format!("/emby/Dlna/{UDN}/contentdirectory/control"))
                .body(Body::from(vec![b'x'; 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn icon_routes_are_cached_bounded_resources_and_ignore_uuid_like_emby() {
    let cases = [
        ("logo48.png", "image/png", [0x89, b'P', b'N', b'G']),
        ("LOGO240.JPG", "image/jpg", [0xff, 0xd8, 0xff, 0xe0]),
    ];
    for (filename, content_type, magic) in cases {
        for path in [
            format!("/emby/Dlna/icons/{filename}?UuId=anything"),
            format!("/emby/dlna/not-the-server/icons/{filename}"),
        ] {
            let response = app()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{filename}");
            assert_eq!(response.headers()["content-type"], content_type);
            assert_eq!(
                response.headers()["cache-control"],
                "public, max-age=31536000"
            );
            assert!(!response.headers().contains_key("accept-ranges"));
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(&bytes[..4], &magic);
        }
    }

    let unknown = app()
        .oneshot(
            Request::get("/emby/Dlna/icons/unknown.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
}

fn soap_request(action: &str, namespace: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action} xmlns:u=\"{namespace}\"></u:{action}></s:Body></s:Envelope>"
    )
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
