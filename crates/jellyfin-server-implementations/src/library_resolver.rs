use std::{io, sync::Arc};

use jellyfin_model::CollectionType;
use jellyfin_naming::{AlbumParser, NamingOptions, is_audio_file};

use crate::{
    ResolutionFileSystemEntry,
    audio_resolver::{AudioFileSystemEntry, AudioParentContext, AudioResolveArgs, AudioResolver},
};

/// Mirrors `MediaBrowser.Controller.Resolvers.ResolverPriority`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolverPriority {
    Plugin = 0,
    First = 1,
    Second = 2,
    Third = 3,
    Fourth = 4,
    Fifth = 5,
    Last = 6,
}

/// Entity kinds produced by the library resolver chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedLibraryItemKind {
    MusicArtist,
    MusicAlbum,
    Audio,
    AudioBook,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLibraryItem {
    pub path: String,
    pub kind: ResolvedLibraryItemKind,
}

/// Parent entity kinds that influence resolver decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryParentKind {
    None,
    Folder,
    MusicArtist,
    MusicAlbum,
}

/// Provides file-system children for nested resolver checks.
pub trait DirectoryReader: Send + Sync {
    /// Lists the direct children of `path`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be read.
    fn get_entries(&self, path: &str) -> io::Result<Vec<ResolutionFileSystemEntry>>;
}

#[derive(Debug, Default)]
pub struct FilesystemDirectoryReader;

impl DirectoryReader for FilesystemDirectoryReader {
    fn get_entries(&self, path: &str) -> io::Result<Vec<ResolutionFileSystemEntry>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_type = entry.file_type().ok()?;
                Some(ResolutionFileSystemEntry::new(
                    entry.path().to_string_lossy().into_owned(),
                    file_type.is_dir(),
                ))
            })
            .collect())
    }
}

/// Context passed to every resolver, mirroring `ItemResolveArgs`.
#[derive(Clone)]
pub struct LibraryResolveArgs<'a> {
    pub collection_type: Option<CollectionType>,
    pub path: &'a str,
    pub is_directory: bool,
    pub children: Vec<ResolutionFileSystemEntry>,
    pub parent: LibraryParentKind,
    pub parent_is_root: bool,
    pub parent_path: Option<&'a str>,
    pub directory_reader: &'a dyn DirectoryReader,
}

/// A single Jellyfin entity resolver.
pub trait LibraryItemResolver: Send + Sync {
    fn priority(&self) -> ResolverPriority;
    fn resolve(&self, args: &LibraryResolveArgs<'_>) -> Option<ResolvedLibraryItem>;
}

/// Sorted resolver chain; the first resolver that accepts the path wins.
#[derive(Default)]
pub struct LibraryResolverChain {
    resolvers: Vec<Box<dyn LibraryItemResolver>>,
}

impl LibraryResolverChain {
    #[must_use]
    pub fn new(resolvers: Vec<Box<dyn LibraryItemResolver>>) -> Self {
        let mut resolvers = resolvers;
        resolvers.sort_by_key(|resolver| resolver.priority());
        Self { resolvers }
    }

    #[must_use]
    pub fn default_music_chain() -> Self {
        let options = Arc::new(NamingOptions::default());
        Self::new(vec![
            Box::new(MusicArtistResolver::new(Arc::clone(&options))),
            Box::new(MusicAlbumResolver::new(Arc::clone(&options))),
            Box::new(AudioLibraryResolver::new(options)),
        ])
    }

    #[must_use]
    pub fn resolve(&self, args: &LibraryResolveArgs<'_>) -> Option<ResolvedLibraryItem> {
        self.resolvers
            .iter()
            .find_map(|resolver| resolver.resolve(args))
    }
}

/// Official `MusicArtistResolver`: a music-library directory containing an
/// artist subfolder, an album subfolder, or `artist.nfo`.
#[derive(Debug, Clone)]
pub struct MusicArtistResolver {
    album_resolver: MusicAlbumResolver,
}

impl MusicArtistResolver {
    #[must_use]
    pub fn new(naming_options: impl Into<Arc<NamingOptions>>) -> Self {
        Self {
            album_resolver: MusicAlbumResolver::new(naming_options),
        }
    }
}

impl LibraryItemResolver for MusicArtistResolver {
    fn priority(&self) -> ResolverPriority {
        ResolverPriority::Second
    }

    fn resolve(&self, args: &LibraryResolveArgs<'_>) -> Option<ResolvedLibraryItem> {
        if !args.is_directory || args.collection_type != Some(CollectionType::Music) {
            return None;
        }
        if matches!(
            args.parent,
            LibraryParentKind::MusicArtist | LibraryParentKind::MusicAlbum
        ) {
            return None;
        }
        if args
            .children
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case("artist.nfo"))
        {
            return Some(resolved(args, ResolvedLibraryItemKind::MusicArtist));
        }
        if args.parent_is_root {
            return None;
        }

        let contains_artist_subfolder = args
            .children
            .iter()
            .any(|entry| entry.is_directory && is_artist_subfolder(&entry.name));
        if contains_artist_subfolder {
            return Some(resolved(args, ResolvedLibraryItemKind::MusicArtist));
        }

