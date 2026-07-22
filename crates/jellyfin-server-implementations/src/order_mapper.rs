use chrono::{DateTime, TimeZone, Utc};
use thiserror::Error;

use crate::PremiereDateOrderKey;

/// Invalid data encountered while projecting an item order value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OrderMappingError {
    #[error("production year {0} cannot be represented as a Jellyfin date")]
    InvalidProductionYear(i32),
}

/// Projects query item fields into values used by Jellyfin ordering.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderMapper;

impl OrderMapper {
    /// Maps premiere-date ordering to the item's premiere date or January 1 of
    /// its production year. A missing date and year map to `None`.
    ///
    /// # Errors
    ///
    /// Returns [`OrderMappingError::InvalidProductionYear`] when the fallback
    /// year is outside the range supported by Jellyfin's `DateTime` contract.
    pub fn premiere_date_order_value(
        item: &PremiereDateOrderKey,
    ) -> Result<Option<DateTime<Utc>>, OrderMappingError> {
        if let Some(premiere_date) = item.premiere_date {
            return Ok(Some(premiere_date));
        }

        let Some(production_year) = item.production_year else {
            return Ok(None);
        };
        if !(1..=9999).contains(&production_year) {
            return Err(OrderMappingError::InvalidProductionYear(production_year));
        }

        Utc.with_ymd_and_hms(production_year, 1, 1, 0, 0, 0)
            .single()
            .map(Some)
            .ok_or(OrderMappingError::InvalidProductionYear(production_year))
    }
}
