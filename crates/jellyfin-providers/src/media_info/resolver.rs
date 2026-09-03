use std::sync::Arc;

use jellyfin_model::{MediaProtocol, MediaStream, MediaStreamType, MimeTypes};
use jellyfin_naming::{
    DlnaProfileType, ExternalPathParser, ExternalPathParserResult, LocalizationManager,
    NamingOptions,
};

/// A directory-service entry considered while resolving external media.
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

/// Filesystem snapshot used by an external media resolver.
#[derive(Clone, Copy, Debug)]
pub struct MediaResolveRequest<'a> {
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
pub struct ResolvedExternalStream {
    pub stream: MediaStream,
    pub mime_type: String,
}

/// Request passed to a capability that inspects one external media file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalMediaInfoRequest<'a> {
    pub path: &'a str,
    pub protocol: MediaProtocol,
    pub profile_type: DlnaProfileType,
    pub stream_type: MediaStreamType,
}

/// Boundary for media inspection implementations such as an ffprobe adapter.
pub trait ExternalMediaInfoCapability {
    type Error;

    /// Returns the streams inspected from one external media file.
    ///
    /// # Errors
    ///
    /// Returns an implementation-defined error when the file cannot be inspected.
    fn get_media_info(
        &self,
        request: ExternalMediaInfoRequest<'_>,
    ) -> Result<Vec<MediaStream>, Self::Error>;
}

pub(crate) struct ExternalStreamResolver<'a, L: LocalizationManager + ?Sized> {
    naming_options: Arc<NamingOptions>,
    path_parser: ExternalPathParser<'a, L>,
    profile_type: DlnaProfileType,
    stream_type: MediaStreamType,
}

impl<'a, L: LocalizationManager + ?Sized> ExternalStreamResolver<'a, L> {
    pub(crate) fn new(
        naming_options: impl Into<Arc<NamingOptions>>,
        localization_manager: &'a L,
        profile_type: DlnaProfileType,
        stream_type: MediaStreamType,
    ) -> Self {
        let naming_options = naming_options.into();
        let path_parser = ExternalPathParser::new(
            Arc::clone(&naming_options),
            localization_manager,
            profile_type,
        );
        Self {
            naming_options,
            path_parser,
            profile_type,
            stream_type,
        }
    }

    pub(crate) fn resolve(&self, request: MediaResolveRequest<'_>) -> Vec<ResolvedExternalStream> {
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

            let ExternalPathParserResult {
                path,
                language,
                title,
                is_default,
                is_forced,
                is_hearing_impaired,
            } = path_info;
            let mime_type = MimeTypes::get_mime_type(&path)
                .unwrap_or_else(|_| "application/octet-stream".to_owned());
            let index_offset = i32::try_from(streams.len()).unwrap_or(i32::MAX);
            let stream = MediaStream {
                index: request.start_index.saturating_add(index_offset),
                stream_type: self.stream_type,
                is_default,
                is_forced,
                is_hearing_impaired,
                is_external: true,
                path: Some(path),
                language,
                title,
                ..MediaStream::default()
            };
            streams.push(ResolvedExternalStream { stream, mime_type });
        }

        streams
    }

    pub(crate) fn resolve_with_media_info<C: ExternalMediaInfoCapability + ?Sized>(
        &self,
        request: MediaResolveRequest<'_>,
        capability: &C,
    ) -> Vec<ResolvedExternalStream> {
        let path_streams = self.resolve(request);
        let mut next_index = request.start_index;
        let mut resolved = Vec::new();

        for mut path_stream in path_streams {
            let Some(path) = path_stream.stream.path.as_deref() else {
                continue;
            };
            let media_info_request = ExternalMediaInfoRequest {
                path,
                protocol: MediaProtocol::File,
                profile_type: self.profile_type,
                stream_type: self.stream_type,
            };
            let Ok(media_streams) = capability.get_media_info(media_info_request) else {
                continue;
            };
            let is_single_stream = media_streams.len() == 1;

            let mut matching_streams = media_streams
                .into_iter()
                .filter(|stream| stream.stream_type == self.stream_type)
                .peekable();
            while let Some(mut media_stream) = matching_streams.next() {
                let is_last = matching_streams.peek().is_none();
                media_stream.index = next_index;
                next_index = next_index.saturating_add(1);
                if is_single_stream {
                    media_stream.is_default = path_stream.stream.is_default;
                    media_stream.is_forced |= path_stream.stream.is_forced;
                    media_stream.is_hearing_impaired |= path_stream.stream.is_hearing_impaired;
                }
                merge_path_metadata(&mut media_stream, &mut path_stream.stream, is_last);
                let mime_type = if is_last {
                    std::mem::take(&mut path_stream.mime_type)
                } else {
                    path_stream.mime_type.clone()
                };
                resolved.push(ResolvedExternalStream {
                    stream: media_stream,
                    mime_type,
                });
            }
        }

        resolved
    }
}

fn merge_path_metadata(
    media_stream: &mut MediaStream,
    path_stream: &mut MediaStream,
    take_owned_fields: bool,
) {
    if take_owned_fields {
        media_stream.path = path_stream.path.take();
    } else {
        media_stream.path.clone_from(&path_stream.path);
    }
    media_stream.is_external = true;
    if media_stream.title.as_deref().is_none_or(str::is_empty) {
        media_stream.title = if take_owned_fields {
            path_stream.title.take().filter(|value| !value.is_empty())
        } else {
            non_empty(path_stream.title.as_deref()).map(ToOwned::to_owned)
        };
    }
    if media_stream.language.as_deref().is_none_or(str::is_empty) {
        media_stream.language = if take_owned_fields {
            path_stream
                .language
                .take()
                .filter(|value| !value.is_empty())
        } else {
            non_empty(path_stream.language.as_deref()).map(ToOwned::to_owned)
        };
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
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