        args.children
            .iter()
            .any(|entry| entry.is_directory && self.album_resolver.is_music_album_at(args, entry))
            .then(|| resolved(args, ResolvedLibraryItemKind::MusicArtist))
    }
}

/// Official `MusicAlbumResolver`: a music-library directory containing audio
/// files, or a multi-disc container whose subfolders hold the actual tracks.
#[derive(Debug, Clone)]
pub struct MusicAlbumResolver {
    naming_options: Arc<NamingOptions>,
    album_parser: AlbumParser,
}

impl MusicAlbumResolver {
    #[must_use]
    pub fn new(naming_options: impl Into<Arc<NamingOptions>>) -> Self {
        let naming_options = naming_options.into();
        Self {
            album_parser: AlbumParser::new(Arc::clone(&naming_options)),
            naming_options,
        }
    }

    #[must_use]
    pub fn is_music_album_at(
        &self,
        args: &LibraryResolveArgs<'_>,
        entry: &ResolutionFileSystemEntry,
    ) -> bool {
        let Ok(children) = args.directory_reader.get_entries(&entry.full_name) else {
            return false;
        };
        self.contains_music(&children, true, args.directory_reader)
    }

    fn contains_music(
        &self,
        children: &[ResolutionFileSystemEntry],
        allow_subfolders: bool,
        reader: &dyn DirectoryReader,
    ) -> bool {
        if children.iter().any(|entry| {
            !entry.is_directory && is_audio_file(&entry.full_name, &self.naming_options)
        }) {
            return true;
        }
        if !allow_subfolders {
            return false;
        }

        let mut disc_subfolder_count = 0;
        for entry in children.iter().filter(|entry| entry.is_directory) {
            let Ok(sub_children) = reader.get_entries(&entry.full_name) else {
                continue;
            };
            if !self.contains_music(&sub_children, false, reader) {
                continue;
            }
            if self.album_parser.is_multi_part(&entry.full_name) {
                disc_subfolder_count += 1;
            } else {
                return false;
            }
        }
        disc_subfolder_count > 0
    }
}

impl LibraryItemResolver for MusicAlbumResolver {
    fn priority(&self) -> ResolverPriority {
        ResolverPriority::Third
    }

    fn resolve(&self, args: &LibraryResolveArgs<'_>) -> Option<ResolvedLibraryItem> {
        if !args.is_directory || args.collection_type != Some(CollectionType::Music) {
            return None;
        }
        if args.parent == LibraryParentKind::MusicAlbum || args.parent_is_root {
            return None;
        }
        if args
            .parent_path
            .and_then(file_name)
            .is_some_and(is_artist_subfolder)
        {
            return None;
        }
        self.contains_music(&args.children, true, args.directory_reader)
            .then(|| resolved(args, ResolvedLibraryItemKind::MusicAlbum))
    }
}

/// Official `AudioResolver` subset for music and audiobook files.
#[derive(Debug, Clone)]
pub struct AudioLibraryResolver {
    naming_options: Arc<NamingOptions>,
}

impl AudioLibraryResolver {
    #[must_use]
    pub fn new(naming_options: impl Into<Arc<NamingOptions>>) -> Self {
        Self {
            naming_options: naming_options.into(),
        }
    }
}

impl LibraryItemResolver for AudioLibraryResolver {
    fn priority(&self) -> ResolverPriority {
        ResolverPriority::Fifth
    }

    fn resolve(&self, args: &LibraryResolveArgs<'_>) -> Option<ResolvedLibraryItem> {
        if args.is_directory {
            if args.collection_type != Some(CollectionType::Books) {
                return None;
            }
            let children = args
                .children
                .iter()
                .map(|entry| AudioFileSystemEntry::new(&entry.full_name, entry.is_directory))
                .collect::<Vec<_>>();
            let resolver = AudioResolver::new(Arc::clone(&self.naming_options));
            return resolver
                .resolve(AudioResolveArgs {
                    collection_type: args.collection_type,
                    file_info: AudioFileSystemEntry::new(args.path, true),
                    file_system_children: children,
                    parent: AudioParentContext::None,
                })
                .map(|book| ResolvedLibraryItem {
                    path: book.path,
                    kind: ResolvedLibraryItemKind::AudioBook,
                });
        }

        if !is_audio_file(args.path, &self.naming_options) {
            return None;
        }
        if is_cue_file(args.path) {
            return None;
        }
        if args.collection_type.is_none() && is_video_file(args.path, &self.naming_options) {
            return None;
        }
        Some(resolved(args, ResolvedLibraryItemKind::Audio))
    }
}

fn resolved(args: &LibraryResolveArgs<'_>, kind: ResolvedLibraryItemKind) -> ResolvedLibraryItem {
    ResolvedLibraryItem {
        path: args.path.to_owned(),
        kind,
    }
}

const ARTIST_SUBFOLDERS: &[&str] = &[
    "albums",
    "broadcasts",
    "bootlegs",
    "compilations",
    "dj-mixes",
    "eps",
    "live",
    "mixtapes",
    "others",
    "remixes",
    "singles",
    "soundtracks",
    "spokenwords",
    "streets",
];

