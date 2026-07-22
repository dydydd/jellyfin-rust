use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use jellyfin_live_tv::tuner_hosts::hdhomerun::{
    HdHomerunHost, HdHomerunHostError, HdHomerunHttpClient, HttpResponse,
};
use jellyfin_model::TunerHostInfo;
use url::Url;

const MODERN_DISCOVER: &[u8] = include_bytes!("fixtures/hdhomerun/192.168.1.182/discover.json");
const MODERN_LINEUP: &[u8] = include_bytes!("fixtures/hdhomerun/192.168.1.182/lineup.json");
const LEGACY_DISCOVER: &[u8] = include_bytes!("fixtures/hdhomerun/10.10.10.100/discover.json");
const LEGACY_LINEUP: &[u8] = include_bytes!("fixtures/hdhomerun/10.10.10.100/lineup.json");

#[derive(Clone, Default)]
struct FakeHttpClient {
    responses: Arc<Mutex<HashMap<String, Result<HttpResponse, HdHomerunHostError>>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeHttpClient {
    fn fixtures() -> Self {
        let client = Self::default();
        for (url, body) in [
            ("http://192.168.1.182/discover.json", MODERN_DISCOVER),
            ("http://192.168.1.182/lineup.json", MODERN_LINEUP),
            ("http://10.10.10.100/discover.json", LEGACY_DISCOVER),
            ("http://10.10.10.100/lineup.json", LEGACY_LINEUP),
        ] {
            client.respond(url, 200, body);
        }
        client
    }

    fn respond(&self, url: &str, status: u16, body: &[u8]) {
        self.responses.lock().unwrap().insert(
            url.to_owned(),
            Ok(HttpResponse {
                status,
                body: body.to_vec(),
            }),
        );
    }

    fn fail(&self, url: &str, error: HdHomerunHostError) {
        self.responses
            .lock()
            .unwrap()
            .insert(url.to_owned(), Err(error));
    }
}

impl HdHomerunHttpClient for FakeHttpClient {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        _max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HdHomerunHostError>> + Send + 'a>> {
        let key = url.to_string();
        self.requests.lock().unwrap().push(key.clone());
        let response = self
            .responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_else(|| {
                Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                })
            });
        Box::pin(async move { response })
    }
}

fn info(url: &str) -> TunerHostInfo {
    TunerHostInfo {
        url: url.to_owned(),
        ..TunerHostInfo::default()
    }
}

#[tokio::test]
async fn get_model_info_valid_success() {
    let host = HdHomerunHost::with_client(FakeHttpClient::fixtures());
    let model = host
        .get_model_info(&info("192.168.1.182"), true)
        .await
        .unwrap();
    assert_eq!(Some("HDHomeRun PRIME"), model.friendly_name.as_deref());
    assert_eq!(Some("HDHR3-CC"), model.model_number.as_deref());
    assert_eq!(Some("hdhomerun3_cablecard"), model.firmware_name.as_deref());
    assert_eq!(Some("20160630atest2"), model.firmware_version.as_deref());
    assert_eq!(Some("FFFFFFFF"), model.device_id.as_deref());
    assert_eq!(Some("FFFFFFFF"), model.device_auth.as_deref());
    assert_eq!(3, model.tuner_count);
    assert_eq!(Some("http://192.168.1.182:80"), model.base_url.as_deref());
    assert_eq!(
        Some("http://192.168.1.182:80/lineup.json"),
        model.lineup_url.as_deref()
    );
}

#[tokio::test]
async fn get_model_info_legacy_success() {
    let host = HdHomerunHost::with_client(FakeHttpClient::fixtures());
    let model = host
        .get_model_info(&info("10.10.10.100"), true)
        .await
        .unwrap();
    assert_eq!(Some("HDHomeRun DUAL"), model.friendly_name.as_deref());
    assert_eq!(Some("HDHR3-US"), model.model_number.as_deref());
    assert_eq!(Some("hdhomerun3_atsc"), model.firmware_name.as_deref());
    assert_eq!(Some("20200225"), model.firmware_version.as_deref());
    assert_eq!(Some("10xxxxx5"), model.device_id.as_deref());
    assert_eq!(None, model.device_auth);
    assert_eq!(2, model.tuner_count);
    assert_eq!(Some("http://10.10.10.100:80"), model.base_url.as_deref());
    assert_eq!(None, model.lineup_url);
}

#[tokio::test]
async fn get_model_info_empty_url_is_invalid() {
    let error = HdHomerunHost::with_client(FakeHttpClient::default())
        .get_model_info(&info(""), true)
        .await
        .unwrap_err();
    assert!(matches!(error, HdHomerunHostError::InvalidTunerUrl { .. }));
}

