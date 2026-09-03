use std::time::Duration;

use jellyfin_data::{NewTunerHost, TunerHostRepository, TunerHostStoreError, entities::tuner_host};
use jellyfin_model::TunerHostInfo;
use thiserror::Error;
use uuid::Uuid;

use super::hdhomerun::HdHomerunHost;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum TunerHostError {
    #[error("unsupported tuner host type")]
    UnsupportedType,
    #[error("tuner host source could not be opened")]
    SourceUnavailable,
    #[error(transparent)]
    Store(#[from] TunerHostStoreError),
}

/// Validates and persists Live TV tuner-host configuration.
#[derive(Clone)]
pub struct TunerHostManager {
    repository: TunerHostRepository,
    http: reqwest::Client,
}

impl TunerHostManager {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self::from_repository(TunerHostRepository::new(database))
    }

    #[must_use]
    pub fn from_repository(repository: TunerHostRepository) -> Self {
        Self {
            repository,
            http: reqwest::Client::new(),
        }
    }

    /// Opens an M3U source before atomically saving its configuration.
    ///
    /// # Errors
    ///
    /// Returns not-found-style validation errors for unsupported providers or
    /// inaccessible sources, and a store error when persistence fails.
    pub async fn save(&self, mut host: TunerHostInfo) -> Result<TunerHostInfo, TunerHostError> {
        let host = match host.tuner_type.to_ascii_lowercase().as_str() {
            "m3u" => {
                self.validate_m3u_source(&host).await?;
                host
            }
            "hdhomerun" => {
                let tuner_host =
                    HdHomerunHost::new().map_err(|_| TunerHostError::SourceUnavailable)?;
                let model = tuner_host
                    .get_model_info(&host, false)
                    .await
                    .map_err(|_| TunerHostError::SourceUnavailable)?;
                host.device_id = model.device_id.or(host.device_id);
                if host.friendly_name.is_none() {
                    host.friendly_name = model.friendly_name;
                }
                host.tuner_count = model.tuner_count;
                host
            }
            _ => return Err(TunerHostError::UnsupportedType),
        };
        let requested_id = host.id.as_deref().and_then(parse_compact_uuid);
        let saved = self
            .repository
            .save(NewTunerHost {
                requested_id,
                url: host.url,
                tuner_type: host.tuner_type,
                device_id: host.device_id,
                friendly_name: host.friendly_name,
                import_favorites_only: host.import_favorites_only,
                allow_hw_transcoding: host.allow_hw_transcoding,
                allow_fmp4_transcoding_container: host.allow_fmp4_transcoding_container,
                allow_stream_sharing: host.allow_stream_sharing,
                fallback_max_streaming_bitrate: host.fallback_max_streaming_bitrate,
                enable_stream_looping: host.enable_stream_looping,
                source: host.source,
                tuner_count: host.tuner_count,
                user_agent: host.user_agent,
                ignore_dts: host.ignore_dts,
                read_at_native_framerate: host.read_at_native_framerate,
            })
            .await?;
        Ok(to_info(saved))
    }

    /// Deletes a compact UUID, treating invalid and absent values as no-ops.
    ///
    /// # Errors
    ///
    /// Returns a store error when persistence fails.
    pub async fn delete(&self, id: Option<&str>) -> Result<bool, TunerHostError> {
        Ok(self
            .repository
            .delete(id.and_then(parse_compact_uuid))
            .await?)
    }

    /// Lists persisted configuration for cross-instance consumers and tests.
    ///
    /// # Errors
    ///
    /// Returns a store error when persistence fails.
    pub async fn list(&self) -> Result<Vec<TunerHostInfo>, TunerHostError> {
        Ok(self
            .repository
            .list()
            .await?
            .into_iter()
            .map(to_info)
            .collect())
    }

    async fn validate_m3u_source(&self, host: &TunerHostInfo) -> Result<(), TunerHostError> {
        let remote_url = url::Url::parse(&host.url).ok().filter(|url| {
            url.scheme().eq_ignore_ascii_case("http") || url.scheme().eq_ignore_ascii_case("https")
        });
        if let Some(remote_url) = remote_url {
            let mut request = self.http.get(remote_url).timeout(VALIDATION_TIMEOUT);
            if let Some(user_agent) = host.user_agent.as_deref().filter(|value| !value.is_empty()) {
                request = request.header(reqwest::header::USER_AGENT, user_agent);
            }
            request
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|_| TunerHostError::SourceUnavailable)?;
            Ok(())
        } else {
            tokio::fs::File::open(&host.url)
                .await
                .map(|_| ())
                .map_err(|_| TunerHostError::SourceUnavailable)
        }
    }
}

fn parse_compact_uuid(value: &str) -> Option<Uuid> {
    (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| Uuid::parse_str(value).ok())
        .flatten()
}

fn to_info(host: tuner_host::Model) -> TunerHostInfo {
    TunerHostInfo {
        id: Some(host.id.simple().to_string()),
        url: host.url,
        tuner_type: host.tuner_type,
        device_id: host.device_id,
        friendly_name: host.friendly_name,
        import_favorites_only: host.import_favorites_only,
        allow_hw_transcoding: host.allow_hw_transcoding,
        allow_fmp4_transcoding_container: host.allow_fmp4_transcoding_container,
        allow_stream_sharing: host.allow_stream_sharing,
        fallback_max_streaming_bitrate: host.fallback_max_streaming_bitrate,
        enable_stream_looping: host.enable_stream_looping,
        source: host.source,
        tuner_count: host.tuner_count,
        user_agent: host.user_agent,
        ignore_dts: host.ignore_dts,
        read_at_native_framerate: host.read_at_native_framerate,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_compact_uuid;

    #[test]
    fn only_compact_uuid_ids_are_accepted() {
        assert!(parse_compact_uuid("2d350a130bf74b61859cd5e601b5facf").is_some());
        assert!(parse_compact_uuid("2d350a13-0bf7-4b61-859c-d5e601b5facf").is_none());
        assert!(parse_compact_uuid("not-an-id").is_none());
    }
}
