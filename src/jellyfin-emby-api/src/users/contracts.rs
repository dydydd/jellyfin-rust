use std::{fmt, marker::PhantomData, str::FromStr};

use jellyfin_model::{
    AccessSchedule, DynamicDayOfWeek, SubtitlePlaybackMode, UnratedItem, UserConfiguration,
    UserPolicy,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use uuid::Uuid;

pub(super) const CONFIGURATION_STORAGE_KEY: &str = "EmbyUserConfiguration";
pub(super) const POLICY_STORAGE_KEY: &str = "EmbyUserPolicy";

const CONFIGURATION_FIELDS: &[&str] = &[
    "AudioLanguagePreference",
    "PlayDefaultAudioTrack",
    "SubtitleLanguagePreference",
    "ProfilePin",
    "DisplayMissingEpisodes",
    "SubtitleMode",
    "OrderedViews",
    "LatestItemsExcludes",
    "MyMediaExcludes",
    "HidePlayedInLatest",
    "HidePlayedInMoreLikeThis",
    "HidePlayedInSuggestions",
    "RememberAudioSelections",
    "RememberSubtitleSelections",
    "EnableNextEpisodeAutoPlay",
    "ResumeRewindSeconds",
    "IntroSkipMode",
    "EnableLocalPassword",
];

const POLICY_FIELDS: &[&str] = &[
    "IsAdministrator",
    "IsHidden",
    "IsHiddenRemotely",
    "IsHiddenFromUnusedDevices",
    "IsDisabled",
    "LockedOutDate",
    "MaxParentalRating",
    "AllowTagOrRating",
    "BlockedTags",
    "IsTagBlockingModeInclusive",
    "IncludeTags",
    "EnableUserPreferenceAccess",
    "AccessSchedules",
    "BlockUnratedItems",
    "EnableRemoteControlOfOtherUsers",
    "EnableSharedDeviceControl",
    "EnableRemoteAccess",
    "EnableLiveTvManagement",
    "EnableLiveTvAccess",
    "EnableMediaPlayback",
    "EnableAudioPlaybackTranscoding",
    "EnableVideoPlaybackTranscoding",
    "EnableTranscodingQuality",
    "AutoRemoteQuality",
    "EnablePlaybackRemuxing",
    "EnableContentDeletion",
    "RestrictedFeatures",
    "EnableContentDeletionFromFolders",
    "EnableContentDownloading",
    "EnableSubtitleDownloading",
    "EnableSubtitleManagement",
    "EnableSyncTranscoding",
    "EnableMediaConversion",
    "EnabledChannels",
    "EnableAllChannels",
    "EnabledFolders",
    "EnableAllFolders",
    "InvalidLoginAttemptCount",
    "EnablePublicSharing",
    "RemoteClientBitrateLimit",
    "AuthenticationProviderId",
    "ExcludedSubFolders",
    "SimultaneousStreamLimit",
    "EnabledDevices",
    "EnableAllDevices",
    "AllowCameraUpload",
    "AllowSharingPersonalItems",
];

fn deserialize_case_insensitive_object<'de, D, T>(
    deserializer: D,
    fields: &'static [&'static str],
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    struct Visitor<T> {
        fields: &'static [&'static str],
        marker: PhantomData<T>,
    }

    impl<'de, T> de::Visitor<'de> for Visitor<T>
    where
        T: serde::de::DeserializeOwned,
    {
        type Value = T;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an Emby user settings object")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: de::MapAccess<'de>,
        {
            let mut normalized = serde_json::Map::new();
            while let Some(key) = map.next_key::<String>()? {
                let value = map.next_value::<Value>()?;
                if let Some(canonical) = self
                    .fields
                    .iter()
                    .find(|field| field.eq_ignore_ascii_case(&key))
                {
                    // System.Text.Json's web defaults bind properties without
                    // regard to case and retain the last duplicate value.
                    normalized.insert((*canonical).to_owned(), value);
                }
            }
            serde_json::from_value(Value::Object(normalized)).map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_map(Visitor {
        fields,
        marker: PhantomData,
    })
}

fn deserialize_optional_number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + FromStr,
    T::Err: fmt::Display,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    value
        .map(|value| match value {
            Value::String(value) => value.parse().map_err(de::Error::custom),
            value => serde_json::from_value(value).map_err(de::Error::custom),
        })
        .transpose()
}

fn deserialize_number<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + FromStr,
    T::Err: fmt::Display,
{
    match Value::deserialize(deserializer)? {
        Value::String(value) => value.parse().map_err(de::Error::custom),
        value => serde_json::from_value(value).map_err(de::Error::custom),
    }
}

fn deserialize_nullable_i32<'de, D>(deserializer: D) -> Result<NullableField<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_optional_number(deserializer).map(|value| match value {
        Some(value) => NullableField::Value(value),
        None => NullableField::Null,
    })
}

macro_rules! emby_enum {
    ($name:ident { $($variant:ident = ($number:literal, $wire:literal)),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        pub(super) enum $name {
            $($variant),+
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct EnumVisitor;

                impl<'de> de::Visitor<'de> for EnumVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a defined ", stringify!($name), " name or integer"))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $(if value.eq_ignore_ascii_case($wire) {
                            return Ok($name::$variant);
                        })+
                        Err(E::unknown_variant(value, &[$($wire),+]))
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($number => Ok($name::$variant),)+
                            _ => Err(E::invalid_value(de::Unexpected::Signed(value), &self)),
                        }
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        i64::try_from(value)
                            .map_err(|_| E::invalid_value(de::Unexpected::Unsigned(value), &self))
                            .and_then(|value| self.visit_i64(value))
                    }
                }

                deserializer.deserialize_any(EnumVisitor)
            }
        }

    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NullableField<T> {
    Missing,
    Null,
    Value(T),
}

