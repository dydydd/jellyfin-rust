use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jellyfin_model::TunerHostInfo;
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "PascalCase")]
pub struct DiscoverResponse {
    pub friendly_name: Option<String>,
    pub model_number: Option<String>,
    pub firmware_name: Option<String>,
    pub firmware_version: Option<String>,
    #[serde(rename = "DeviceID")]
    pub device_id: Option<String>,
    pub device_auth: Option<String>,
    #[serde(rename = "BaseURL")]
    pub base_url: Option<String>,
    #[serde(rename = "LineupURL")]
    pub lineup_url: Option<String>,
    pub tuner_count: i32,
}

impl DiscoverResponse {
    #[must_use]
    pub fn supports_transcoding(&self) -> bool {
        self.model_number
            .as_deref()
            .is_some_and(|model| model.to_ascii_lowercase().contains("hdtc"))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, rename_all = "PascalCase")]
pub struct HdHomerunChannel {
    pub guide_number: String,
    pub guide_name: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[serde(rename = "URL")]
    pub url: Option<String>,
    #[serde(deserialize_with = "deserialize_bool_number")]
    pub favorite: bool,
    #[serde(rename = "DRM", deserialize_with = "deserialize_bool_number")]
    pub drm: bool,
    #[serde(rename = "HD", deserialize_with = "deserialize_bool_number")]
    pub hd: bool,
}

fn deserialize_bool_number<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolNumber {
        Bool(bool),
        Number(i32),
    }

    Ok(match BoolNumber::deserialize(deserializer)? {
        BoolNumber::Bool(value) => value,
        BoolNumber::Number(value) => value != 0,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait HdHomerunHttpClient: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HdHomerunHostError>> + Send + 'a>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HdHomerunHostError {
    InvalidTunerUrl { value: String },
    HttpStatus { status: u16, url: String },
    RequestTimedOut { url: String },
    HttpTransport { url: String, message: String },
    ResponseTooLarge { limit: usize, actual: usize },
    InvalidJson { url: String, message: String },
}

impl fmt::Display for HdHomerunHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTunerUrl { value } => write!(formatter, "invalid tuner URL: {value}"),
            Self::HttpStatus { status, url } => write!(formatter, "HTTP {status} from {url}"),
            Self::RequestTimedOut { url } => write!(formatter, "request to {url} timed out"),
            Self::HttpTransport { url, message } => {
                write!(formatter, "request to {url} failed: {message}")
            }
            Self::ResponseTooLarge { limit, actual } => {
                write!(formatter, "response is {actual} bytes, exceeding {limit}")
            }
            Self::InvalidJson { url, message } => {
                write!(formatter, "invalid JSON from {url}: {message}")
            }
        }
    }
}

impl Error for HdHomerunHostError {}

pub struct ReqwestHdHomerunHttpClient {
    client: reqwest::Client,
}

impl ReqwestHdHomerunHttpClient {
    pub fn new(timeout: Duration) -> Result<Self, HdHomerunHostError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .map_err(|error| HdHomerunHostError::HttpTransport {
                url: String::new(),
                message: error.to_string(),
            })?;
        Ok(Self { client })
    }
}

impl HdHomerunHttpClient for ReqwestHdHomerunHttpClient {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        max_response_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HdHomerunHostError>> + Send + 'a>> {
        Box::pin(async move {
            let mut response = self.client.get(url.clone()).send().await.map_err(|error| {
                if error.is_timeout() {
                    HdHomerunHostError::RequestTimedOut {
                        url: url.to_string(),
                    }
                } else {
                    HdHomerunHostError::HttpTransport {
                        url: url.to_string(),
                        message: error.to_string(),
                    }
                }
            })?;
            let status = response.status().as_u16();
            if let Some(length) = response.content_length() {
                let actual = usize::try_from(length).unwrap_or(usize::MAX);
                if actual > max_response_bytes {
                    return Err(HdHomerunHostError::ResponseTooLarge {
                        limit: max_response_bytes,
                        actual,
                    });
                }
            }
            if !response.status().is_success() {
                return Ok(HttpResponse {
                    status,
                    body: Vec::new(),
                });
            }

            let mut body = Vec::new();
            while let Some(chunk) =
                response
                    .chunk()
                    .await
                    .map_err(|error| HdHomerunHostError::HttpTransport {
                        url: url.to_string(),
                        message: error.to_string(),
                    })?
            {
                let actual = body.len().saturating_add(chunk.len());
                if actual > max_response_bytes {
                    return Err(HdHomerunHostError::ResponseTooLarge {
                        limit: max_response_bytes,
                        actual,
                    });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HttpResponse { status, body })
        })
    }
}

pub struct HdHomerunHost {
    http: Arc<dyn HdHomerunHttpClient>,
    max_response_bytes: usize,
    model_cache: Mutex<HashMap<String, DiscoverResponse>>,
}

