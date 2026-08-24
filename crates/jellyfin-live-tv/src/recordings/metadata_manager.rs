use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use jellyfin_model::{MetadataProvider, TimerInfo};
use jellyfin_xbmc_metadata::{MovieNfo, NfoDocumentKind, NfoMetadata, parse_movie_nfo, parse_nfo};
use roxmltree::Document;
use xml::writer::{EmitterConfig, EventWriter, XmlEvent};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);
const DATE_ADDED_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// Recording metadata options consumed by [`RecordingsMetadataManager`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordingMetadataOptions {
    pub save_nfo: bool,
}

impl Default for RecordingMetadataOptions {
    fn default() -> Self {
        Self { save_nfo: true }
    }
}

/// Injectable UTC clock used to make metadata timestamps deterministic.
pub trait RecordingMetadataClock: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
}

/// Production wall clock for recording metadata timestamps.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemUtcClock;

impl RecordingMetadataClock for SystemUtcClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Parsed recording NFO content.
#[derive(Clone, Debug, PartialEq)]
pub enum RecordingMetadataDocument {
    Movie(MovieNfo),
    Episode(NfoMetadata),
}

/// Paths written by one metadata save.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SavedRecordingMetadata {
    pub recording_nfo: Option<PathBuf>,
    pub series_nfo: Option<PathBuf>,
}

/// Failure while validating, writing, or reading recording metadata.
#[derive(Debug)]
pub enum RecordingMetadataError {
    InvalidRoot(PathBuf),
    EmptyPath,
    ParentTraversal(PathBuf),
    OutsideRecordingRoot {
        root: PathBuf,
        path: PathBuf,
    },
    SymbolicLink(PathBuf),
    SidecarCollidesWithRecording(PathBuf),
    NotAFile(PathBuf),
    NotADirectory(PathBuf),
    MissingSeriesPath,
    UnsupportedNfoRoot(String),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    XmlWrite(xml::writer::Error),
    XmlParse(String),
}

impl fmt::Display for RecordingMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot(path) => {
                write!(
                    formatter,
                    "recording root is not a directory: {}",
                    path.display()
                )
            }
            Self::EmptyPath => formatter.write_str("recording path is empty"),
            Self::ParentTraversal(path) => write!(
                formatter,
                "recording path contains parent traversal: {}",
                path.display()
            ),
            Self::OutsideRecordingRoot { root, path } => write!(
                formatter,
                "recording path {} is outside configured root {}",
                path.display(),
                root.display()
            ),
            Self::SymbolicLink(path) => {
                write!(
                    formatter,
                    "symbolic links are not accepted: {}",
                    path.display()
                )
            }
            Self::SidecarCollidesWithRecording(path) => write!(
                formatter,
                "recording path already has the NFO extension: {}",
                path.display()
            ),
            Self::NotAFile(path) => {
                write!(formatter, "recording is not a file: {}", path.display())
            }
            Self::NotADirectory(path) => {
                write!(
                    formatter,
                    "series path is not a directory: {}",
                    path.display()
                )
            }
            Self::MissingSeriesPath => {
                formatter.write_str("series recordings require a series path")
            }
            Self::UnsupportedNfoRoot(root) => {
                write!(formatter, "unsupported recording NFO root: {root}")
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} {}: {source}",
                path.display()
            ),
            Self::XmlWrite(source) => write!(formatter, "failed to write recording NFO: {source}"),
            Self::XmlParse(source) => write!(formatter, "failed to parse recording NFO: {source}"),
        }
    }
}

impl Error for RecordingMetadataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::XmlWrite(source) => Some(source),
            Self::InvalidRoot(_)
            | Self::EmptyPath
            | Self::ParentTraversal(_)
            | Self::OutsideRecordingRoot { .. }
            | Self::SymbolicLink(_)
            | Self::SidecarCollidesWithRecording(_)
            | Self::NotAFile(_)
            | Self::NotADirectory(_)
            | Self::MissingSeriesPath
            | Self::UnsupportedNfoRoot(_)
            | Self::XmlParse(_) => None,
        }
    }
}