impl<T> Serialize for NullableField<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}

impl<T> Default for NullableField<T> {
    fn default() -> Self {
        Self::Missing
    }
}

impl<'de, T> Deserialize<'de> for NullableField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

impl<T> NullableField<T> {
    const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

emby_enum!(EmbySubtitlePlaybackMode {
    Default = (0, "Default"),
    Always = (1, "Always"),
    OnlyForced = (2, "OnlyForced"),
    None = (3, "None"),
    Smart = (4, "Smart"),
    HearingImpaired = (5, "HearingImpaired"),
});

emby_enum!(EmbySegmentSkipMode {
    ShowButton = (0, "ShowButton"),
    AutoSkip = (1, "AutoSkip"),
    None = (2, "None"),
});

emby_enum!(EmbyDynamicDayOfWeek {
    Sunday = (0, "Sunday"),
    Monday = (1, "Monday"),
    Tuesday = (2, "Tuesday"),
    Wednesday = (3, "Wednesday"),
    Thursday = (4, "Thursday"),
    Friday = (5, "Friday"),
    Saturday = (6, "Saturday"),
    Everyday = (7, "Everyday"),
    Weekday = (8, "Weekday"),
    Weekend = (9, "Weekend"),
});

emby_enum!(EmbyUnratedItem {
    Movie = (0, "Movie"),
    Trailer = (1, "Trailer"),
    Series = (2, "Series"),
    Music = (3, "Music"),
    Game = (4, "Game"),
    Book = (5, "Book"),
    LiveTvChannel = (6, "LiveTvChannel"),
    LiveTvProgram = (7, "LiveTvProgram"),
    ChannelContent = (8, "ChannelContent"),
    Other = (9, "Other"),
});

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct EmbyUserConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_language_preference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_default_audio_track: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_language_preference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_missing_episodes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_mode: Option<EmbySubtitlePlaybackMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordered_views: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_items_excludes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_media_excludes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_played_in_latest: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_played_in_more_like_this: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_played_in_suggestions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_audio_selections: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember_subtitle_selections: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_next_episode_auto_play: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_rewind_seconds: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro_skip_mode: Option<EmbySegmentSkipMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_local_password: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EmbyUserConfigurationFields {
    audio_language_preference: Option<String>,
    play_default_audio_track: Option<bool>,
    subtitle_language_preference: Option<String>,
    profile_pin: Option<String>,
    display_missing_episodes: Option<bool>,
    subtitle_mode: Option<EmbySubtitlePlaybackMode>,
    ordered_views: Option<Vec<String>>,
    latest_items_excludes: Option<Vec<String>>,
    my_media_excludes: Option<Vec<String>>,
    hide_played_in_latest: Option<bool>,
    hide_played_in_more_like_this: Option<bool>,
    hide_played_in_suggestions: Option<bool>,
    remember_audio_selections: Option<bool>,
    remember_subtitle_selections: Option<bool>,
    enable_next_episode_auto_play: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    resume_rewind_seconds: Option<i32>,
    intro_skip_mode: Option<EmbySegmentSkipMode>,
    enable_local_password: Option<bool>,
}

impl From<EmbyUserConfigurationFields> for EmbyUserConfiguration {
    fn from(value: EmbyUserConfigurationFields) -> Self {
        Self {
            audio_language_preference: value.audio_language_preference,
            play_default_audio_track: value.play_default_audio_track,
            subtitle_language_preference: value.subtitle_language_preference,
            profile_pin: value.profile_pin,
            display_missing_episodes: value.display_missing_episodes,
            subtitle_mode: value.subtitle_mode,
            ordered_views: value.ordered_views,
            latest_items_excludes: value.latest_items_excludes,
            my_media_excludes: value.my_media_excludes,
            hide_played_in_latest: value.hide_played_in_latest,
            hide_played_in_more_like_this: value.hide_played_in_more_like_this,
            hide_played_in_suggestions: value.hide_played_in_suggestions,
            remember_audio_selections: value.remember_audio_selections,
            remember_subtitle_selections: value.remember_subtitle_selections,
            enable_next_episode_auto_play: value.enable_next_episode_auto_play,
            resume_rewind_seconds: value.resume_rewind_seconds,
            intro_skip_mode: value.intro_skip_mode,
            enable_local_password: value.enable_local_password,
        }
    }
}

impl<'de> Deserialize<'de> for EmbyUserConfiguration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_case_insensitive_object::<_, EmbyUserConfigurationFields>(
            deserializer,
            CONFIGURATION_FIELDS,
        )
        .map(Into::into)
    }
}

impl EmbyUserConfiguration {
    pub(super) fn apply_to_shared(&self, shared: &mut UserConfiguration) {
        if let Some(value) = &self.audio_language_preference {
            shared.audio_language_preference = Some(value.clone());
        }
        if let Some(value) = self.play_default_audio_track {
            shared.play_default_audio_track = value;
        }
        if let Some(value) = &self.subtitle_language_preference {
            shared.subtitle_language_preference = Some(value.clone());
        }
        if let Some(value) = self.display_missing_episodes {
            shared.display_missing_episodes = value;
        }
        if let Some(value) = self.subtitle_mode.and_then(shared_subtitle_mode) {
            shared.subtitle_mode = value;
        }
        if let Some(value) = &self.ordered_views {
            shared.ordered_views = parse_uuids(value);
        }
        if let Some(value) = &self.latest_items_excludes {
            shared.latest_items_excludes = parse_uuids(value);
        }
        if let Some(value) = &self.my_media_excludes {
            shared.my_media_excludes = parse_uuids(value);
        }
        if let Some(value) = self.hide_played_in_latest {
            shared.hide_played_in_latest = value;
        }
        if let Some(value) = self.remember_audio_selections {
            shared.remember_audio_selections = value;
        }
        if let Some(value) = self.remember_subtitle_selections {
            shared.remember_subtitle_selections = value;
        }
        if let Some(value) = self.enable_next_episode_auto_play {
            shared.enable_next_episode_auto_play = value;
        }
        if let Some(value) = self.enable_local_password {
            shared.enable_local_password = value;
        }
    }

