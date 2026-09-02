use jellyfin_model::CollectionType;
use jellyfin_naming::{AudioBookInfo, AudioBookListResolver, NamingOptions, StackFileInfo};

/// File-system metadata consumed by [`AudioResolver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFileSystemEntry {
    pub full_name: String,
    pub is_directory: bool,
}

impl AudioFileSystemEntry {
    #[must_use]
    pub fn new(full_name: impl Into<String>, is_directory: bool) -> Self {
        Self {
            full_name: full_name.into(),
            is_directory,
        }
    }
}

/// Parent information that affects multi-item audio resolution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AudioParentContext {
    #[default]
    None,
    Folder {
        is_top_parent: bool,
    },
}

impl AudioParentContext {
    #[must_use]
    pub const fn folder(is_top_parent: bool) -> Self {
        Self::Folder { is_top_parent }
    }

    const fn is_top_parent(self) -> bool {
        matches!(
            self,
            Self::Folder {
                is_top_parent: true
            }
        )
    }
}

/// Context for resolving one library file-system entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioResolveArgs {
    pub collection_type: Option<CollectionType>,
    pub file_info: AudioFileSystemEntry,
    pub file_system_children: Vec<AudioFileSystemEntry>,
    pub parent: AudioParentContext,
}

/// A displayable audiobook produced by library resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAudioBook {
    pub path: String,
    pub name: String,
    pub production_year: Option<u16>,
    pub is_in_mixed_folder: bool,
}

/// The result of resolving all audiobook candidates in one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleAudioResolverResult {
    pub items: Vec<ResolvedAudioBook>,
    pub extra_files: Vec<AudioFileSystemEntry>,
}

/// Applies Jellyfin's server-level audiobook directory policy on top of naming.
#[derive(Debug, Clone)]
pub struct AudioResolver {
    naming_options: NamingOptions,
}

impl AudioResolver {
    #[must_use]
    pub fn new(naming_options: NamingOptions) -> Self {
        Self { naming_options }
    }

    /// Resolves a directory as one navigable audiobook.
    #[must_use]
    pub fn resolve(&self, args: AudioResolveArgs) -> Option<ResolvedAudioBook> {
        if !args.file_info.is_directory || args.collection_type != Some(CollectionType::Books) {
            return None;
        }

        let mut result = self.resolve_multiple_audio(
            args.parent,
            args.file_system_children,
            args.collection_type,
            false,
        )?;
        if result.items.len() != 1 {
            return None;
        }

        let mut item = result.items.pop()?;
        item.is_in_mixed_folder = false;
        file_name(&args.file_info.full_name).clone_into(&mut item.name);
        Some(item)
    }

    /// Resolves displayable audiobook items and files left for other resolvers.
    #[must_use]
    pub fn resolve_multiple(
        &self,
        parent: AudioParentContext,
        entries: Vec<AudioFileSystemEntry>,
        collection_type: Option<CollectionType>,
    ) -> Option<MultipleAudioResolverResult> {
        self.resolve_multiple_audio(parent, entries, collection_type, true)
    }

    fn resolve_multiple_audio(
        &self,
        parent: AudioParentContext,
        entries: Vec<AudioFileSystemEntry>,
        collection_type: Option<CollectionType>,
        parse_name: bool,
    ) -> Option<MultipleAudioResolverResult> {
        if collection_type != Some(CollectionType::Books) {
            return None;
        }

        let (directories, files): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(|entry| entry.is_directory);
        let naming_files = files
            .iter()
            .map(|entry| StackFileInfo::new(&entry.full_name, false))
            .collect::<Vec<_>>();
        let resolved =
            AudioBookListResolver::new(self.naming_options.clone()).resolve(&naming_files);
        let is_in_mixed_folder = resolved.len() > 1 || parent.is_top_parent();

        let mut extra_files = directories;
        extra_files.extend(
            files
                .into_iter()
                .filter(|entry| !contains_file(&resolved, &entry.full_name)),
        );
        let items = resolved
            .into_iter()
            .filter_map(|book| displayable_item(book, parse_name, is_in_mixed_folder))
            .collect();

        Some(MultipleAudioResolverResult { items, extra_files })
    }
}

fn displayable_item(
    book: AudioBookInfo,
    parse_name: bool,
    is_in_mixed_folder: bool,
) -> Option<ResolvedAudioBook> {
    if book.files.len() != 1 || !book.extras.is_empty() || !book.alternate_versions.is_empty() {
        return None;
    }

    let media = book.files.into_iter().next()?;
    let path = media.path;
    let name = if parse_name {
        book.name
    } else {
        file_stem(file_name(&path)).to_owned()
    };
    Some(ResolvedAudioBook {
        path,
        name,
        production_year: book.year,
        is_in_mixed_folder,
    })
}

fn contains_file(resolved: &[AudioBookInfo], path: &str) -> bool {
    resolved.iter().any(|book| {
        book.files
            .iter()
            .chain(&book.alternate_versions)
            .chain(&book.extras)
            .any(|file| file.path.eq_ignore_ascii_case(path))
    })
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

fn file_stem(path: &str) -> &str {
    path.rfind('.').map_or(path, |index| &path[..index])
}
