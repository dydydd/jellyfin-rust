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
pub mod net;
mod serde_datetime;
mod serde_guid;
pub mod system;
pub mod users;

pub use configuration::UserConfiguration;
pub use cryptography::{PasswordHash, PasswordHashError, PasswordHashSegment};
pub use dlna::{
    ContainerHelper, ContainerProfile, DlnaProfileType, EncodingContext, MediaSourceInfo,
    MediaStreamProtocol, PlayMethod, ProfileCondition, ProfileConditionType, ProfileConditionValue,
    StreamInfo, TranscodeReason, TranscodeSeekInfo, VideoType,
};
pub use drawing::{ImageFormat, InvalidImageFormat};
pub use dto::UserDto;
pub use entities::{
    AudioSpatialFormat, HasProviderIds, MediaStream, MediaStreamType, MetadataProvider,
    ProviderIdError, ProviderIdMap, ProviderIdsExtensions, SubtitleDeliveryMethod, VideoRange,
    VideoRangeType,
};
pub use enums::{DynamicDayOfWeek, SubtitlePlaybackMode, SyncPlayUserAccessType, UnratedItem};
pub use net::{MimeTypeError, MimeTypes};
pub use system::PublicSystemInfo;
pub use users::{AccessSchedule, UserPolicy};
