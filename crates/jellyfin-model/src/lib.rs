//! API-facing models shared by the Jellyfin server crates.
//!
//! Jellyfin's public JSON contract uses PascalCase property names, string
//! enums and UUIDs in the compact `N` format. The DTOs in this crate encode
//! those rules at the type boundary.

mod authentication;
pub mod configuration;
pub mod cryptography;
mod devices;
pub mod dlna;
pub mod drawing;
pub mod dto;
pub mod entities;
pub mod enums;
pub mod extensions;
mod globalization;
pub mod io;
pub mod live_tv;
mod metadata_editor;
pub mod net;
pub mod plugins;
pub mod providers;
mod serde_datetime;
mod serde_guid;
mod session;
mod sync_play;
pub mod system;
mod user_data;
pub mod users;

pub use authentication::{AuthenticationInfo, QueryResult};
pub use configuration::UserConfiguration;
pub use cryptography::{PasswordHash, PasswordHashError, PasswordHashSegment};
pub use devices::{DeviceInfoDto, DeviceOptionsDto};
pub use dlna::{
    CodecProfile, CodecType, ContainerHelper, ContainerProfile, DeviceProfile, DirectPlayProfile,
    DlnaProfileType, EncodingContext, IsoType, MediaOptions, MediaProtocol, MediaSourceInfo,
    MediaSourceType, MediaStreamProtocol, PlayMethod, ProfileCondition, ProfileConditionType,
    ProfileConditionValue, StreamBuilder, StreamBuilderError, StreamInfo, SubtitleProfile,
    TranscodeReason, TranscodeSeekInfo, TranscodingProfile, TransportStreamTimestamp,
    Video3DFormat, VideoType,
};
pub use drawing::{ImageFormat, InvalidImageFormat};
pub use dto::{NameIdPair, UserDto};
pub use entities::{
    AudioSpatialFormat, HasProviderIds, MediaAttachment, MediaStream, MediaStreamType,
    MetadataProvider, ProviderIdError, ProviderIdMap, ProviderIdsExtensions,
    SubtitleDeliveryMethod, VideoRange, VideoRangeType,
};
pub use enums::{DynamicDayOfWeek, SubtitlePlaybackMode, SyncPlayUserAccessType, UnratedItem};
pub use extensions::first_to_upper;
pub use globalization::LocalizationOption;
pub use io::{FileSystemEntryInfo, FileSystemEntryType};
pub use live_tv::{TimerInfo, TunerHostInfo};
pub use metadata_editor::{
    CollectionType, CountryInfo, CultureDto, ExternalIdInfo, ExternalIdMediaType,
    MetadataEditorInfo, NameValuePair, ParentalRating, ParentalRatingScore,
    ParseCollectionTypeError,
};
pub use net::{EndPointInfo, MimeTypeError, MimeTypes};
pub use plugins::{PluginInfo, PluginStatus};
pub use providers::{ImageType, RatingType, RemoteImageInfo, order_by_language_descending};
pub use session::{
    ClientCapabilitiesDto, GeneralCommand, GeneralCommandType, MediaType, MessageCommand,
    PlayCommand, PlayRequest, PlaystateCommand, PlaystateRequest, SessionInfoDto, SessionUserInfo,
};
pub use sync_play::UtcTimeResponse;
pub use system::PublicSystemInfo;
pub use user_data::{UpdateUserItemDataDto, UserItemDataDto};
pub use users::{AccessSchedule, UserPolicy};