impl From<xml::writer::Error> for RecordingMetadataError {
    fn from(source: xml::writer::Error) -> Self {
        Self::XmlWrite(source)
    }
}

/// Saves and reads Kodi-compatible NFO sidecars below one recording root.
pub struct RecordingsMetadataManager {
    recording_root: PathBuf,
    options: RecordingMetadataOptions,
    clock: Arc<dyn RecordingMetadataClock>,
}

impl RecordingsMetadataManager {
    pub fn new(
        recording_root: impl AsRef<Path>,
        options: RecordingMetadataOptions,
    ) -> Result<Self, RecordingMetadataError> {
        Self::with_clock(recording_root, options, Arc::new(SystemUtcClock))
    }

    pub fn with_clock(
        recording_root: impl AsRef<Path>,
        options: RecordingMetadataOptions,
        clock: Arc<dyn RecordingMetadataClock>,
    ) -> Result<Self, RecordingMetadataError> {
        let requested_root = recording_root.as_ref();
        let metadata =
            fs::metadata(requested_root).map_err(|source| RecordingMetadataError::Io {
                operation: "inspect recording root",
                path: requested_root.to_path_buf(),
                source,
            })?;
        if !metadata.is_dir() {
            return Err(RecordingMetadataError::InvalidRoot(
                requested_root.to_path_buf(),
            ));
        }
        let recording_root =
            fs::canonicalize(requested_root).map_err(|source| RecordingMetadataError::Io {
                operation: "canonicalize recording root",
                path: requested_root.to_path_buf(),
                source,
            })?;
        Ok(Self {
            recording_root,
            options,
            clock,
        })
    }

    #[must_use]
    pub fn recording_root(&self) -> &Path {
        &self.recording_root
    }

    /// Writes a recording sidecar and, for episodes, a series sidecar.
    /// Existing files are atomically replaced with complete XML documents.
    pub fn save_recording_metadata(
        &self,
        timer: &TimerInfo,
        recording_path: impl AsRef<Path>,
        series_path: Option<&Path>,
    ) -> Result<SavedRecordingMetadata, RecordingMetadataError> {
        let recording_path = self.resolve_recording_file(recording_path.as_ref())?;
        if !self.options.save_nfo {
            return Ok(SavedRecordingMetadata::default());
        }

        let recording_nfo = recording_nfo_path(&recording_path)?;
        self.reject_existing_symlink(&recording_nfo)?;
        let series_nfo = if timer.is_program_series {
            let series_path = series_path.ok_or(RecordingMetadataError::MissingSeriesPath)?;
            let series_path = self.resolve_series_directory(series_path)?;
            let nfo_path = series_path.join("tvshow.nfo");
            self.reject_existing_symlink(&nfo_path)?;
            Some(nfo_path)
        } else {
            None
        };

        let date_added = self.clock.now_utc();
        let recording_xml = recording_xml(timer, date_added)?;
        atomic_write(&recording_nfo, &recording_xml)?;
        if let Some(series_nfo) = &series_nfo {
            atomic_write(series_nfo, &series_xml(timer)?)?;
        }

        Ok(SavedRecordingMetadata {
            recording_nfo: Some(recording_nfo),
            series_nfo,
        })
    }

    /// Reads and parses the sidecar belonging to a recording path.
    pub fn read_recording_metadata(
        &self,
        recording_path: impl AsRef<Path>,
    ) -> Result<RecordingMetadataDocument, RecordingMetadataError> {
        let recording_path = self.resolve_recording_file(recording_path.as_ref())?;
        let nfo_path = recording_nfo_path(&recording_path)?;
        let nfo_path = self.resolve_existing_file(&nfo_path)?;
        let input = fs::read_to_string(&nfo_path).map_err(|source| RecordingMetadataError::Io {
            operation: "read recording NFO",
            path: nfo_path,
            source,
        })?;
        let document = Document::parse(&input)
            .map_err(|source| RecordingMetadataError::XmlParse(source.to_string()))?;
        match document.root_element().tag_name().name() {
            "movie" => parse_movie_nfo(&input)
                .map(RecordingMetadataDocument::Movie)
                .map_err(|source| RecordingMetadataError::XmlParse(source.to_string())),
            "episodedetails" => parse_nfo(&input, NfoDocumentKind::Episode)
                .map(RecordingMetadataDocument::Episode)
                .map_err(|source| RecordingMetadataError::XmlParse(source.to_string())),
            root => Err(RecordingMetadataError::UnsupportedNfoRoot(root.to_owned())),
        }
    }