    pub(super) fn from_storage(preferences: &Value, enable_local_password: bool) -> Self {
        let mut shared =
            serde_json::from_value::<UserConfiguration>(preferences.clone()).unwrap_or_default();
        shared.enable_local_password = enable_local_password;
        if let Some(mut stored) = preferences
            .get(CONFIGURATION_STORAGE_KEY)
            .and_then(|value| serde_json::from_value::<EmbyUserConfiguration>(value.clone()).ok())
        {
            stored.overlay_shared(&shared);
            return stored;
        }
        Self::from_shared(&shared)
    }

    fn overlay_shared(&mut self, shared: &UserConfiguration) {
        self.audio_language_preference = shared.audio_language_preference.clone();
        self.play_default_audio_track = Some(shared.play_default_audio_track);
        self.subtitle_language_preference = shared.subtitle_language_preference.clone();
        self.display_missing_episodes = Some(shared.display_missing_episodes);
        // HearingImpaired has no Jellyfin representation. Retain that Emby-only
        // value instead of replacing it with the unchanged shared fallback.
        if self.subtitle_mode != Some(EmbySubtitlePlaybackMode::HearingImpaired) {
            self.subtitle_mode = Some(emby_subtitle_mode(shared.subtitle_mode));
        }
        self.ordered_views = Some(overlay_uuid_strings(
            self.ordered_views.as_deref(),
            &shared.ordered_views,
        ));
        self.latest_items_excludes = Some(overlay_uuid_strings(
            self.latest_items_excludes.as_deref(),
            &shared.latest_items_excludes,
        ));
        self.my_media_excludes = Some(overlay_uuid_strings(
            self.my_media_excludes.as_deref(),
            &shared.my_media_excludes,
        ));
        self.hide_played_in_latest = Some(shared.hide_played_in_latest);
        self.remember_audio_selections = Some(shared.remember_audio_selections);
        self.remember_subtitle_selections = Some(shared.remember_subtitle_selections);
        self.enable_next_episode_auto_play = Some(shared.enable_next_episode_auto_play);
        self.enable_local_password = Some(shared.enable_local_password);
    }

