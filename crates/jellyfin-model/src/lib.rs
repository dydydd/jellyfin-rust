//! API-facing models shared by the Jellyfin server crates.
//!
//! Jellyfin's public JSON contract uses PascalCase property names, string
//! enums and UUIDs in the compact `N` format. The DTOs in this crate encode
//! those rules at the type boundary.

pub mod configuration;
pub mod cryptography;
pub mod dlna;
pub mod drawing;
pub mod dto;
pub mod entities;
pub mod enums;
pub mod extensions;
pub mod live_tv;
pub mod net;
pub mod plugins;
pub mod providers;
mod serde_datetime;
mod serde_guid;
pub mod system;
mod user_data;
pub mod users;

pub use configuration::UserConfiguration;
pub use cryptography::{PasswordHash, PasswordHashError, PasswordHashSegment};
pub use dlna::{
    CodecProfile, CodecType, ContainerHelper, ContainerProfile, DeviceProfile, DirectPlayProfile,
    DlnaProfileType, EncodingContext, MediaOptions, MediaProtocol, MediaSourceInfo,
    MediaStreamProtocol, PlayMethod, ProfileCondition, ProfileConditionType, ProfileConditionValue,
    StreamBuilder, StreamBuilderError, StreamInfo, SubtitleProfile, TranscodeReason,
    TranscodeSeekInfo, TranscodingProfile, VideoType,
};
pub use drawing::{ImageFormat, InvalidImageFormat};
pub use dto::UserDto;
pub use entities::{
    AudioSpatialFormat, HasProviderIds, MediaStream, MediaStreamType, MetadataProvider,
    ProviderIdError, ProviderIdMap, ProviderIdsExtensions, SubtitleDeliveryMethod, VideoRange,
    VideoRangeType,
};
pub use enums::{DynamicDayOfWeek, SubtitlePlaybackMode, SyncPlayUserAccessType, UnratedItem};
pub use extensions::first_to_upper;
pub use live_tv::{TimerInfo, TunerHostInfo};
pub use net::{MimeTypeError, MimeTypes};
pub use plugins::{PluginInfo, PluginStatus};
pub use providers::{ImageType, RatingType, RemoteImageInfo, order_by_language_descending};
pub use system::PublicSystemInfo;
pub use user_data::UserItemDataDto;
pub use users::{AccessSchedule, UserPolicy};
