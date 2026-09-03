use serde::{Deserialize, Serialize};

/// Jellyfin's public item-kind enum, generated from `BaseItem` subclasses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum BaseItemKind {
    #[default]
    AggregateFolder,
    Audio,
    AudioBook,
    BasePluginFolder,
    Book,
    BoxSet,
    Channel,
    ChannelFolderItem,
    CollectionFolder,
    Episode,
    Folder,
    Genre,
    ManualPlaylistsFolder,
    Movie,
    LiveTvChannel,
    LiveTvProgram,
    MusicAlbum,
    MusicArtist,
    MusicGenre,
    MusicVideo,
    Person,
    Photo,
    PhotoAlbum,
    Playlist,
    PlaylistsFolder,
    Program,
    Recording,
    Season,
    Series,
    Studio,
    Trailer,
    TvChannel,
    TvProgram,
    UserRootFolder,
    UserView,
    Video,
    Year,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PlayAccess {
    #[default]
    Full,
    None,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SubtitlePlaybackMode {
    #[default]
    Default,
    Always,
    OnlyForced,
    None,
    Smart,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum DynamicDayOfWeek {
    #[default]
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Everyday,
    Weekday,
    Weekend,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SyncPlayUserAccessType {
    #[default]
    CreateAndJoinGroups,
    JoinGroups,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum UnratedItem {
    Movie,
    Trailer,
    Series,
    Music,
    Book,
    LiveTvChannel,
    LiveTvProgram,
    ChannelContent,
    Other,
}

impl UnratedItem {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "Movie",
            Self::Trailer => "Trailer",
            Self::Series => "Series",
            Self::Music => "Music",
            Self::Book => "Book",
            Self::LiveTvChannel => "LiveTvChannel",
            Self::LiveTvProgram => "LiveTvProgram",
            Self::ChannelContent => "ChannelContent",
            Self::Other => "Other",
        }
    }
}