#[tokio::test]
async fn get_lineup_valid_success() {
    let channels = HdHomerunHost::with_client(FakeHttpClient::fixtures())
        .get_lineup(&info("192.168.1.182"))
        .await
        .unwrap();
    assert_eq!(6, channels.len());
    assert_eq!("4.1", channels[0].guide_number);
    assert_eq!("WCMH-DT", channels[0].guide_name);
    assert!(channels[0].hd);
    assert!(channels[0].favorite);
    assert_eq!(
        Some("http://192.168.1.111:5004/auto/v4.1"),
        channels[0].url.as_deref()
    );
}

#[tokio::test]
async fn get_lineup_legacy_invalid_json_is_reported() {
    let error = HdHomerunHost::with_client(FakeHttpClient::fixtures())
        .get_lineup(&info("10.10.10.100"))
        .await
        .unwrap_err();
    assert!(matches!(error, HdHomerunHostError::InvalidJson { .. }));
}

#[tokio::test]
async fn get_lineup_import_favorites_only_success() {
    let mut tuner = info("192.168.1.182");
    tuner.import_favorites_only = true;
    let channels = HdHomerunHost::with_client(FakeHttpClient::fixtures())
        .get_lineup(&tuner)
        .await
        .unwrap();
    assert_eq!(1, channels.len());
    assert_eq!("4.1", channels[0].guide_number);
}

#[tokio::test]
async fn try_get_tuner_host_info_valid_success() {
    let host = HdHomerunHost::with_client(FakeHttpClient::fixtures())
        .try_get_tuner_host_info("192.168.1.182")
        .await
        .unwrap();
    assert_eq!("hdhomerun", host.tuner_type);
    assert_eq!("192.168.1.182", host.url);
    assert_eq!(Some("HDHomeRun PRIME"), host.friendly_name.as_deref());
    assert_eq!(Some("FFFFFFFF"), host.device_id.as_deref());
    assert_eq!(3, host.tuner_count);
}

#[tokio::test]
async fn not_found_falls_back_only_when_requested() {
    let client = FakeHttpClient::default();
    let host = HdHomerunHost::with_client(client);
    let fallback = host
        .get_model_info(&info("legacy.test"), false)
        .await
        .unwrap();
    assert_eq!(Some("HDHR"), fallback.model_number.as_deref());
    assert_eq!(Some("http://legacy.test"), fallback.base_url.as_deref());
    let error = host
        .get_model_info(&info("legacy.test"), true)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        HdHomerunHostError::HttpStatus { status: 404, .. }
    ));
}

#[tokio::test]
async fn response_status_size_timeout_and_invalid_json_are_reported() {
    let status = FakeHttpClient::default();
    status.respond("http://status.test/discover.json", 503, b"{}");
    assert!(matches!(
        HdHomerunHost::with_client(status)
            .get_model_info(&info("status.test"), true)
            .await,
        Err(HdHomerunHostError::HttpStatus { status: 503, .. })
    ));

    let large = FakeHttpClient::default();
    large.respond("http://large.test/discover.json", 200, &[b'x'; 33]);
    assert_eq!(
        Err(HdHomerunHostError::ResponseTooLarge {
            limit: 32,
            actual: 33
        }),
        HdHomerunHost::with_client_and_limit(large, 32)
            .get_model_info(&info("large.test"), true)
            .await
    );

    let timeout = FakeHttpClient::default();
    timeout.fail(
        "http://timeout.test/discover.json",
        HdHomerunHostError::RequestTimedOut {
            url: "http://timeout.test/discover.json".to_owned(),
        },
    );
    assert!(matches!(
        HdHomerunHost::with_client(timeout)
            .get_model_info(&info("timeout.test"), true)
            .await,
        Err(HdHomerunHostError::RequestTimedOut { .. })
    ));

    let invalid = FakeHttpClient::default();
    invalid.respond("http://json.test/discover.json", 200, b"{");
    assert!(matches!(
        HdHomerunHost::with_client(invalid)
            .get_model_info(&info("json.test"), true)
            .await,
        Err(HdHomerunHostError::InvalidJson { .. })
    ));
}

#[tokio::test]
async fn bare_ipv6_hosts_are_bracketed_before_requesting() {
    let client = FakeHttpClient::default();
    client.respond("http://[2001:db8::1]/discover.json", 200, b"{}");
    let requests = Arc::clone(&client.requests);
    HdHomerunHost::with_client(client)
        .get_model_info(&info("2001:db8::1"), true)
        .await
        .unwrap();
    assert_eq!(
        ["http://[2001:db8::1]/discover.json"],
        requests.lock().unwrap().as_slice()
    );
}
