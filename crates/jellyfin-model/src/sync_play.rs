use chrono::{DateTime, Utc};
use serde::Serialize;

/// Response returned by Jellyfin's high-level UTC time sync endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UtcTimeResponse {
    #[serde(with = "crate::serde_datetime::required")]
    pub request_reception_time: DateTime<Utc>,
    #[serde(with = "crate::serde_datetime::required")]
    pub response_transmission_time: DateTime<Utc>,
}

impl UtcTimeResponse {
    #[must_use]
    pub const fn new(
        request_reception_time: DateTime<Utc>,
        response_transmission_time: DateTime<Utc>,
    ) -> Self {
        Self {
            request_reception_time,
            response_transmission_time,
        }
    }
}