    fn from_shared(shared: &UserConfiguration) -> Self {
        Self {
            audio_language_preference: shared.audio_language_preference.clone(),
            play_default_audio_track: Some(shared.play_default_audio_track),
            subtitle_language_preference: shared.subtitle_language_preference.clone(),
            profile_pin: None,
            display_missing_episodes: Some(shared.display_missing_episodes),
            subtitle_mode: Some(emby_subtitle_mode(shared.subtitle_mode)),
            ordered_views: Some(format_uuids(&shared.ordered_views)),
            latest_items_excludes: Some(format_uuids(&shared.latest_items_excludes)),
            my_media_excludes: Some(format_uuids(&shared.my_media_excludes)),
            hide_played_in_latest: Some(shared.hide_played_in_latest),
            hide_played_in_more_like_this: Some(false),
            hide_played_in_suggestions: Some(false),
            remember_audio_selections: Some(shared.remember_audio_selections),
            remember_subtitle_selections: Some(shared.remember_subtitle_selections),
            enable_next_episode_auto_play: Some(shared.enable_next_episode_auto_play),
            resume_rewind_seconds: Some(0),
            intro_skip_mode: Some(EmbySegmentSkipMode::ShowButton),
            enable_local_password: Some(shared.enable_local_password),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct EmbyAccessSchedule {
    pub day_of_week: EmbyDynamicDayOfWeek,
    #[serde(deserialize_with = "deserialize_number")]
    pub start_hour: f64,
    #[serde(deserialize_with = "deserialize_number")]
    pub end_hour: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct EmbyUserPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_administrator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hidden_remotely: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_hidden_from_unused_devices: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_out_date: Option<i64>,
    #[serde(skip_serializing_if = "NullableField::is_missing")]
    pub max_parental_rating: NullableField<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_tag_or_rating: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_tag_blocking_mode_inclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_user_preference_access: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_schedules: Option<Vec<EmbyAccessSchedule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_unrated_items: Option<Vec<EmbyUnratedItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_remote_control_of_other_users: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_shared_device_control: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_remote_access: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_live_tv_management: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_live_tv_access: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_media_playback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_audio_playback_transcoding: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_video_playback_transcoding: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_transcoding_quality: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_remote_quality: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_playback_remuxing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_content_deletion: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restricted_features: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_content_deletion_from_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_content_downloading: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_subtitle_downloading: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_subtitle_management: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_sync_transcoding: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_media_conversion: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_channels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_all_channels: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_all_folders: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_login_attempt_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_public_sharing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_client_bitrate_limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded_sub_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simultaneous_stream_limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_devices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_all_devices: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_camera_upload: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_sharing_personal_items: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct EmbyUserPolicyFields {
    is_administrator: Option<bool>,
    is_hidden: Option<bool>,
    is_hidden_remotely: Option<bool>,
    is_hidden_from_unused_devices: Option<bool>,
    is_disabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    locked_out_date: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_nullable_i32")]
    max_parental_rating: NullableField<i32>,
    allow_tag_or_rating: Option<bool>,
    blocked_tags: Option<Vec<String>>,
    is_tag_blocking_mode_inclusive: Option<bool>,
    include_tags: Option<Vec<String>>,
    enable_user_preference_access: Option<bool>,
    access_schedules: Option<Vec<EmbyAccessSchedule>>,
    block_unrated_items: Option<Vec<EmbyUnratedItem>>,
    enable_remote_control_of_other_users: Option<bool>,
    enable_shared_device_control: Option<bool>,
    enable_remote_access: Option<bool>,
    enable_live_tv_management: Option<bool>,
    enable_live_tv_access: Option<bool>,
    enable_media_playback: Option<bool>,
    enable_audio_playback_transcoding: Option<bool>,
    enable_video_playback_transcoding: Option<bool>,
    enable_transcoding_quality: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    auto_remote_quality: Option<i32>,
    enable_playback_remuxing: Option<bool>,
    enable_content_deletion: Option<bool>,
    restricted_features: Option<Vec<String>>,
    enable_content_deletion_from_folders: Option<Vec<String>>,
    enable_content_downloading: Option<bool>,
    enable_subtitle_downloading: Option<bool>,
    enable_subtitle_management: Option<bool>,
    enable_sync_transcoding: Option<bool>,
    enable_media_conversion: Option<bool>,
    enabled_channels: Option<Vec<String>>,
    enable_all_channels: Option<bool>,
    enabled_folders: Option<Vec<String>>,
    enable_all_folders: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    invalid_login_attempt_count: Option<i32>,
    enable_public_sharing: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    remote_client_bitrate_limit: Option<i32>,
    authentication_provider_id: Option<String>,
    excluded_sub_folders: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    simultaneous_stream_limit: Option<i32>,
    enabled_devices: Option<Vec<String>>,
    enable_all_devices: Option<bool>,
    allow_camera_upload: Option<bool>,
    allow_sharing_personal_items: Option<bool>,
}

impl From<EmbyUserPolicyFields> for EmbyUserPolicy {
    fn from(value: EmbyUserPolicyFields) -> Self {
        Self {
            is_administrator: value.is_administrator,
            is_hidden: value.is_hidden,
            is_hidden_remotely: value.is_hidden_remotely,
            is_hidden_from_unused_devices: value.is_hidden_from_unused_devices,
            is_disabled: value.is_disabled,
            locked_out_date: value.locked_out_date,
            max_parental_rating: value.max_parental_rating,
            allow_tag_or_rating: value.allow_tag_or_rating,
            blocked_tags: value.blocked_tags,
            is_tag_blocking_mode_inclusive: value.is_tag_blocking_mode_inclusive,
            include_tags: value.include_tags,
            enable_user_preference_access: value.enable_user_preference_access,
            access_schedules: value.access_schedules,
            block_unrated_items: value.block_unrated_items,
            enable_remote_control_of_other_users: value.enable_remote_control_of_other_users,
            enable_shared_device_control: value.enable_shared_device_control,
            enable_remote_access: value.enable_remote_access,
            enable_live_tv_management: value.enable_live_tv_management,
            enable_live_tv_access: value.enable_live_tv_access,
            enable_media_playback: value.enable_media_playback,
            enable_audio_playback_transcoding: value.enable_audio_playback_transcoding,
            enable_video_playback_transcoding: value.enable_video_playback_transcoding,
            enable_transcoding_quality: value.enable_transcoding_quality,
            auto_remote_quality: value.auto_remote_quality,
            enable_playback_remuxing: value.enable_playback_remuxing,
            enable_content_deletion: value.enable_content_deletion,
            restricted_features: value.restricted_features,
            enable_content_deletion_from_folders: value.enable_content_deletion_from_folders,
            enable_content_downloading: value.enable_content_downloading,
            enable_subtitle_downloading: value.enable_subtitle_downloading,
            enable_subtitle_management: value.enable_subtitle_management,
            enable_sync_transcoding: value.enable_sync_transcoding,
            enable_media_conversion: value.enable_media_conversion,
            enabled_channels: value.enabled_channels,
            enable_all_channels: value.enable_all_channels,
            enabled_folders: value.enabled_folders,
            enable_all_folders: value.enable_all_folders,
            invalid_login_attempt_count: value.invalid_login_attempt_count,
            enable_public_sharing: value.enable_public_sharing,
            remote_client_bitrate_limit: value.remote_client_bitrate_limit,
            authentication_provider_id: value.authentication_provider_id,
            excluded_sub_folders: value.excluded_sub_folders,
            simultaneous_stream_limit: value.simultaneous_stream_limit,
            enabled_devices: value.enabled_devices,
            enable_all_devices: value.enable_all_devices,
            allow_camera_upload: value.allow_camera_upload,
            allow_sharing_personal_items: value.allow_sharing_personal_items,
        }
    }
}

impl<'de> Deserialize<'de> for EmbyUserPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_case_insensitive_object::<_, EmbyUserPolicyFields>(deserializer, POLICY_FIELDS)
            .map(Into::into)
    }
}

impl EmbyUserPolicy {
    pub(super) fn apply_to_shared(&self, shared: &mut UserPolicy) {
        macro_rules! assign {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    shared.$field = value;
                }
            };
        }
        macro_rules! assign_clone {
            ($source:ident, $target:ident) => {
                if let Some(value) = &self.$source {
                    shared.$target = value.clone();
                }
            };
        }

        assign!(is_administrator);
        assign!(is_hidden);
        assign!(is_disabled);
        match self.max_parental_rating {
            NullableField::Missing => {}
            NullableField::Null => shared.max_parental_rating = None,
            NullableField::Value(value) => shared.max_parental_rating = Some(value),
        }
        assign_clone!(blocked_tags, blocked_tags);
        assign_clone!(include_tags, allowed_tags);
        assign!(enable_user_preference_access);
        if let Some(schedules) = &self.access_schedules {
            shared.access_schedules = schedules.iter().map(shared_access_schedule).collect();
        }
        if let Some(items) = &self.block_unrated_items {
            shared.block_unrated_items = items
                .iter()
                .filter_map(|item| shared_unrated_item(*item))
                .collect();
        }
        assign!(enable_remote_control_of_other_users);
        assign!(enable_shared_device_control);
        assign!(enable_remote_access);
        assign!(enable_live_tv_management);
        assign!(enable_live_tv_access);
        assign!(enable_media_playback);
        assign!(enable_audio_playback_transcoding);
        assign!(enable_video_playback_transcoding);
        assign!(enable_playback_remuxing);
        assign!(enable_content_deletion);
        assign_clone!(
            enable_content_deletion_from_folders,
            enable_content_deletion_from_folders
        );
        assign!(enable_content_downloading);
        assign!(enable_subtitle_management);
        assign!(enable_sync_transcoding);
        assign!(enable_media_conversion);
        if let Some(values) = &self.enabled_channels {
            shared.enabled_channels = parse_uuids(values);
        }
        assign!(enable_all_channels);
        if let Some(values) = &self.enabled_folders {
            shared.enabled_folders = parse_uuids(values);
        }
        assign!(enable_all_folders);
        assign!(invalid_login_attempt_count);
        assign!(enable_public_sharing);
        assign!(remote_client_bitrate_limit);
        if let Some(value) = &self.authentication_provider_id {
            shared.authentication_provider_id = Some(value.clone());
        }
        if let Some(value) = self.simultaneous_stream_limit {
            shared.max_active_sessions = value;
        }
        assign_clone!(enabled_devices, enabled_devices);
        assign!(enable_all_devices);
    }

