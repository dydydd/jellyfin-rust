use serde::Serialize;

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
