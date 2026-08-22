mod artists;
mod audio_db;
pub mod client_event;
mod collections;
mod dashboard;
mod environment;
mod episode_parser;
mod genres;
mod item_images;
mod item_lookup;
mod item_types;
mod item_update;
pub mod library;
mod library_controller;
mod library_scan;
pub mod library_watcher;
mod localization;
mod lyrics;
mod media_attachments;
pub mod media_encoding;
pub mod transcode;

pub use transcode::{
    FfmpegCommand, HlsSegmentSettings, TranscodeJobRegistry, TranscodeTarget, audio_command,
    build_main_playlist, build_master_playlist, hls_command, hls_job_id, run_ffmpeg,
    wait_for_segment,
};
mod media_streams;
mod metadata_editor;
pub mod metadata_providers;
mod music_genre;
mod omdb;
mod packages;
mod persons;
mod playlists;
mod playstate;
mod plugins;
pub mod providers;
mod scheduled_tasks;
mod studios;
mod system_logs;
mod system_storage;
mod subtitles;
mod tmdb;
mod trickplay;
mod user_data;
mod user_library;
mod users;
mod videos;
mod virtual_folders;
mod years;

pub use artists::{Artist, ArtistError, ArtistPage, ArtistService, ArtistValueKind};
pub use collections::{CollectionError, CollectionService};
pub use dashboard::{DashboardError, DashboardPage, DashboardService};
pub use environment::{EnvironmentError, EnvironmentService};
pub use genres::{Genre, GenreError, GenreKind, GenrePage, GenreService};
pub use item_images::{ItemImageError, ItemImageResource, ItemImageService};
pub use item_lookup::{ItemLookupError, ItemLookupService, RemoteSearchInfo, RemoteSearchRequest};
pub use item_types::{
    HydratedBaseItem, ItemTypeRegistrationError, ItemTypeRegistry, KnownItemType,
};
pub use item_update::{ItemUpdateError, ItemUpdateInput, ItemUpdateService};
pub use library_controller::{LibraryControllerError, LibraryControllerService};
pub use library_scan::{LibraryScanError, LibraryScanService, LibraryScanSummary};
pub use localization::LocalizationService;
pub use lyrics::{LyricManager, LyricProvider, LyricSearchRequest, RemoteLyricInfo};
pub use media_attachments::{
    MediaAttachmentFilter, MediaAttachmentMapper, MediaAttachmentService,
    MediaAttachmentServiceError,
};
pub use media_streams::{
    IdentityMediaStreamPathMapper, MediaStreamFilter, MediaStreamMapper, MediaStreamPathMapper,
    MediaStreamService, MediaStreamServiceError,
};
pub use metadata_editor::{MetadataEditorError, MetadataEditorService};
pub use music_genre::{MusicGenre, MusicGenreError, MusicGenrePage, MusicGenreService};
pub use packages::{PackageError, PackageService};
pub use persons::{Person, PersonError, PersonPage, PersonService};
pub use playlists::{PlaylistError, PlaylistService};
pub use playstate::{
    PlaybackProgressUpdate, PlaybackStartUpdate, PlaybackStopUpdate, PlaystateError,
    PlaystateService, PlaystateUpdate, format_date_played, parse_date_played,
};
pub use plugins::{InstalledPlugin, PluginImage, PluginRegistry};
pub use scheduled_tasks::{ScheduledTaskError, ScheduledTaskPaths, ScheduledTaskService};
pub use studios::{Studio, StudioError, StudioPage, StudioService};
pub use subtitles::{SubtitleManager, SubtitleProvider, SubtitleResponse, SubtitleSearchRequest};
pub use system_logs::{OpenedSystemLog, SystemLogError, SystemLogFile, SystemLogService};
pub use system_storage::SystemStorageService;
pub use trickplay::{TrickplayError, TrickplayManifest, TrickplayManifests, TrickplayService};
pub use user_data::{UserDataService, UserDataServiceError, UserDataUpdate};
pub use user_library::{RelatedItemKind, UserLibraryError, UserLibraryService};
pub use users::{UserError, UserService, validate_username};
pub use videos::{VideoError, VideoService};
pub use virtual_folders::{VirtualFolder, VirtualFolderService, VirtualFolderServiceError};
pub use years::{Year, YearError, YearItem, YearPage, YearService};