    pub(super) fn from_storage(policy: &Value) -> Self {
        let shared = serde_json::from_value::<UserPolicy>(policy.clone()).unwrap_or_default();
        if let Some(mut stored) = policy
            .get(POLICY_STORAGE_KEY)
            .and_then(|value| serde_json::from_value::<EmbyUserPolicy>(value.clone()).ok())
        {
            stored.overlay_shared(&shared);
            return stored;
        }
        Self::from_shared(&shared)
    }

    fn overlay_shared(&mut self, shared: &UserPolicy) {
        self.is_administrator = Some(shared.is_administrator);
        self.is_hidden = Some(shared.is_hidden);
        self.is_disabled = Some(shared.is_disabled);
        self.max_parental_rating = shared
            .max_parental_rating
            .map_or(NullableField::Null, NullableField::Value);
        self.blocked_tags = Some(shared.blocked_tags.clone());
        self.include_tags = Some(shared.allowed_tags.clone());
        self.enable_user_preference_access = Some(shared.enable_user_preference_access);
        self.access_schedules = Some(
            shared
                .access_schedules
                .iter()
                .map(emby_access_schedule)
                .collect(),
        );
        self.block_unrated_items = Some(overlay_unrated_items(
            self.block_unrated_items.as_deref(),
            &shared.block_unrated_items,
        ));
        self.enable_remote_control_of_other_users =
            Some(shared.enable_remote_control_of_other_users);
        self.enable_shared_device_control = Some(shared.enable_shared_device_control);
        self.enable_remote_access = Some(shared.enable_remote_access);
        self.enable_live_tv_management = Some(shared.enable_live_tv_management);
        self.enable_live_tv_access = Some(shared.enable_live_tv_access);
        self.enable_media_playback = Some(shared.enable_media_playback);
        self.enable_audio_playback_transcoding = Some(shared.enable_audio_playback_transcoding);
        self.enable_video_playback_transcoding = Some(shared.enable_video_playback_transcoding);
        self.enable_playback_remuxing = Some(shared.enable_playback_remuxing);
        self.enable_content_deletion = Some(shared.enable_content_deletion);
        self.enable_content_deletion_from_folders =
            Some(shared.enable_content_deletion_from_folders.clone());
        self.enable_content_downloading = Some(shared.enable_content_downloading);
        self.enable_subtitle_management = Some(shared.enable_subtitle_management);
        self.enable_sync_transcoding = Some(shared.enable_sync_transcoding);
        self.enable_media_conversion = Some(shared.enable_media_conversion);
        self.enabled_channels = Some(overlay_uuid_strings(
            self.enabled_channels.as_deref(),
            &shared.enabled_channels,
        ));
        self.enable_all_channels = Some(shared.enable_all_channels);
        self.enabled_folders = Some(overlay_uuid_strings(
            self.enabled_folders.as_deref(),
            &shared.enabled_folders,
        ));
        self.enable_all_folders = Some(shared.enable_all_folders);
        self.invalid_login_attempt_count = Some(shared.invalid_login_attempt_count);
        self.enable_public_sharing = Some(shared.enable_public_sharing);
        self.remote_client_bitrate_limit = Some(shared.remote_client_bitrate_limit);
        self.authentication_provider_id = shared.authentication_provider_id.clone();
        self.simultaneous_stream_limit = Some(shared.max_active_sessions);
        self.enabled_devices = Some(shared.enabled_devices.clone());
        self.enable_all_devices = Some(shared.enable_all_devices);
    }

