//! Parsers for Jellyfin media naming conventions.

pub mod audio;
pub mod audiobook;
pub mod book;
pub mod common;
pub mod external_files;
pub mod extra;
mod provider_ids;
pub mod stack;
pub mod tv;
pub mod tv_paths;
pub mod video;
pub mod video_list;

pub use audio::{AlbumParser, is_audio_file};
pub use audiobook::{
    AudioBookFileInfo, AudioBookFilePathParser, AudioBookFilePathParserResult, AudioBookInfo,
    AudioBookListResolver, AudioBookNameParser, AudioBookNameParserResult, AudioBookResolver,
};
pub use book::{BookFileNameParser, BookFileNameParserResult};
pub use common::NamingOptions;
pub use external_files::{
    DlnaProfileType, ExternalPathParser, ExternalPathParserResult, LanguageInfo,
    LocalizationManager,
};
pub use extra::{ExtraResolver, ExtraResult, ExtraRuleResolver};
pub use provider_ids::ProviderIdMap;
pub use stack::{FileStack, FileStackRule, FileStackRuleResult, StackFileInfo, StackResolver};
pub use tv::{
    DateOrder, EpisodeExpression, EpisodeInfo, EpisodePathParser, EpisodePathParserResult,
    EpisodeResolver, SeriesStatus, TvParserHelpers,
};
pub use tv_paths::{
    SeasonPathParser, SeasonPathParserResult, SeriesInfo, SeriesPathParser, SeriesPathParserResult,
    SeriesResolver,
};
pub use video::{
    CleanDateTimeResult, ExtraRule, ExtraRuleType, ExtraType, Format3dParser, Format3dResult,
    Format3dRule, MediaType, StubResolver, StubTypeRule, VideoFileInfo, VideoResolver,
};
pub use video_list::{CollectionType, VideoInfo, VideoListResolver};
