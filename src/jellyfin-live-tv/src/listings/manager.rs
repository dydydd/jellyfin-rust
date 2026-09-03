use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::fs::File;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

fn default_true() -> bool {
    true
}

fn default_recording_post_processor_arguments() -> String {
    "\"{path}\"".to_owned()
}

fn default_news_categories() -> Vec<String> {
    strings(&["news", "journalism", "documentary", "current affairs"])
}

fn default_sports_categories() -> Vec<String> {
    strings(&["sports", "basketball", "baseball", "football"])
}

fn default_kids_categories() -> Vec<String> {
    strings(&["kids", "family", "children", "childrens", "disney"])
}

fn default_movie_categories() -> Vec<String> {
    strings(&["movie"])
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// A channel mapping persisted as part of a listings provider configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ChannelMapping {
    pub name: Option<String>,
    pub value: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Configuration used to connect a tuner to one listings provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ListingProviderConfiguration {
    pub id: Option<String>,
    #[serde(rename = "Type")]
    pub provider_type: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub listings_id: Option<String>,
    pub zip_code: Option<String>,
    pub country: Option<String>,
    pub path: Option<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub enabled_tuners: Vec<String>,
    #[serde(default = "default_true")]
    pub enable_all_tuners: bool,
    #[serde(
        default = "default_news_categories",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub news_categories: Vec<String>,
    #[serde(
        default = "default_sports_categories",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub sports_categories: Vec<String>,
    #[serde(
        default = "default_kids_categories",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub kids_categories: Vec<String>,
    #[serde(
        default = "default_movie_categories",
        deserialize_with = "deserialize_vec_or_default"
    )]
    pub movie_categories: Vec<String>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub channel_mappings: Vec<ChannelMapping>,
    pub movie_prefix: Option<String>,
    pub preferred_language: Option<String>,
    pub user_agent: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for ListingProviderConfiguration {
    fn default() -> Self {
        Self {
            id: None,
            provider_type: None,
            username: None,
            password: None,
            listings_id: None,
            zip_code: None,
            country: None,
            path: None,
            enabled_tuners: Vec::new(),
            enable_all_tuners: true,
            news_categories: default_news_categories(),
            sports_categories: default_sports_categories(),
            kids_categories: default_kids_categories(),
            movie_categories: default_movie_categories(),
            channel_mappings: Vec::new(),
            movie_prefix: None,
            preferred_language: None,
            user_agent: None,
            extra: BTreeMap::new(),
        }
    }
}

/// The persisted `livetv` server configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LiveTvConfiguration {
    pub guide_days: Option<i32>,
    pub recording_path: Option<String>,
    pub movie_recording_path: Option<String>,
    pub series_recording_path: Option<String>,
    pub enable_recording_subfolders: bool,
    pub enable_original_audio_with_encoded_recordings: bool,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub tuner_hosts: Vec<Value>,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub listing_providers: Vec<ListingProviderConfiguration>,
    pub pre_padding_seconds: i32,
    pub post_padding_seconds: i32,
    #[serde(deserialize_with = "deserialize_vec_or_default")]
    pub media_locations_created: Vec<String>,
    pub recording_post_processor: Option<String>,
    #[serde(default = "default_recording_post_processor_arguments")]
    pub recording_post_processor_arguments: String,
    #[serde(default = "default_true")]
    pub save_recording_nfo: bool,
    #[serde(default = "default_true")]
    pub save_recording_images: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for LiveTvConfiguration {
    fn default() -> Self {
        Self {
            guide_days: None,
            recording_path: None,
            movie_recording_path: None,
            series_recording_path: None,
            enable_recording_subfolders: false,
            enable_original_audio_with_encoded_recordings: false,
            tuner_hosts: Vec::new(),
            listing_providers: Vec::new(),
            pre_padding_seconds: 0,
            post_padding_seconds: 0,
            media_locations_created: Vec::new(),
            recording_post_processor: None,
            recording_post_processor_arguments: default_recording_post_processor_arguments(),
            save_recording_nfo: true,
            save_recording_images: true,
            extra: BTreeMap::new(),
        }
    }
}

/// An error reading or atomically updating the Live TV configuration.
#[derive(Debug)]
pub enum ListingsConfigurationError {
    Io {
        operation: &'static str,
        path: Arc<PathBuf>,
        source: std::io::Error,
    },
    InvalidJson {
        path: Arc<PathBuf>,
        source: serde_json::Error,
    },
    Serialize(serde_json::Error),
    LockPoisoned,
    InvalidPath(Arc<PathBuf>),
}

impl fmt::Display for ListingsConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} Live TV configuration {}: {source}",
                path.display()
            ),
            Self::InvalidJson { path, source } => write!(
                formatter,
                "invalid Live TV configuration {}: {source}",
                path.display()
            ),
            Self::Serialize(source) => {
                write!(
                    formatter,
                    "failed to serialize Live TV configuration: {source}"
                )
            }
            Self::LockPoisoned => formatter.write_str("Live TV configuration lock is poisoned"),
            Self::InvalidPath(path) => write!(
                formatter,
                "Live TV configuration path has no file name: {}",
                path.display()
            ),
        }
    }
}

impl Error for ListingsConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidJson { source, .. } | Self::Serialize(source) => Some(source),
            Self::LockPoisoned | Self::InvalidPath(_) => None,
        }
    }
}

/// Injectable persistence boundary for the `livetv` server configuration.
pub trait ListingsConfigurationStore: Send + Sync {
    fn load(&self) -> Result<LiveTvConfiguration, ListingsConfigurationError>;

    /// Runs `mutation` exactly once while holding the store's update lock.
    /// The store persists the configuration only when the mutation returns `true`.
    fn mutate(
        &self,
        mutation: &mut dyn FnMut(&mut LiveTvConfiguration) -> bool,
    ) -> Result<bool, ListingsConfigurationError>;
}