    fn from_shared(shared: &UserPolicy) -> Self {
        Self {
            is_administrator: Some(shared.is_administrator),
            is_hidden: Some(shared.is_hidden),
            is_hidden_remotely: Some(false),
            is_hidden_from_unused_devices: Some(false),
            is_disabled: Some(shared.is_disabled),
            locked_out_date: None,
            max_parental_rating: shared
                .max_parental_rating
                .map_or(NullableField::Null, NullableField::Value),
            allow_tag_or_rating: Some(false),
            blocked_tags: Some(shared.blocked_tags.clone()),
            is_tag_blocking_mode_inclusive: Some(false),
            include_tags: Some(shared.allowed_tags.clone()),
            enable_user_preference_access: Some(shared.enable_user_preference_access),
            access_schedules: Some(
                shared
                    .access_schedules
                    .iter()
                    .map(emby_access_schedule)
                    .collect(),
            ),
            block_unrated_items: Some(
                shared
                    .block_unrated_items
                    .iter()
                    .map(|item| emby_unrated_item(*item))
                    .collect(),
            ),
            enable_remote_control_of_other_users: Some(shared.enable_remote_control_of_other_users),
            enable_shared_device_control: Some(shared.enable_shared_device_control),
            enable_remote_access: Some(shared.enable_remote_access),
            enable_live_tv_management: Some(shared.enable_live_tv_management),
            enable_live_tv_access: Some(shared.enable_live_tv_access),
            enable_media_playback: Some(shared.enable_media_playback),
            enable_audio_playback_transcoding: Some(shared.enable_audio_playback_transcoding),
            enable_video_playback_transcoding: Some(shared.enable_video_playback_transcoding),
            enable_transcoding_quality: Some(false),
            auto_remote_quality: Some(0),
            enable_playback_remuxing: Some(shared.enable_playback_remuxing),
            enable_content_deletion: Some(shared.enable_content_deletion),
            restricted_features: Some(Vec::new()),
            enable_content_deletion_from_folders: Some(
                shared.enable_content_deletion_from_folders.clone(),
            ),
            enable_content_downloading: Some(shared.enable_content_downloading),
            enable_subtitle_downloading: Some(false),
            enable_subtitle_management: Some(shared.enable_subtitle_management),
            enable_sync_transcoding: Some(shared.enable_sync_transcoding),
            enable_media_conversion: Some(shared.enable_media_conversion),
            enabled_channels: Some(format_uuids(&shared.enabled_channels)),
            enable_all_channels: Some(shared.enable_all_channels),
            enabled_folders: Some(format_uuids(&shared.enabled_folders)),
            enable_all_folders: Some(shared.enable_all_folders),
            invalid_login_attempt_count: Some(shared.invalid_login_attempt_count),
            enable_public_sharing: Some(shared.enable_public_sharing),
            remote_client_bitrate_limit: Some(shared.remote_client_bitrate_limit),
            authentication_provider_id: shared.authentication_provider_id.clone(),
            excluded_sub_folders: Some(Vec::new()),
            simultaneous_stream_limit: Some(shared.max_active_sessions),
            enabled_devices: Some(shared.enabled_devices.clone()),
            enable_all_devices: Some(shared.enable_all_devices),
            allow_camera_upload: Some(false),
            allow_sharing_personal_items: Some(false),
        }
    }
}

fn parse_uuids(values: &[String]) -> Vec<Uuid> {
    values
        .iter()
        .filter_map(|value| value.parse().ok())
        .collect()
}

fn format_uuids(values: &[Uuid]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.simple().to_string())
        .collect()
}

fn overlay_uuid_strings(stored: Option<&[String]>, shared: &[Uuid]) -> Vec<String> {
    let mut shared = format_uuids(shared).into_iter();
    let mut values = Vec::new();
    for value in stored.unwrap_or_default() {
        if value.parse::<Uuid>().is_err() {
            // Emby ids are strings and may not have a Jellyfin Guid mapping.
            // Keep those protocol-only ids while refreshing every Guid-backed
            // entry from the authoritative shared policy/configuration.
            values.push(value.clone());
        } else if let Some(value) = shared.next() {
            values.push(value);
        }
    }
    values.extend(shared);
    values
}

fn overlay_unrated_items(
    stored: Option<&[EmbyUnratedItem]>,
    shared: &[UnratedItem],
) -> Vec<EmbyUnratedItem> {
    let mut shared = shared.iter().copied().map(emby_unrated_item);
    let mut values = Vec::new();
    for value in stored.unwrap_or_default() {
        if *value == EmbyUnratedItem::Game {
            values.push(*value);
        } else if let Some(value) = shared.next() {
            values.push(value);
        }
    }
    values.extend(shared);
    values
}

const fn shared_subtitle_mode(value: EmbySubtitlePlaybackMode) -> Option<SubtitlePlaybackMode> {
    match value {
        EmbySubtitlePlaybackMode::Default => Some(SubtitlePlaybackMode::Default),
        EmbySubtitlePlaybackMode::Always => Some(SubtitlePlaybackMode::Always),
        EmbySubtitlePlaybackMode::OnlyForced => Some(SubtitlePlaybackMode::OnlyForced),
        EmbySubtitlePlaybackMode::None => Some(SubtitlePlaybackMode::None),
        EmbySubtitlePlaybackMode::Smart => Some(SubtitlePlaybackMode::Smart),
        EmbySubtitlePlaybackMode::HearingImpaired => None,
    }
}

