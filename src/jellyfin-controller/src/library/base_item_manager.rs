#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SourceType {
    #[default]
    Library = 0,
    Channel = 1,
    LiveTv = 2,
}

/// Item fields used when deciding whether metadata providers are enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseItemInfo {
    pub item_type: String,
    pub source_type: SourceType,
    pub enable_media_source_display: bool,
    pub is_channel: bool,
}

impl BaseItemInfo {
    #[must_use]
    pub fn new(item_type: impl Into<String>) -> Self {
        Self {
            item_type: item_type.into(),
            source_type: SourceType::default(),
            enable_media_source_display: false,
            is_channel: false,
        }
    }
}

/// Per-library provider selections for one item type.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TypeOptions {
    pub item_type: String,
    pub metadata_fetchers: Vec<String>,
    pub metadata_fetcher_order: Vec<String>,
    pub image_fetchers: Vec<String>,
    pub image_fetcher_order: Vec<String>,
    pub similar_item_providers: Vec<String>,
    pub similar_item_provider_order: Vec<String>,
}

/// Server-wide provider settings for one item type.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MetadataOptions {
    pub item_type: String,
    pub disabled_metadata_savers: Vec<String>,
    pub local_metadata_reader_order: Vec<String>,
    pub disabled_metadata_fetchers: Vec<String>,
    pub metadata_fetcher_order: Vec<String>,
    pub disabled_image_fetchers: Vec<String>,
    pub image_fetcher_order: Vec<String>,
}

impl MetadataOptions {
    fn for_item_type(item_type: &str) -> Self {
        Self {
            item_type: item_type.to_owned(),
            ..Self::default()
        }
    }
}

/// Server configuration fields consumed by [`BaseItemManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfiguration {
    pub metadata_options: Vec<MetadataOptions>,
}

impl Default for ServerConfiguration {
    fn default() -> Self {
        let mut music_video = MetadataOptions::for_item_type("MusicVideo");
        music_video.disabled_metadata_fetchers = vec!["The Open Movie Database".to_owned()];
        music_video.disabled_image_fetchers = vec!["The Open Movie Database".to_owned()];

        let mut music_album = MetadataOptions::for_item_type("MusicAlbum");
        music_album.disabled_metadata_fetchers = vec!["TheAudioDB".to_owned()];

        let mut music_artist = MetadataOptions::for_item_type("MusicArtist");
        music_artist.disabled_metadata_fetchers = vec!["TheAudioDB".to_owned()];

        Self {
            metadata_options: vec![
                MetadataOptions::for_item_type("Book"),
                MetadataOptions::for_item_type("Movie"),
                music_video,
                MetadataOptions::for_item_type("Series"),
                music_album,
                music_artist,
                MetadataOptions::for_item_type("BoxSet"),
                MetadataOptions::for_item_type("Season"),
                MetadataOptions::for_item_type("Episode"),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseItemManager {
    server_configuration: ServerConfiguration,
}

impl BaseItemManager {
    #[must_use]
    pub const fn new(server_configuration: ServerConfiguration) -> Self {
        Self {
            server_configuration,
        }
    }

    #[must_use]
    pub fn server_configuration(&self) -> &ServerConfiguration {
        &self.server_configuration
    }

    #[must_use]
    pub fn is_metadata_fetcher_enabled(
        &self,
        item: &BaseItemInfo,
        library_type_options: Option<&TypeOptions>,
        name: &str,
    ) -> bool {
        self.is_fetcher_enabled(item, library_type_options, name, FetcherKind::Metadata)
    }

    #[must_use]
    pub fn is_image_fetcher_enabled(
        &self,
        item: &BaseItemInfo,
        library_type_options: Option<&TypeOptions>,
        name: &str,
    ) -> bool {
        self.is_fetcher_enabled(item, library_type_options, name, FetcherKind::Image)
    }

    fn is_fetcher_enabled(
        &self,
        item: &BaseItemInfo,
        library_type_options: Option<&TypeOptions>,
        name: &str,
        kind: FetcherKind,
    ) -> bool {
        if item.is_channel {
            return true;
        }
        if item.source_type == SourceType::Channel {
            return !item.enable_media_source_display;
        }

        if let Some(options) = library_type_options {
            let enabled_fetchers = match kind {
                FetcherKind::Metadata => &options.metadata_fetchers,
                FetcherKind::Image => &options.image_fetchers,
            };
            return contains_ignore_case(enabled_fetchers, name);
        }

        self.server_configuration
            .metadata_options
            .iter()
            .find(|options| options.item_type.eq_ignore_ascii_case(&item.item_type))
            .is_none_or(|options| {
                let disabled_fetchers = match kind {
                    FetcherKind::Metadata => &options.disabled_metadata_fetchers,
                    FetcherKind::Image => &options.disabled_image_fetchers,
                };
                !contains_ignore_case(disabled_fetchers, name)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetcherKind {
    Metadata,
    Image,
}

fn contains_ignore_case(values: &[String], expected: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(expected))
}