fn is_artist_subfolder(name: &str) -> bool {
    ARTIST_SUBFOLDERS
        .iter()
        .any(|subfolder| subfolder.eq_ignore_ascii_case(name))
}

fn file_name(path: &str) -> Option<&str> {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
}

fn is_video_file(path: &str, options: &NamingOptions) -> bool {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return false;
    };
    options
        .video_file_extensions
        .iter()
        .any(|candidate| candidate[1..].eq_ignore_ascii_case(extension))
}

fn is_cue_file(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("cue"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapDirectoryReader(HashMap<String, Vec<ResolutionFileSystemEntry>>);

    impl DirectoryReader for MapDirectoryReader {
        fn get_entries(&self, path: &str) -> io::Result<Vec<ResolutionFileSystemEntry>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
    }

    fn entry(path: &str, is_directory: bool) -> ResolutionFileSystemEntry {
        ResolutionFileSystemEntry::new(path, is_directory)
    }

    fn args<'a>(
        reader: &'a dyn DirectoryReader,
        path: &'a str,
        children: Vec<ResolutionFileSystemEntry>,
        parent: LibraryParentKind,
        parent_is_root: bool,
        parent_path: Option<&'a str>,
    ) -> LibraryResolveArgs<'a> {
        LibraryResolveArgs {
            collection_type: Some(CollectionType::Music),
            path,
            is_directory: true,
            children,
            parent,
            parent_is_root,
            parent_path,
            directory_reader: reader,
        }
    }

    #[test]
    fn music_chain_resolves_artist_album_and_audio() {
        let chain = LibraryResolverChain::default_music_chain();
        let reader = MapDirectoryReader(HashMap::from([(
            "/music/Artist/Album".to_owned(),
            vec![entry("/music/Artist/Album/song.mp3", false)],
        )]));

        let artist = chain
            .resolve(&args(
                &reader,
                "/music/Artist",
                vec![entry("/music/Artist/Album", true)],
                LibraryParentKind::Folder,
                false,
                Some("/music"),
            ))
            .expect("artist directory should resolve");
        assert_eq!(artist.kind, ResolvedLibraryItemKind::MusicArtist);

        let album = chain
            .resolve(&args(
                &reader,
                "/music/Artist/Album",
                vec![entry("/music/Artist/Album/song.mp3", false)],
                LibraryParentKind::MusicArtist,
                false,
                Some("/music/Artist"),
            ))
            .expect("album directory should resolve");
        assert_eq!(album.kind, ResolvedLibraryItemKind::MusicAlbum);

        let audio = chain
            .resolve(&LibraryResolveArgs {
                collection_type: Some(CollectionType::Music),
                path: "/music/Artist/Album/song.mp3",
                is_directory: false,
                children: Vec::new(),
                parent: LibraryParentKind::MusicAlbum,
                parent_is_root: false,
                parent_path: Some("/music/Artist/Album"),
                directory_reader: &reader,
            })
            .expect("audio file should resolve");
        assert_eq!(audio.kind, ResolvedLibraryItemKind::Audio);
    }

    #[test]
    fn music_chain_rejects_root_and_nested_artists() {
        let chain = LibraryResolverChain::default_music_chain();
        let reader = MapDirectoryReader(HashMap::new());
        assert!(
            chain
                .resolve(&args(
                    &reader,
                    "/music",
                    vec![entry("/music/Artist", true)],
                    LibraryParentKind::Folder,
                    true,
                    None,
                ))
                .is_none(),
            "music collection root must not become an artist"
        );
        assert!(
            chain
                .resolve(&args(
                    &reader,
                    "/music/Artist/Nested",
                    vec![entry("/music/Artist/Nested/Album", true)],
                    LibraryParentKind::MusicArtist,
                    false,
                    Some("/music/Artist"),
                ))
                .is_none(),
            "nested artist directories are not supported"
        );
    }

    #[test]
    fn multi_disc_album_uses_subfolder_tracks() {
        let reader = MapDirectoryReader(HashMap::from([
            (
                "/music/Artist/Double Album".to_owned(),
                vec![
                    entry("/music/Artist/Double Album/CD1", true),
                    entry("/music/Artist/Double Album/CD2", true),
                ],
            ),
            (
                "/music/Artist/Double Album/CD1".to_owned(),
                vec![entry("/music/Artist/Double Album/CD1/track.flac", false)],
            ),
            (
                "/music/Artist/Double Album/CD2".to_owned(),
                vec![entry("/music/Artist/Double Album/CD2/track.flac", false)],
            ),
        ]));
        let resolver = MusicAlbumResolver::new(NamingOptions::default());
        assert!(resolver.is_music_album_at(
            &args(
                &reader,
                "/music/Artist/Double Album",
                vec![
                    entry("/music/Artist/Double Album/CD1", true),
                    entry("/music/Artist/Double Album/CD2", true),
                ],
                LibraryParentKind::MusicArtist,
                false,
                Some("/music/Artist"),
            ),
            &entry("/music/Artist/Double Album", true),
        ));
    }
}
