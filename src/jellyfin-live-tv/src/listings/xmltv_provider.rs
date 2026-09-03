use std::error::Error;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;

use super::{
    ProgramInfo, XmlTvChannel, XmlTvOptions, XmlTvParseError, parse_xmltv_channels,
    parse_xmltv_programs,
};

const DEFAULT_MAX_XML_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct XmlTvProviderInfo {
    pub path: String,
    pub options: XmlTvOptions,
}

#[derive(Debug)]
pub enum XmlTvProviderError {
    EmptyPath,
    UnsupportedUrl(String),
    FileNotFound(String),
    Io(String),
    HttpStatus { status: u16, url: String },
    Timeout(String),
    Http(String),
    TooLarge { limit: usize },
    InvalidUtf8,
    Parse(XmlTvParseError),
}

impl fmt::Display for XmlTvProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("XMLTV path is empty"),
            Self::UnsupportedUrl(value) => write!(formatter, "unsupported XMLTV URL: {value}"),
            Self::FileNotFound(value) => write!(formatter, "XMLTV file does not exist: {value}"),
            Self::Io(value) | Self::Http(value) => formatter.write_str(value),
            Self::HttpStatus { status, url } => write!(formatter, "HTTP {status} from {url}"),
            Self::Timeout(url) => write!(formatter, "request to {url} timed out"),
            Self::TooLarge { limit } => write!(formatter, "XMLTV source exceeds {limit} bytes"),
            Self::InvalidUtf8 => formatter.write_str("XMLTV source is not UTF-8"),
            Self::Parse(error) => error.fmt(formatter),
        }
    }
}

impl Error for XmlTvProviderError {}

impl From<XmlTvParseError> for XmlTvProviderError {
    fn from(value: XmlTvParseError) -> Self {
        Self::Parse(value)
    }
}

pub struct XmlTvListingsProvider {
    client: reqwest::Client,
    max_xml_bytes: usize,
}

impl XmlTvListingsProvider {
    pub fn new() -> Result<Self, XmlTvProviderError> {
        Self::with_limits(DEFAULT_TIMEOUT, DEFAULT_MAX_XML_BYTES)
    }

    pub fn with_limits(
        timeout: Duration,
        max_xml_bytes: usize,
    ) -> Result<Self, XmlTvProviderError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .map_err(|error| XmlTvProviderError::Http(error.to_string()))?;
        Ok(Self {
            client,
            max_xml_bytes,
        })
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        "XmlTV"
    }

    #[must_use]
    pub const fn provider_type(&self) -> &'static str {
        "xmltv"
    }

    pub fn validate(&self, info: &XmlTvProviderInfo) -> Result<(), XmlTvProviderError> {
        let path = info.path.trim();
        if path.is_empty() {
            return Err(XmlTvProviderError::EmptyPath);
        }
        if is_http(path) {
            reqwest::Url::parse(path)
                .map_err(|_| XmlTvProviderError::UnsupportedUrl(path.to_owned()))?;
            return Ok(());
        }
        if path.contains("://") {
            return Err(XmlTvProviderError::UnsupportedUrl(path.to_owned()));
        }
        if !Path::new(path).is_file() {
            return Err(XmlTvProviderError::FileNotFound(path.to_owned()));
        }
        Ok(())
    }

    pub async fn get_programs(
        &self,
        info: &XmlTvProviderInfo,
        channel_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<ProgramInfo>, XmlTvProviderError> {
        let xml = self.acquire(&info.path).await?;
        Ok(parse_xmltv_programs(
            &xml,
            channel_id,
            start,
            end,
            &info.options,
        )?)
    }

    pub async fn get_channels(
        &self,
        info: &XmlTvProviderInfo,
    ) -> Result<Vec<XmlTvChannel>, XmlTvProviderError> {
        let xml = self.acquire(&info.path).await?;
        Ok(parse_xmltv_channels(&xml)?)
    }

    pub async fn get_lineups(
        &self,
        info: &XmlTvProviderInfo,
    ) -> Result<Vec<(String, String)>, XmlTvProviderError> {
        Ok(self
            .get_channels(info)
            .await?
            .into_iter()
            .map(|channel| (channel.id, channel.display_name))
            .collect())
    }

    async fn acquire(&self, source: &str) -> Result<String, XmlTvProviderError> {
        let source = source.trim();
        if source.is_empty() {
            return Err(XmlTvProviderError::EmptyPath);
        }
        let bytes = if is_http(source) {
            self.download(source).await?
        } else {
            if source.contains("://") {
                return Err(XmlTvProviderError::UnsupportedUrl(source.to_owned()));
            }
            let metadata = tokio::fs::metadata(source).await.map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    XmlTvProviderError::FileNotFound(source.to_owned())
                } else {
                    XmlTvProviderError::Io(error.to_string())
                }
            })?;
            if metadata.len() > self.max_xml_bytes as u64 {
                return Err(XmlTvProviderError::TooLarge {
                    limit: self.max_xml_bytes,
                });
            }
            tokio::fs::read(source)
                .await
                .map_err(|error| XmlTvProviderError::Io(error.to_string()))?
        };
        let bytes = if is_gzip(source) {
            decompress(&bytes, self.max_xml_bytes)?
        } else {
            bytes
        };
        if bytes.len() > self.max_xml_bytes {
            return Err(XmlTvProviderError::TooLarge {
                limit: self.max_xml_bytes,
            });
        }
        String::from_utf8(bytes).map_err(|_| XmlTvProviderError::InvalidUtf8)
    }

    async fn download(&self, source: &str) -> Result<Vec<u8>, XmlTvProviderError> {
        let mut response = self.client.get(source).send().await.map_err(|error| {
            if error.is_timeout() {
                XmlTvProviderError::Timeout(source.to_owned())
            } else {
                XmlTvProviderError::Http(error.to_string())
            }
        })?;
        if !response.status().is_success() {
            return Err(XmlTvProviderError::HttpStatus {
                status: response.status().as_u16(),
                url: source.to_owned(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.max_xml_bytes as u64)
        {
            return Err(XmlTvProviderError::TooLarge {
                limit: self.max_xml_bytes,
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| XmlTvProviderError::Http(error.to_string()))?
        {
            if body.len().saturating_add(chunk.len()) > self.max_xml_bytes {
                return Err(XmlTvProviderError::TooLarge {
                    limit: self.max_xml_bytes,
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn is_http(value: &str) -> bool {
    value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || value
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

fn is_gzip(value: &str) -> bool {
    value
        .split('?')
        .next()
        .and_then(|path| Path::new(path).extension())
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("gz") || extension.eq_ignore_ascii_case("gzip")
        })
}

fn decompress(bytes: &[u8], limit: usize) -> Result<Vec<u8>, XmlTvProviderError> {
    let mut output = Vec::new();
    GzDecoder::new(bytes)
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut output)
        .map_err(|error| XmlTvProviderError::Io(error.to_string()))?;
    if output.len() > limit {
        return Err(XmlTvProviderError::TooLarge { limit });
    }
    Ok(output)
}