const fn emby_subtitle_mode(value: SubtitlePlaybackMode) -> EmbySubtitlePlaybackMode {
    match value {
        SubtitlePlaybackMode::Default => EmbySubtitlePlaybackMode::Default,
        SubtitlePlaybackMode::Always => EmbySubtitlePlaybackMode::Always,
        SubtitlePlaybackMode::OnlyForced => EmbySubtitlePlaybackMode::OnlyForced,
        SubtitlePlaybackMode::None => EmbySubtitlePlaybackMode::None,
        SubtitlePlaybackMode::Smart => EmbySubtitlePlaybackMode::Smart,
    }
}

const fn shared_day(value: EmbyDynamicDayOfWeek) -> DynamicDayOfWeek {
    match value {
        EmbyDynamicDayOfWeek::Sunday => DynamicDayOfWeek::Sunday,
        EmbyDynamicDayOfWeek::Monday => DynamicDayOfWeek::Monday,
        EmbyDynamicDayOfWeek::Tuesday => DynamicDayOfWeek::Tuesday,
        EmbyDynamicDayOfWeek::Wednesday => DynamicDayOfWeek::Wednesday,
        EmbyDynamicDayOfWeek::Thursday => DynamicDayOfWeek::Thursday,
        EmbyDynamicDayOfWeek::Friday => DynamicDayOfWeek::Friday,
        EmbyDynamicDayOfWeek::Saturday => DynamicDayOfWeek::Saturday,
        EmbyDynamicDayOfWeek::Everyday => DynamicDayOfWeek::Everyday,
        EmbyDynamicDayOfWeek::Weekday => DynamicDayOfWeek::Weekday,
        EmbyDynamicDayOfWeek::Weekend => DynamicDayOfWeek::Weekend,
    }
}

const fn emby_day(value: DynamicDayOfWeek) -> EmbyDynamicDayOfWeek {
    match value {
        DynamicDayOfWeek::Sunday => EmbyDynamicDayOfWeek::Sunday,
        DynamicDayOfWeek::Monday => EmbyDynamicDayOfWeek::Monday,
        DynamicDayOfWeek::Tuesday => EmbyDynamicDayOfWeek::Tuesday,
        DynamicDayOfWeek::Wednesday => EmbyDynamicDayOfWeek::Wednesday,
        DynamicDayOfWeek::Thursday => EmbyDynamicDayOfWeek::Thursday,
        DynamicDayOfWeek::Friday => EmbyDynamicDayOfWeek::Friday,
        DynamicDayOfWeek::Saturday => EmbyDynamicDayOfWeek::Saturday,
        DynamicDayOfWeek::Everyday => EmbyDynamicDayOfWeek::Everyday,
        DynamicDayOfWeek::Weekday => EmbyDynamicDayOfWeek::Weekday,
        DynamicDayOfWeek::Weekend => EmbyDynamicDayOfWeek::Weekend,
    }
}

fn shared_access_schedule(value: &EmbyAccessSchedule) -> AccessSchedule {
    AccessSchedule {
        id: 0,
        user_id: Uuid::nil(),
        day_of_week: shared_day(value.day_of_week),
        start_hour: value.start_hour,
        end_hour: value.end_hour,
    }
}

fn emby_access_schedule(value: &AccessSchedule) -> EmbyAccessSchedule {
    EmbyAccessSchedule {
        day_of_week: emby_day(value.day_of_week),
        start_hour: value.start_hour,
        end_hour: value.end_hour,
    }
}

const fn shared_unrated_item(value: EmbyUnratedItem) -> Option<UnratedItem> {
    match value {
        EmbyUnratedItem::Movie => Some(UnratedItem::Movie),
        EmbyUnratedItem::Trailer => Some(UnratedItem::Trailer),
        EmbyUnratedItem::Series => Some(UnratedItem::Series),
        EmbyUnratedItem::Music => Some(UnratedItem::Music),
        EmbyUnratedItem::Game => None,
        EmbyUnratedItem::Book => Some(UnratedItem::Book),
        EmbyUnratedItem::LiveTvChannel => Some(UnratedItem::LiveTvChannel),
        EmbyUnratedItem::LiveTvProgram => Some(UnratedItem::LiveTvProgram),
        EmbyUnratedItem::ChannelContent => Some(UnratedItem::ChannelContent),
        EmbyUnratedItem::Other => Some(UnratedItem::Other),
    }
}

