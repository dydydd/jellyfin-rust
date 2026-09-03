use reqwest::StatusCode;
use thiserror::Error;

use super::schedules_direct::{
    ChannelLineupResponse, LineupsResponse, ProgramDetails, ScheduleDay, ScheduleRequest,
    TokenResponse,
};

const DEFAULT_BASE_URL: &str = "https://json.schedulesdirect.org/20141201";
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Failures while talking to the Schedules Direct JSON API.
#[derive(Debug, Error)]
pub enum SchedulesDirectClientError {
    #[error("Schedules Direct returned HTTP {0}")]
    Http(StatusCode),
    #[error("Schedules Direct request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Schedules Direct returned invalid JSON: {0}")]
    Decode(#[from] serde_json::Error),
}

/// Minimal Schedules Direct JSON API client.
#[derive(Clone, Debug)]
pub struct SchedulesDirectClient {
    http: reqwest::Client,
    base_url: String,
}

impl Default for SchedulesDirectClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulesDirectClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("Schedules Direct HTTP client must build"),
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    #[must_use]
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("Schedules Direct HTTP client must build"),
            base_url: base_url.into(),
        }
    }

    /// Exchanges Schedules Direct credentials for a token.
    ///
    /// # Errors
    ///
    /// Returns a request or HTTP error when the service cannot be reached.
    pub async fn token(
        &self,
        username: &str,
        password: &str,
    ) -> Result<TokenResponse, SchedulesDirectClientError> {
        let body = serde_json::json!({ "username": username, "password": password });
        let response = self
            .http
            .post(format!("{}/token", self.base_url))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SchedulesDirectClientError::Http(status));
        }
        Ok(response.json().await?)
    }

    /// Lists lineups available to the authenticated account.
    ///
    /// # Errors
    ///
    /// Returns a request or HTTP error when the service cannot be reached.
    pub async fn lineups(
        &self,
        token: &str,
    ) -> Result<LineupsResponse, SchedulesDirectClientError> {
        self.get_with_token("/lineups", token).await
    }

    /// Loads the station map and station details for one lineup.
    ///
    /// # Errors
    ///
    /// Returns a request or HTTP error when the service cannot be reached.
    pub async fn channel_lineup(
        &self,
        token: &str,
        lineup_id: &str,
    ) -> Result<ChannelLineupResponse, SchedulesDirectClientError> {
        self.get_with_token(&format!("/lineups/{lineup_id}"), token)
            .await
    }

    /// Fetches scheduled programs for one or more stations and dates.
    ///
    /// # Errors
    ///
    /// Returns a request or HTTP error when the service cannot be reached.
    pub async fn schedules(
        &self,
        token: &str,
        requests: &[ScheduleRequest],
    ) -> Result<Vec<ScheduleDay>, SchedulesDirectClientError> {
        let response = self
            .http
            .post(format!("{}/schedules", self.base_url))
            .header("token", token)
            .json(requests)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SchedulesDirectClientError::Http(status));
        }
        Ok(response.json().await?)
    }

    pub(crate) async fn schedules_for_stations<'a>(
        &self,
        token: &str,
        station_ids: impl IntoIterator<Item = &'a str>,
        dates: &'a [String],
    ) -> Result<Vec<ScheduleDay>, SchedulesDirectClientError> {
        #[derive(serde::Serialize)]
        struct BorrowedScheduleRequest<'a> {
            #[serde(rename = "stationID")]
            station_id: &'a str,
            date: &'a [String],
        }

        let requests = station_ids
            .into_iter()
            .map(|station_id| BorrowedScheduleRequest {
                station_id,
                date: dates,
            })
            .collect::<Vec<_>>();
        let response = self
            .http
            .post(format!("{}/schedules", self.base_url))
            .header("token", token)
            .json(&requests)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SchedulesDirectClientError::Http(status));
        }
        Ok(response.json().await?)
    }

    /// Fetches program metadata for one or more program identifiers.
    ///
    /// # Errors
    ///
    /// Returns a request or HTTP error when the service cannot be reached.
    pub async fn programs(
        &self,
        token: &str,
        program_ids: &[String],
    ) -> Result<Vec<ProgramDetails>, SchedulesDirectClientError> {
        let response = self
            .http
            .post(format!("{}/programs", self.base_url))
            .header("token", token)
            .json(program_ids)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SchedulesDirectClientError::Http(status));
        }
        Ok(response.json().await?)
    }

    async fn get_with_token<T>(
        &self,
        path: &str,
        token: &str,
    ) -> Result<T, SchedulesDirectClientError>
    where
        T: serde::de::DeserializeOwned,
    {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .header("token", token)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(SchedulesDirectClientError::Http(status));
        }
        Ok(response.json().await?)
    }
}
