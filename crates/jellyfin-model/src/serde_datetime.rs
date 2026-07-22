use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Deserializer, Serializer};

/// Matches Jellyfin's legacy `JsonDateTimeConverter`, including its seven
/// fractional digits for timestamps that fall on a whole millisecond.
pub(crate) mod option {
    use super::*;

    pub fn serialize<S>(value: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => {
                let ticks = value.nanosecond() / 100;
                let mut output = format!("{}.{ticks:07}Z", value.format("%Y-%m-%dT%H:%M:%S"));

                if value.timestamp_subsec_millis() != 0 {
                    let z = output.pop().expect("timestamp always ends in Z");
                    while output.ends_with('0') {
                        output.pop();
                    }
                    output.push(z);
                }

                serializer.serialize_str(&output)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(DateTime::into)
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}
