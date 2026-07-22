use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};

/// Error codes returned by the Schedules Direct JSON API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SchedulesDirectErrorCode {
    ServiceOffline = 3000,
    ServiceBusy = 3001,
    AccountExpired = 4001,
    InvalidHash = 4002,
    InvalidUser = 4003,
    AccountTemporaryLock = 4004,
    AccountLocked = 4005,
    TokenExpired = 4006,
    ApplicationLocked = 4007,
    AccountInactive = 4008,
    MaximumLoginAttempts = 4009,
    MaximumIpAttempts = 4010,
    MaximumScheduleRequests = 4100,
    ImageNotFound = 5000,
    MaximumImageDownloads = 5002,
    MaximumImageDownloadsTrial = 5003,
    MaximumInvalidImages = 5004,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenResponse {
    pub code: i32,
    pub message: Option<String>,
    #[serde(rename = "serverID")]
    pub server_id: Option<String>,
    pub token: Option<String>,
    #[serde(rename = "datetime")]
    pub token_timestamp: Option<DateTime<Utc>>,
    pub response: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleRequest {
    #[serde(rename = "stationID")]
    pub station_id: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub date: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleDay {
    #[serde(rename = "stationID")]
    pub station_id: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub programs: Vec<ScheduledProgram>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledProgram {
    #[serde(rename = "programID")]
    pub program_id: Option<String>,
    #[serde(rename = "airDateTime")]
    pub air_date_time: Option<DateTime<Utc>>,
    pub duration: i32,
    pub md5: Option<String>,
    #[serde(
        rename = "audioProperties",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub audio_properties: Vec<String>,
    #[serde(
        rename = "videoProperties",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub video_properties: Vec<String>,
    #[serde(rename = "new")]
    pub is_new: Option<bool>,
    #[serde(rename = "liveTapeDelay")]
    pub live_tape_delay: Option<String>,
    pub premiere: bool,
    pub repeat: bool,
    #[serde(rename = "isPremiereOrFinale")]
    pub premiere_or_finale: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramDetails {
    #[serde(rename = "programID")]
    pub program_id: Option<String>,
    pub audience: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub titles: Vec<ProgramTitle>,
    #[serde(rename = "eventDetails")]
    pub event_details: Option<ProgramEventDetails>,
    pub descriptions: Option<ProgramDescriptions>,
    #[serde(rename = "originalAirDate")]
    pub original_air_date: Option<NaiveDate>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub genres: Vec<String>,
    #[serde(rename = "episodeTitle150")]
    pub episode_title: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub metadata: Vec<ProgramMetadata>,
    #[serde(
        rename = "contentRating",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub content_ratings: Vec<ContentRating>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub cast: Vec<ProgramCredit>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub crew: Vec<ProgramCredit>,
    #[serde(rename = "entityType")]
    pub entity_type: Option<String>,
    #[serde(rename = "showType")]
    pub show_type: Option<String>,
    #[serde(rename = "hasImageArtwork")]
    pub has_image_artwork: bool,
    pub md5: Option<String>,
    pub movie: Option<MovieDetails>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramTitle {
    #[serde(rename = "title120")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramEventDetails {
    #[serde(rename = "subType")]
    pub sub_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramDescriptions {
    #[serde(
        rename = "description100",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub short: Vec<ProgramDescription>,
    #[serde(
        rename = "description1000",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub long: Vec<ProgramDescription>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramDescription {
    #[serde(rename = "descriptionLanguage")]
    pub language: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramMetadata {
    #[serde(rename = "Gracenote")]
    pub gracenote: Option<GracenoteMetadata>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GracenoteMetadata {
    pub season: i32,
    pub episode: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentRating {
    pub body: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MovieDetails {
    pub year: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramCredit {
    #[serde(rename = "personId")]
    pub person_id: Option<String>,
    #[serde(rename = "nameId")]
    pub name_id: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    #[serde(rename = "billingOrder")]
    pub billing_order: Option<String>,
    #[serde(rename = "characterName")]
    pub character_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowImagesResponse {
    #[serde(rename = "programID")]
    pub program_id: Option<String>,
    pub code: Option<i32>,
    pub message: Option<String>,
    #[serde(deserialize_with = "deserialize_image_data")]
    pub data: Vec<ImageData>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageData {
    pub width: Option<String>,
    pub height: Option<String>,
    pub uri: Option<String>,
    pub size: Option<String>,
    pub aspect: Option<String>,
    pub category: Option<String>,
    pub text: Option<String>,
    pub primary: Option<String>,
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Headend {
    pub headend: Option<String>,
    pub transport: Option<String>,
    pub location: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub lineups: Vec<Lineup>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lineup {
    pub lineup: Option<String>,
    pub name: Option<String>,
    pub transport: Option<String>,
    pub location: Option<String>,
    pub uri: Option<String>,
    #[serde(rename = "isDeleted")]
    pub is_deleted: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineupsResponse {
    pub code: i32,
    #[serde(rename = "serverID")]
    pub server_id: Option<String>,
    #[serde(rename = "datetime")]
    pub lineup_timestamp: Option<DateTime<Utc>>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub lineups: Vec<Lineup>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelLineupResponse {
    #[serde(rename = "map", deserialize_with = "deserialize_vec_or_default")]
    pub channel_map: Vec<ChannelMap>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub stations: Vec<Station>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMap {
    #[serde(rename = "stationID")]
    pub station_id: Option<String>,
    pub channel: Option<String>,
    #[serde(rename = "providerCallsign")]
    pub provider_callsign: Option<String>,
    #[serde(rename = "logicalChannelNumber")]
    pub logical_channel_number: Option<String>,
    #[serde(rename = "uhfVhf")]
    pub uhf_vhf: i32,
    #[serde(rename = "atscMajor")]
    pub atsc_major: i32,
    #[serde(rename = "atscMinor")]
    pub atsc_minor: i32,
    #[serde(rename = "matchType")]
    pub match_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    #[serde(rename = "stationID")]
    pub station_id: Option<String>,
    pub name: Option<String>,
    pub callsign: Option<String>,
    #[serde(
        rename = "broadcastLanguage",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub broadcast_language: Vec<String>,
    #[serde(
        rename = "descriptionLanguage",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub description_language: Vec<String>,
    pub affiliate: Option<String>,
    pub logo: Option<StationLogo>,
    #[serde(rename = "isCommercialFree")]
    pub is_commercial_free: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StationLogo {
    #[serde(rename = "URL")]
    pub url: Option<String>,
    pub height: i32,
    pub width: i32,
    pub md5: Option<String>,
}

fn deserialize_vec_or_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn deserialize_image_data<'de, D>(deserializer: D) -> Result<Vec<ImageData>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Data {
        Images(Vec<ImageData>),
        Other(serde_json::Value),
    }

    match Option::<Data>::deserialize(deserializer)? {
        Some(Data::Images(images)) => Ok(images),
        Some(Data::Other(other)) => {
            drop(other);
            Ok(Vec::new())
        }
        None => Ok(Vec::new()),
    }
}