const fn emby_unrated_item(value: UnratedItem) -> EmbyUnratedItem {
    match value {
        UnratedItem::Movie => EmbyUnratedItem::Movie,
        UnratedItem::Trailer => EmbyUnratedItem::Trailer,
        UnratedItem::Series => EmbyUnratedItem::Series,
        UnratedItem::Music => EmbyUnratedItem::Music,
        UnratedItem::Book => EmbyUnratedItem::Book,
        UnratedItem::LiveTvChannel => EmbyUnratedItem::LiveTvChannel,
        UnratedItem::LiveTvProgram => EmbyUnratedItem::LiveTvProgram,
        UnratedItem::ChannelContent => EmbyUnratedItem::ChannelContent,
        UnratedItem::Other => EmbyUnratedItem::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_binding_is_case_insensitive_last_wins_and_typed() {
        let configuration: EmbyUserConfiguration = serde_json::from_str(
            r#"{
                "subtitlemode":"hearingimpaired",
                "SubtitleMode":5,
                "INTroskipMODE":1,
                "ResumeRewindSeconds":17,
                "unknown":true
            }"#,
        )
        .unwrap();
        assert_eq!(
            configuration.subtitle_mode,
            Some(EmbySubtitlePlaybackMode::HearingImpaired)
        );
        assert_eq!(
            configuration.intro_skip_mode,
            Some(EmbySegmentSkipMode::AutoSkip)
        );
        assert_eq!(configuration.resume_rewind_seconds, Some(17));
        let serialized = serde_json::to_value(configuration).unwrap();
        assert_eq!(serialized["SubtitleMode"], "HearingImpaired");
        assert_eq!(serialized["IntroSkipMode"], "AutoSkip");
        assert!(serialized.get("unknown").is_none());
    }

    #[test]
    fn policy_binding_preserves_emby_only_enum_values() {
        let policy: EmbyUserPolicy = serde_json::from_str(
            r#"{
                "blockunrateditems":[4,"OTHER"],
                "IsHiddenRemotely":true,
                "AllowCameraUpload":true,
                "MaxParentalRating":null
            }"#,
        )
        .unwrap();
        assert_eq!(
            policy.block_unrated_items,
            Some(vec![EmbyUnratedItem::Game, EmbyUnratedItem::Other])
        );
        let mut shared = UserPolicy::default();
        shared.max_parental_rating = Some(12);
        policy.apply_to_shared(&mut shared);
        assert_eq!(shared.block_unrated_items, vec![UnratedItem::Other]);
        assert_eq!(shared.max_parental_rating, None);
        let serialized = serde_json::to_value(policy).unwrap();
        assert_eq!(
            serialized["BlockUnratedItems"],
            serde_json::json!(["Game", "Other"])
        );
        assert_eq!(serialized["IsHiddenRemotely"], true);
        assert!(serialized["MaxParentalRating"].is_null());
    }

    #[test]
    fn undefined_enum_values_are_rejected() {
        assert!(serde_json::from_str::<EmbyUserConfiguration>(r#"{"SubtitleMode":6}"#).is_err());
        assert!(serde_json::from_str::<EmbyUserPolicy>(r#"{"BlockUnratedItems":[10]}"#).is_err());
    }

    #[test]
    fn numeric_strings_follow_official_json_defaults() {
        let configuration: EmbyUserConfiguration =
            serde_json::from_str(r#"{"ResumeRewindSeconds":"17"}"#).unwrap();
        assert_eq!(configuration.resume_rewind_seconds, Some(17));

        let policy: EmbyUserPolicy = serde_json::from_str(
            r#"{
                "LockedOutDate":"638900000000000000",
                "MaxParentalRating":"12",
                "AutoRemoteQuality":"2",
                "InvalidLoginAttemptCount":"3",
                "RemoteClientBitrateLimit":"4000000",
                "SimultaneousStreamLimit":"4",
                "AccessSchedules":[{
                    "DayOfWeek":"Weekday",
                    "StartHour":"8.5",
                    "EndHour":"17.25"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(policy.locked_out_date, Some(638_900_000_000_000_000));
        assert_eq!(policy.max_parental_rating, NullableField::Value(12));
        assert_eq!(policy.auto_remote_quality, Some(2));
        assert_eq!(policy.invalid_login_attempt_count, Some(3));
        assert_eq!(policy.remote_client_bitrate_limit, Some(4_000_000));
        assert_eq!(policy.simultaneous_stream_limit, Some(4));
        let schedule = &policy.access_schedules.unwrap()[0];
        assert_eq!(schedule.start_hour, 8.5);
        assert_eq!(schedule.end_hour, 17.25);
    }

    #[test]
    fn stored_emby_contracts_overlay_shared_fields_but_keep_protocol_only_values() {
        let old_view = Uuid::new_v4();
        let new_view = Uuid::new_v4();
        let mut configuration = UserConfiguration::default();
        configuration.play_default_audio_track = false;
        configuration.ordered_views = vec![new_view];
        let mut preferences = serde_json::to_value(&configuration).unwrap();
        preferences.as_object_mut().unwrap().insert(
            CONFIGURATION_STORAGE_KEY.to_owned(),
            serde_json::json!({
                "PlayDefaultAudioTrack": true,
                "SubtitleMode": "HearingImpaired",
                "ProfilePin": "7391",
                "OrderedViews": [old_view.simple().to_string(), "emby-only-view"]
            }),
        );
        let projected = EmbyUserConfiguration::from_storage(&preferences, true);
        assert_eq!(projected.play_default_audio_track, Some(false));
        assert_eq!(
            projected.subtitle_mode,
            Some(EmbySubtitlePlaybackMode::HearingImpaired)
        );
        assert_eq!(projected.profile_pin.as_deref(), Some("7391"));
        assert_eq!(
            projected.ordered_views,
            Some(vec![
                new_view.simple().to_string(),
                "emby-only-view".to_owned()
            ])
        );
        assert_eq!(projected.enable_local_password, Some(true));

        let new_folder = Uuid::new_v4();
        let mut policy = UserPolicy::default();
        policy.enable_media_playback = false;
        policy.enabled_folders = vec![new_folder];
        policy.block_unrated_items = vec![UnratedItem::Book];
        let mut stored_policy = serde_json::to_value(&policy).unwrap();
        stored_policy.as_object_mut().unwrap().insert(
            POLICY_STORAGE_KEY.to_owned(),
            serde_json::json!({
                "EnableMediaPlayback": true,
                "IsHiddenRemotely": true,
                "EnabledFolders": [old_view.simple().to_string(), "emby-only-folder"],
                "BlockUnratedItems": ["Game", "Movie"]
            }),
        );
        let projected = EmbyUserPolicy::from_storage(&stored_policy);
        assert_eq!(projected.enable_media_playback, Some(false));
        assert_eq!(projected.is_hidden_remotely, Some(true));
        assert_eq!(
            projected.enabled_folders,
            Some(vec![
                new_folder.simple().to_string(),
                "emby-only-folder".to_owned()
            ])
        );
        assert_eq!(
            projected.block_unrated_items,
            Some(vec![EmbyUnratedItem::Game, EmbyUnratedItem::Book])
        );
    }
}