impl HdHomerunHost {
    pub fn new() -> Result<Self, HdHomerunHostError> {
        Self::with_timeout(DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Result<Self, HdHomerunHostError> {
        Ok(Self::with_client(ReqwestHdHomerunHttpClient::new(timeout)?))
    }

    #[must_use]
    pub fn with_client(client: impl HdHomerunHttpClient + 'static) -> Self {
        Self::with_client_and_limit(client, DEFAULT_MAX_RESPONSE_BYTES)
    }

    #[must_use]
    pub fn with_client_and_limit(
        client: impl HdHomerunHttpClient + 'static,
        max_response_bytes: usize,
    ) -> Self {
        Self {
            http: Arc::new(client),
            max_response_bytes,
            model_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_model_info(
        &self,
        info: &TunerHostInfo,
        throw_all_exceptions: bool,
    ) -> Result<DiscoverResponse, HdHomerunHostError> {
        let base_url = normalize_base_url(&info.url)?;
        if let Some(id) = info.id.as_deref().filter(|id| !id.is_empty())
            && let Some(cached) = self
                .model_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(id)
                .cloned()
        {
            return Ok(cached);
        }
        let url = base_url
            .join("discover.json")
            .map_err(|_| invalid_url(&info.url))?;
        let response = self.fetch(&url).await?;
        let model = if response.status == 404 && !throw_all_exceptions {
            DiscoverResponse {
                model_number: Some("HDHR".to_owned()),
                base_url: Some(base_url.as_str().trim_end_matches('/').to_owned()),
                ..DiscoverResponse::default()
            }
        } else {
            ensure_success(response.status, &url)?;
            serde_json::from_slice(&response.body).map_err(|error| {
                HdHomerunHostError::InvalidJson {
                    url: url.to_string(),
                    message: error.to_string(),
                }
            })?
        };
        if let Some(id) = info.id.as_deref().filter(|id| !id.is_empty()) {
            self.model_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(id.to_owned(), model.clone());
        }
        Ok(model)
    }

    pub async fn get_lineup(
        &self,
        info: &TunerHostInfo,
    ) -> Result<Vec<HdHomerunChannel>, HdHomerunHostError> {
        let model = self.get_model_info(info, false).await?;
        let lineup_url = lineup_url(&model, &info.url)?;
        let response = self.fetch(&lineup_url).await?;
        ensure_success(response.status, &lineup_url)?;
        let channels: Option<Vec<HdHomerunChannel>> = serde_json::from_slice(&response.body)
            .map_err(|error| HdHomerunHostError::InvalidJson {
                url: lineup_url.to_string(),
                message: error.to_string(),
            })?;
        Ok(channels
            .unwrap_or_default()
            .into_iter()
            .filter(|channel| !channel.drm)
            .filter(|channel| !info.import_favorites_only || channel.favorite)
            .collect())
    }

    pub async fn try_get_tuner_host_info(
        &self,
        url: impl Into<String>,
    ) -> Result<TunerHostInfo, HdHomerunHostError> {
        let url = url.into();
        let mut host = TunerHostInfo {
            url,
            tuner_type: "hdhomerun".to_owned(),
            ..TunerHostInfo::default()
        };
        let model = self.get_model_info(&host, false).await?;
        host.device_id = model.device_id;
        host.friendly_name = model.friendly_name;
        host.tuner_count = model.tuner_count;
        Ok(host)
    }

    async fn fetch(&self, url: &Url) -> Result<HttpResponse, HdHomerunHostError> {
        let response = self.http.get(url, self.max_response_bytes).await?;
        if response.body.len() > self.max_response_bytes {
            return Err(HdHomerunHostError::ResponseTooLarge {
                limit: self.max_response_bytes,
                actual: response.body.len(),
            });
        }
        Ok(response)
    }
}

fn normalize_base_url(value: &str) -> Result<Url, HdHomerunHostError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_url(value));
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else if value.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("http://[{value}]")
    } else {
        format!("http://{value}")
    };
    let mut url = Url::parse(&candidate).map_err(|_| invalid_url(value))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(invalid_url(value));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn lineup_url(model: &DiscoverResponse, tuner_url: &str) -> Result<Url, HdHomerunHostError> {
    if let Some(value) = model
        .lineup_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return Url::parse(value).map_err(|_| invalid_url(value));
    }
    let base = model.base_url.as_deref().unwrap_or(tuner_url);
    normalize_base_url(base)?
        .join("lineup.json")
        .map_err(|_| invalid_url(base))
}

fn ensure_success(status: u16, url: &Url) -> Result<(), HdHomerunHostError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(HdHomerunHostError::HttpStatus {
            status,
            url: url.to_string(),
        })
    }
}

fn invalid_url(value: &str) -> HdHomerunHostError {
    HdHomerunHostError::InvalidTunerUrl {
        value: value.to_owned(),
    }
}
