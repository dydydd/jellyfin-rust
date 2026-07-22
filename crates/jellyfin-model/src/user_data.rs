use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Fields accepted by Jellyfin's generic per-item user-data update endpoint.
///
/// An absent field and an explicit JSON `null` are both represented by
/// `None`; the endpoint treats both forms as "leave the stored value alone".
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateUserItemDataDto {
    #[serde(alias = "rating")]
    pub rating: Option<f64>,
    #[serde(alias = "playedPercentage")]
    pub played_percentage: Option<f64>,
    #[serde(alias = "unplayedItemCount")]
    pub unplayed_item_count: Option<i32>,
    #[serde(alias = "playbackPositionTicks")]
    pub playback_position_ticks: Option<i64>,
    #[serde(alias = "playCount")]
    pub play_count: Option<i32>,
    #[serde(alias = "isFavorite")]
    pub is_favorite: Option<bool>,
    #[serde(alias = "likes")]
    pub likes: Option<bool>,
    #[serde(alias = "lastPlayedDate")]
    pub last_played_date: Option<DateTime<Utc>>,
    #[serde(alias = "played")]
    pub played: Option<bool>,
    #[serde(alias = "key")]
    pub key: Option<String>,
    #[serde(alias = "itemId")]
    pub item_id: Option<String>,
}

/// API representation of per-user state for one library item.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub played_percentage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unplayed_item_count: Option<i32>,
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub likes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played_date: Option<String>,
    pub played: bool,
    pub key: String,
    pub item_id: String,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::UpdateUserItemDataDto;

    #[test]
    fn update_dto_deserializes_pascal_case_fields_and_utc_date() {
        let dto: UpdateUserItemDataDto = serde_json::from_value(json!({
            "Rating": 6.5,
            "PlayedPercentage": 75.0,
            "UnplayedItemCount": 2,
            "PlaybackPositionTicks": 123,
            "PlayCount": 4,
            "IsFavorite": true,
            "Likes": false,
            "LastPlayedDate": "2026-07-22T09:10:11Z",
            "Played": true,
            "Key": "ignored-key",
            "ItemId": "ignored-item"
        }))
        .expect("official update body must deserialize");

        assert_eq!(dto.rating, Some(6.5));
        assert_eq!(dto.played_percentage, Some(75.0));
        assert_eq!(dto.unplayed_item_count, Some(2));
        assert_eq!(dto.playback_position_ticks, Some(123));
        assert_eq!(dto.play_count, Some(4));
        assert_eq!(dto.is_favorite, Some(true));
        assert_eq!(dto.likes, Some(false));
        assert_eq!(
            dto.last_played_date,
            Some(Utc.with_ymd_and_hms(2026, 7, 22, 9, 10, 11).unwrap())
        );
        assert_eq!(dto.played, Some(true));
        assert_eq!(dto.key.as_deref(), Some("ignored-key"));
        assert_eq!(dto.item_id.as_deref(), Some("ignored-item"));
    }

    #[test]
    fn update_dto_treats_missing_and_null_as_no_update() {
        let missing: UpdateUserItemDataDto =
            serde_json::from_value(json!({})).expect("empty object must deserialize");
        let nulls: UpdateUserItemDataDto = serde_json::from_value(json!({
            "Rating": null,
            "PlaybackPositionTicks": null,
            "LastPlayedDate": null,
            "Likes": null
        }))
        .expect("nullable fields must deserialize");

        assert_eq!(missing, UpdateUserItemDataDto::default());
        assert_eq!(nulls, UpdateUserItemDataDto::default());
    }

    #[test]
    fn update_dto_accepts_standard_camel_case_input() {
        let dto: UpdateUserItemDataDto = serde_json::from_value(json!({
            "rating": 5.0,
            "playedPercentage": 10.0,
            "unplayedItemCount": 3,
            "playbackPositionTicks": 22,
            "playCount": 2,
            "isFavorite": true,
            "likes": false,
            "lastPlayedDate": "2026-07-22T09:10:11Z",
            "played": true,
            "key": "key",
            "itemId": "item"
        }))
        .expect("camelCase update body must deserialize");

        assert_eq!(dto.rating, Some(5.0));
        assert_eq!(dto.played_percentage, Some(10.0));
        assert_eq!(dto.unplayed_item_count, Some(3));
        assert_eq!(dto.playback_position_ticks, Some(22));
        assert_eq!(dto.play_count, Some(2));
        assert_eq!(dto.is_favorite, Some(true));
        assert_eq!(dto.likes, Some(false));
        assert!(dto.last_played_date.is_some());
        assert_eq!(dto.played, Some(true));
        assert_eq!(dto.key.as_deref(), Some("key"));
        assert_eq!(dto.item_id.as_deref(), Some("item"));
    }
}