/// JSON file-backed Live TV configuration store using atomic replacement.
pub struct JsonListingsConfigurationStore {
    path: Arc<PathBuf>,
    update_lock: Mutex<()>,
}

impl JsonListingsConfigurationStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Arc::new(path.into()),
            update_lock: Mutex::new(()),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, ListingsConfigurationError> {
        self.update_lock
            .lock()
            .map_err(|_| ListingsConfigurationError::LockPoisoned)
    }

    fn load_unlocked(&self) -> Result<LiveTvConfiguration, ListingsConfigurationError> {
        match fs::read(self.path.as_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| {
                ListingsConfigurationError::InvalidJson {
                    path: Arc::clone(&self.path),
                    source,
                }
            }),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(LiveTvConfiguration::default())
            }
            Err(source) => Err(ListingsConfigurationError::Io {
                operation: "read",
                path: Arc::clone(&self.path),
                source,
            }),
        }
    }

    fn save_unlocked(
        &self,
        configuration: &LiveTvConfiguration,
    ) -> Result<(), ListingsConfigurationError> {
        let mut bytes = serde_json::to_vec_pretty(configuration)
            .map_err(ListingsConfigurationError::Serialize)?;
        bytes.push(b'\n');

        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| ListingsConfigurationError::Io {
            operation: "create parent directory for",
            path: Arc::clone(&self.path),
            source,
        })?;

        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ListingsConfigurationError::InvalidPath(Arc::clone(&self.path)))?;
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(
            ".{file_name}.{}.{timestamp}.{sequence}.tmp",
            std::process::id(),
        ));

        let result = write_and_replace(&temporary, &self.path, &bytes);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl ListingsConfigurationStore for JsonListingsConfigurationStore {
    fn load(&self) -> Result<LiveTvConfiguration, ListingsConfigurationError> {
        let _guard = self.lock()?;
        self.load_unlocked()
    }

    fn mutate(
        &self,
        mutation: &mut dyn FnMut(&mut LiveTvConfiguration) -> bool,
    ) -> Result<bool, ListingsConfigurationError> {
        let _guard = self.lock()?;
        let mut configuration = self.load_unlocked()?;
        let changed = mutation(&mut configuration);
        if changed {
            self.save_unlocked(&configuration)?;
        }
        Ok(changed)
    }
}

/// In-memory store for embedding and deterministic tests.
pub struct MemoryListingsConfigurationStore {
    configuration: Mutex<LiveTvConfiguration>,
}

impl MemoryListingsConfigurationStore {
    #[must_use]
    pub fn new(configuration: LiveTvConfiguration) -> Self {
        Self {
            configuration: Mutex::new(configuration),
        }
    }
}

impl Default for MemoryListingsConfigurationStore {
    fn default() -> Self {
        Self::new(LiveTvConfiguration::default())
    }
}

impl ListingsConfigurationStore for MemoryListingsConfigurationStore {
    fn load(&self) -> Result<LiveTvConfiguration, ListingsConfigurationError> {
        self.configuration
            .lock()
            .map(|configuration| configuration.clone())
            .map_err(|_| ListingsConfigurationError::LockPoisoned)
    }

    fn mutate(
        &self,
        mutation: &mut dyn FnMut(&mut LiveTvConfiguration) -> bool,
    ) -> Result<bool, ListingsConfigurationError> {
        let mut configuration = self
            .configuration
            .lock()
            .map_err(|_| ListingsConfigurationError::LockPoisoned)?;
        let mut candidate = configuration.clone();
        let changed = mutation(&mut candidate);
        if changed {
            *configuration = candidate;
        }
        Ok(changed)
    }
}

/// Coordinates listings provider configuration updates.
#[derive(Clone)]
pub struct ListingsManager {
    store: Arc<dyn ListingsConfigurationStore>,
}

impl ListingsManager {
    #[must_use]
    pub fn new(store: Arc<dyn ListingsConfigurationStore>) -> Self {
        Self { store }
    }

    /// Deletes a provider by its case-insensitive configuration id.
    ///
    /// Returns `true` only when a provider was removed and the updated
    /// configuration was persisted.
    pub fn delete_listings_provider(&self, id: &str) -> Result<bool, ListingsConfigurationError> {
        self.store.mutate(&mut |configuration| {
            let original_len = configuration.listing_providers.len();
            configuration.listing_providers.retain(|provider| {
                provider
                    .id
                    .as_deref()
                    .is_none_or(|provider_id| !provider_id.eq_ignore_ascii_case(id))
            });
            configuration.listing_providers.len() != original_len
        })
    }

    pub fn configuration(&self) -> Result<LiveTvConfiguration, ListingsConfigurationError> {
        self.store.load()
    }
}

fn write_and_replace(
    temporary: &Path,
    destination: &Arc<PathBuf>,
    bytes: &[u8],
) -> Result<(), ListingsConfigurationError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(temporary)
        .map_err(|source| ListingsConfigurationError::Io {
            operation: "create temporary file for",
            path: Arc::clone(destination),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| ListingsConfigurationError::Io {
            operation: "write temporary file for",
            path: Arc::clone(destination),
            source,
        })?;
    drop(file);

    fs::rename(temporary, destination.as_path()).map_err(|source| {
        ListingsConfigurationError::Io {
            operation: "replace",
            path: Arc::clone(destination),
            source,
        }
    })?;

    #[cfg(unix)]
    {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| ListingsConfigurationError::Io {
                operation: "sync parent directory for",
                path: Arc::clone(destination),
                source,
            })?;
    }

    Ok(())
}

fn deserialize_vec_or_default<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<Vec<T>>::deserialize(deserializer).map(Option::unwrap_or_default)
}
