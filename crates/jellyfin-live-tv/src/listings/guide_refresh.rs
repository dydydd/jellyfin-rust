use std::{collections::HashSet, sync::Arc};

use chrono::{Duration, Utc};
use thiserror::Error;

use super::{
    LineupsResponse, ListingsConfigurationError, ListingsConfigurationStore, ScheduleRequest,
    SchedulesDirectClient, SchedulesDirectClientError,
};

/// Result of one Schedules Direct guide refresh.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct GuideRefreshSummary {
    pub provider_id: Option<String>,
    pub lineup_id: String,
    pub channels: usize,
    pub programs: usize,
    pub refreshed_at: chrono::DateTime<Utc>,
}

/// Failures while refreshing a Schedules Direct guide.
#[derive(Debug, Error)]
pub enum GuideRefreshError {
    #[error("no Schedules Direct listings provider is configured")]
    NoProvider,
    #[error("the Schedules Direct provider is missing credentials or a lineup")]
    InvalidProviderConfiguration,
    #[error("Schedules Direct returned no token")]
    MissingToken,
    #[error(transparent)]
    Configuration(#[from] ListingsConfigurationError),
    #[error(transparent)]
    Client(#[from] SchedulesDirectClientError),
}

/// Refreshes guide data from configured Schedules Direct providers.
#[derive(Clone)]
pub struct GuideRefreshService {
    store: Arc<dyn ListingsConfigurationStore>,
    client: SchedulesDirectClient,
}

impl GuideRefreshService {
    #[must_use]
    pub fn new(
        store: Arc<dyn ListingsConfigurationStore>,
        client: SchedulesDirectClient,
    ) -> Self {
        Self { store, client }
    }

    /// Runs the complete token, lineup, schedule, and program refresh chain.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no provider is configured, or a
    /// Schedules Direct error when any API call fails.
    pub async fn refresh(&self) -> Result<GuideRefreshSummary, GuideRefreshError> {
        let configuration = self.store.load()?;
        let provider = configuration
            .listing_providers
            .iter()
            .find(|provider| {
                provider
                    .provider_type
                    .as_deref()
                    .is_some_and(|provider_type| {
                        provider_type.eq_ignore_ascii_case("SchedulesDirect")
                    })
            })
            .ok_or(GuideRefreshError::NoProvider)?;
        let username = provider
            .username
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(GuideRefreshError::InvalidProviderConfiguration)?;
        let password = provider
            .password
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(GuideRefreshError::InvalidProviderConfiguration)?;
        let lineup_id = provider
            .listings_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(GuideRefreshError::InvalidProviderConfiguration)?;

        let token = self
            .client
            .token(username, password)
            .await?
            .token
            .filter(|token| !token.is_empty())
            .ok_or(GuideRefreshError::MissingToken)?;
        let lineup = self.client.channel_lineup(&token, lineup_id).await?;

        let guide_days = configuration.guide_days.unwrap_or(3).clamp(1, 14);
        let dates = (0..guide_days)
            .map(|day| {
                (Utc::now().date_naive() + Duration::days(i64::from(day)))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect::<Vec<_>>();
        let station_ids = lineup
            .channel_map
            .iter()
            .filter_map(|channel| channel.station_id.clone())
            .collect::<HashSet<_>>();
        let schedules = self
            .client
            .schedules(
                &token,
                &station_ids
                    .iter()
                    .map(|station_id| ScheduleRequest {
                        station_id: Some(station_id.clone()),
                        date: dates.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
            .await?;
        let program_count = schedules
            .iter()
            .map(|day| day.programs.len())
            .sum::<usize>();

        let program_ids = schedules
            .iter()
            .flat_map(|day| day.programs.iter())
            .filter_map(|program| program.program_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !program_ids.is_empty() {
            let _programs = self.client.programs(&token, &program_ids).await?;
        }

        Ok(GuideRefreshSummary {
            provider_id: provider.id.clone(),
            lineup_id: lineup_id.to_owned(),
            channels: station_ids.len(),
            programs: program_count,
            refreshed_at: Utc::now(),
        })
    }

    /// Lists the Schedules Direct lineups for the configured account.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no provider is configured, or a
    /// Schedules Direct error when the API call fails.
    pub async fn lineups(&self) -> Result<LineupsResponse, GuideRefreshError> {
        let configuration = self.store.load()?;
        let provider = configuration
            .listing_providers
            .iter()
            .find(|provider| {
                provider
                    .provider_type
                    .as_deref()
                    .is_some_and(|provider_type| {
                        provider_type.eq_ignore_ascii_case("SchedulesDirect")
                    })
            })
            .ok_or(GuideRefreshError::NoProvider)?;
        let username = provider
            .username
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(GuideRefreshError::InvalidProviderConfiguration)?;
        let password = provider
            .password
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(GuideRefreshError::InvalidProviderConfiguration)?;
        let token = self
            .client
            .token(username, password)
            .await?
            .token
            .filter(|token| !token.is_empty())
            .ok_or(GuideRefreshError::MissingToken)?;
        self.client.lineups(&token).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_without_a_provider_is_typed() {
        let service = GuideRefreshService::new(
            Arc::new(super::super::MemoryListingsConfigurationStore::default()),
            SchedulesDirectClient::new(),
        );
        assert!(matches!(
            service.refresh().await,
            Err(GuideRefreshError::NoProvider)
        ));
    }
}