    /// Reads the `tvshow.nfo` belonging to a series directory.
    pub fn read_series_metadata(
        &self,
        series_path: impl AsRef<Path>,
    ) -> Result<NfoMetadata, RecordingMetadataError> {
        let series_path = self.resolve_series_directory(series_path.as_ref())?;
        let nfo_path = self.resolve_existing_file(&series_path.join("tvshow.nfo"))?;
        let input = fs::read_to_string(&nfo_path).map_err(|source| RecordingMetadataError::Io {
            operation: "read series NFO",
            path: nfo_path,
            source,
        })?;
        parse_nfo(&input, NfoDocumentKind::Series)
            .map_err(|source| RecordingMetadataError::XmlParse(source.to_string()))
    }

    fn resolve_recording_file(&self, path: &Path) -> Result<PathBuf, RecordingMetadataError> {
        let resolved = self.resolve_existing_path(path)?;
        let metadata = match fs::metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(source) => {
                return Err(RecordingMetadataError::Io {
                    operation: "inspect recording",
                    path: resolved,
                    source,
                });
            }
        };
        if !metadata.is_file() {
            return Err(RecordingMetadataError::NotAFile(resolved));
        }
        Ok(resolved)
    }

    fn resolve_series_directory(&self, path: &Path) -> Result<PathBuf, RecordingMetadataError> {
        let resolved = self.resolve_existing_path(path)?;
        let metadata = match fs::metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(source) => {
                return Err(RecordingMetadataError::Io {
                    operation: "inspect series directory",
                    path: resolved,
                    source,
                });
            }
        };
        if !metadata.is_dir() {
            return Err(RecordingMetadataError::NotADirectory(resolved));
        }
        Ok(resolved)
    }

    fn resolve_existing_file(&self, path: &Path) -> Result<PathBuf, RecordingMetadataError> {
        self.reject_existing_symlink(path)?;
        let resolved = fs::canonicalize(path).map_err(|source| RecordingMetadataError::Io {
            operation: "canonicalize metadata file",
            path: path.to_path_buf(),
            source,
        })?;
        if !resolved.starts_with(&self.recording_root) {
            return Err(RecordingMetadataError::OutsideRecordingRoot {
                root: self.recording_root.clone(),
                path: resolved,
            });
        }
        if !resolved.is_file() {
            return Err(RecordingMetadataError::NotAFile(resolved));
        }
        Ok(resolved)
    }

    fn resolve_existing_path(&self, path: &Path) -> Result<PathBuf, RecordingMetadataError> {
        if path.as_os_str().is_empty() {
            return Err(RecordingMetadataError::EmptyPath);
        }
        if path
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(RecordingMetadataError::ParentTraversal(path.to_path_buf()));
        }

        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.recording_root.join(path)
        };
        let relative = match candidate.strip_prefix(&self.recording_root) {
            Ok(relative) => relative,
            Err(_) => {
                return Err(RecordingMetadataError::OutsideRecordingRoot {
                    root: self.recording_root.clone(),
                    path: candidate,
                });
            }
        };

        let mut current = self.recording_root.clone();
        for component in relative.components() {
            current.push(component);
            let metadata = match fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(source) => {
                    return Err(RecordingMetadataError::Io {
                        operation: "inspect path component",
                        path: current,
                        source,
                    });
                }
            };
            if metadata.file_type().is_symlink() {
                return Err(RecordingMetadataError::SymbolicLink(current));
            }
        }

        let resolved =
            fs::canonicalize(&candidate).map_err(|source| RecordingMetadataError::Io {
                operation: "canonicalize recording path",
                path: candidate,
                source,
            })?;
        if !resolved.starts_with(&self.recording_root) {
            return Err(RecordingMetadataError::OutsideRecordingRoot {
                root: self.recording_root.clone(),
                path: resolved,
            });
        }
        Ok(resolved)
    }

    fn reject_existing_symlink(&self, path: &Path) -> Result<(), RecordingMetadataError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(RecordingMetadataError::SymbolicLink(path.to_path_buf()))
            }
            Ok(_) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RecordingMetadataError::Io {
                operation: "inspect metadata path",
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

fn recording_nfo_path(recording_path: &Path) -> Result<PathBuf, RecordingMetadataError> {
    if recording_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nfo"))
    {
        return Err(RecordingMetadataError::SidecarCollidesWithRecording(
            recording_path.to_path_buf(),
        ));
    }
    Ok(recording_path.with_extension("nfo"))
}

fn recording_xml(
    timer: &TimerInfo,
    date_added: DateTime<Utc>,
) -> Result<Vec<u8>, RecordingMetadataError> {
    let root = if timer.is_program_series {
        "episodedetails"
    } else {
        "movie"
    };
    let mut output = Vec::new();
    let mut writer = xml_writer(&mut output);
    writer.write(XmlEvent::start_element(root))?;

    if timer.is_program_series {
        write_optional_element(&mut writer, "title", timer.episode_title.as_deref())?;
        if let Some(original_air_date) = timer.original_air_date {
            write_element(
                &mut writer,
                "aired",
                &original_air_date.format("%Y-%m-%d").to_string(),
            )?;
        } else if !timer.is_repeat {
            write_element(
                &mut writer,
                "aired",
                &date_added.format("%Y-%m-%d").to_string(),
            )?;
        }
        write_optional_number(&mut writer, "episode", timer.episode_number)?;
        write_optional_number(&mut writer, "season", timer.season_number)?;
    } else {
        write_optional_element(&mut writer, "title", non_empty(&timer.name))?;
        if let Some(original_air_date) = timer.original_air_date {
            let date = original_air_date.format("%Y-%m-%d").to_string();
            write_element(&mut writer, "premiered", &date)?;
            write_element(&mut writer, "releasedate", &date)?;
        }
    }

    write_element(
        &mut writer,
        "dateadded",
        &date_added.format(DATE_ADDED_FORMAT).to_string(),
    )?;
    write_optional_number(&mut writer, "year", timer.production_year)?;
    write_optional_element(&mut writer, "mpaa", timer.official_rating.as_deref())?;
    write_element(
        &mut writer,
        "plot",
        timer.overview.as_deref().unwrap_or_default(),
    )?;
    if let Some(rating) = timer.community_rating.filter(|rating| rating.is_finite()) {
        write_element(&mut writer, "rating", &rating.to_string())?;
    }
    for genre in recording_genres(timer) {
        write_element(&mut writer, "genre", &genre)?;
    }

    write_provider_ids(&mut writer, timer, timer.is_program_series)?;
    if !timer.is_program_series && !timer.is_movie && timer.provider_ids.is_empty() {
        write_element(&mut writer, "lockdata", "true")?;
    }

    writer.write(XmlEvent::end_element())?;
    drop(writer);
    Ok(output)
}

fn series_xml(timer: &TimerInfo) -> Result<Vec<u8>, RecordingMetadataError> {
    let mut output = Vec::new();
    let mut writer = xml_writer(&mut output);
    writer.write(XmlEvent::start_element("tvshow"))?;
    write_optional_element(&mut writer, "title", non_empty(&timer.name))?;
    write_optional_element(&mut writer, "mpaa", timer.official_rating.as_deref())?;
    for genre in recording_genres(timer) {
        write_element(&mut writer, "genre", &genre)?;
    }
    write_known_provider(
        &mut writer,
        "id",
        &timer.series_provider_ids,
        MetadataProvider::Tvdb,
    )?;
    write_known_provider(
        &mut writer,
        "imdb_id",
        &timer.series_provider_ids,
        MetadataProvider::Imdb,
    )?;
    write_known_provider(
        &mut writer,
        "tmdbid",
        &timer.series_provider_ids,
        MetadataProvider::Tmdb,
    )?;
    write_known_provider_by_name(
        &mut writer,
        "zap2itid",
        &timer.series_provider_ids,
        "Zap2It",
    )?;
    writer.write(XmlEvent::end_element())?;
    drop(writer);
    Ok(output)
}

fn xml_writer(output: &mut Vec<u8>) -> EventWriter<&mut Vec<u8>> {
    EmitterConfig::new()
        .perform_indent(true)
        .write_document_declaration(true)
        .create_writer(output)
}

fn write_element<W: Write>(
    writer: &mut EventWriter<W>,
    name: &str,
    value: &str,
) -> Result<(), RecordingMetadataError> {
    writer.write(XmlEvent::start_element(name))?;
    writer.write(XmlEvent::characters(value))?;
    writer.write(XmlEvent::end_element())?;
    Ok(())
}

fn write_optional_element<W: Write>(
    writer: &mut EventWriter<W>,
    name: &str,
    value: Option<&str>,
) -> Result<(), RecordingMetadataError> {
    if let Some(value) = value.and_then(non_empty) {
        write_element(writer, name, value)?;
    }
    Ok(())
}

fn write_optional_number<W: Write>(
    writer: &mut EventWriter<W>,
    name: &str,
    value: Option<i32>,
) -> Result<(), RecordingMetadataError> {
    if let Some(value) = value {
        write_element(writer, name, &value.to_string())?;
    }
    Ok(())
}

fn write_provider_ids<W: Write>(
    writer: &mut EventWriter<W>,
    timer: &TimerInfo,
    episode: bool,
) -> Result<(), RecordingMetadataError> {
    if !episode {
        write_known_provider(writer, "id", &timer.provider_ids, MetadataProvider::Imdb)?;
    }
    write_known_provider(
        writer,
        "imdbid",
        &timer.provider_ids,
        MetadataProvider::Imdb,
    )?;
    write_known_provider(
        writer,
        "tvdbid",
        &timer.provider_ids,
        MetadataProvider::Tvdb,
    )?;
    write_known_provider(
        writer,
        "tmdbid",
        &timer.provider_ids,
        MetadataProvider::Tmdb,
    )?;
    Ok(())
}

fn write_known_provider<W: Write>(
    writer: &mut EventWriter<W>,
    element: &str,
    provider_ids: &jellyfin_model::ProviderIdMap,
    provider: MetadataProvider,
) -> Result<(), RecordingMetadataError> {
    write_known_provider_by_name(writer, element, provider_ids, provider.as_str())
}

fn write_known_provider_by_name<W: Write>(
    writer: &mut EventWriter<W>,
    element: &str,
    provider_ids: &jellyfin_model::ProviderIdMap,
    provider: &str,
) -> Result<(), RecordingMetadataError> {
    let value = provider_ids
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(provider))
        .map(|(_, value)| value.as_str());
    write_optional_element(writer, element, value)
}

