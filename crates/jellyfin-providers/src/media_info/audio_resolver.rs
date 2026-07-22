use jellyfin_model::{MediaProtocol, MediaStream, MediaStreamType, MimeTypes};
use jellyfin_naming::{DlnaProfileType, ExternalPathParser, LocalizationManager, NamingOptions};

/// A directory-service entry considered while resolving external audio.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaFileSystemEntry {
    pub path: String,
    pub is_directory: bool,
}

impl MediaFileSystemEntry {
    #[must_use]
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            is_directory: false,
        }
    }

    #[must_use]
    pub fn directory(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            is_directory: true,
        }
    }
}

/// Filesystem snapshot used by [`AudioResolver`].
#[derive(Clone, Copy, Debug)]
pub struct AudioResolveRequest<'a> {
    pub media_path: &'a str,
    pub protocol: MediaProtocol,
    pub media_is_directory: bool,
    pub containing_directory_exists: bool,
    pub directory_entries: &'a [MediaFileSystemEntry],
    pub metadata_directory_exists: bool,
    pub metadata_entries: &'a [MediaFileSystemEntry],
    pub start_index: i32,
}

/// Resolved model stream and its extension-derived MIME type.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAudioStream {
    pub stream: MediaStream,
    pub mime_type: String,
}

/// Resolves external audio files associated with a local video file.
pub struct AudioResolver<'a, L: LocalizationManager + ?Sized> {
    naming_options: NamingOptions,
    path_parser: ExternalPathParser<'a, L>,
}

impl<'a, L: LocalizationManager + ?Sized> AudioResolver<'a, L> {
    pub fn new(naming_options: NamingOptions, localization_manager: &'a L) -> Self {
        let path_parser = ExternalPathParser::new(
            naming_options.clone(),
            localization_manager,
            DlnaProfileType::Audio,
        );
        Self {
            naming_options,
            path_parser,
        }
    }

    /// Resolves external audio candidates without probing their contents.
    #[must_use]
    pub fn resolve(&self, request: AudioResolveRequest<'_>) -> Vec<ResolvedAudioStream> {
        if request.protocol != MediaProtocol::File
            || request.media_is_directory
            || request.media_path.is_empty()
            || !request.containing_directory_exists
        {
            return Vec::new();
        }

        let Some(prefix) = file_stem(request.media_path).filter(|prefix| !prefix.is_empty()) else {
            return Vec::new();
        };

        let metadata_entries = if request.metadata_directory_exists {
            request.metadata_entries
        } else {
            &[]
        };
        let candidates = request.directory_entries.iter().chain(metadata_entries);
        let mut streams = Vec::new();

        for entry in candidates {
            if entry.is_directory
                || entry.path == request.media_path
                || entry.path.ends_with_ignore_ascii_case(".strm")
            {
                continue;
            }

            let Some(candidate_stem) = file_stem(&entry.path) else {
                continue;
            };
            let Some(extra) = matching_suffix(
                candidate_stem,
                prefix,
                &self.naming_options.media_flag_delimiters,
            ) else {
                continue;
            };
            let Some(path_info) = self.path_parser.parse_file(&entry.path, Some(extra)) else {
                continue;
            };

            let index_offset = i32::try_from(streams.len()).unwrap_or(i32::MAX);
            let stream = MediaStream {
                index: request.start_index.saturating_add(index_offset),
                stream_type: MediaStreamType::Audio,
                is_default: path_info.is_default,
                is_forced: path_info.is_forced,
                is_hearing_impaired: path_info.is_hearing_impaired,
                is_external: true,
                path: Some(path_info.path.clone()),
                language: path_info.language,
                title: path_info.title,
                ..MediaStream::default()
            };
            let mime_type = MimeTypes::get_mime_type(&path_info.path)
                .unwrap_or_else(|_| "application/octet-stream".to_owned());
            streams.push(ResolvedAudioStream { stream, mime_type });
        }

        streams
    }
}

fn file_name(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then_some(name)
}

fn file_stem(path: &str) -> Option<&str> {
    let name = file_name(path)?;
    Some(name.rsplit_once('.').map_or(name, |(stem, _)| stem))
}

fn matching_suffix<'a>(candidate: &'a str, prefix: &str, delimiters: &[char]) -> Option<&'a str> {
    let candidate_prefix = candidate.get(..prefix.len())?;
    if !candidate_prefix.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let suffix = candidate.get(prefix.len()..)?;
    if suffix.is_empty()
        || suffix
            .chars()
            .next()
            .is_some_and(|character| delimiters.contains(&character))
    {
        Some(suffix)
    } else {
        None
    }
}

trait EndsWithIgnoreAsciiCase {
    fn ends_with_ignore_ascii_case(&self, suffix: &str) -> bool;
}

impl EndsWithIgnoreAsciiCase for str {
    fn ends_with_ignore_ascii_case(&self, suffix: &str) -> bool {
        self.get(self.len().saturating_sub(suffix.len())..)
            .is_some_and(|ending| ending.eq_ignore_ascii_case(suffix))
    }
}