fn recording_genres(timer: &TimerInfo) -> Vec<String> {
    let mut genres = timer.genres.clone();
    for (enabled, genre) in [
        (timer.is_sports, "Sports"),
        (timer.is_kids, "Kids"),
        (timer.is_kids, "Children"),
        (timer.is_news, "News"),
    ] {
        if enabled && !genres.iter().any(|value| value.eq_ignore_ascii_case(genre)) {
            genres.push(genre.to_owned());
        }
    }
    genres
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RecordingMetadataError> {
    let parent = path.parent().ok_or_else(|| RecordingMetadataError::Io {
        operation: "locate metadata directory for",
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent"),
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| RecordingMetadataError::Io {
            operation: "derive metadata file name for",
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file name"),
        })?;
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".{file_name}.{}.{timestamp}.{sequence}.tmp",
        std::process::id()
    ));

    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|source| RecordingMetadataError::Io {
                operation: "create temporary metadata file for",
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| RecordingMetadataError::Io {
                operation: "write temporary metadata file for",
                path: path.to_path_buf(),
                source,
            })?;
        drop(file);
        fs::rename(&temporary, path).map_err(|source| RecordingMetadataError::Io {
            operation: "atomically replace metadata file",
            path: path.to_path_buf(),
            source,
        })?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| RecordingMetadataError::Io {
                operation: "sync metadata directory for",
                path: path.to_path_buf(),
                source,
            })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
