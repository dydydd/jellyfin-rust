use std::path::Path;

use jellyfin_data::{
    NewMediaPath, NewVirtualFolder, VirtualFolderError, VirtualFolderRepository,
    VirtualFolderWithPaths,
};
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct VirtualFolder {
    pub id: Uuid,
    pub name: String,
    pub collection_type: Option<String>,
    pub library_options: Value,
    pub locations: Vec<String>,
    pub refresh_requested: bool,
}

#[derive(Debug, Error)]
pub enum VirtualFolderServiceError {
    #[error("library options must be a JSON object")]
    InvalidOptions,
    #[error("unknown virtual folder collection type")]
    InvalidCollectionType,
    #[error("media path cannot be empty")]
    InvalidPath,
    #[error("media path does not exist")]
    PathNotFound,
    #[error("media path is not a directory")]
    PathNotDirectory,
    #[error("media path is not valid UTF-8")]
    NonUtf8Path,
    #[error(transparent)]
    Repository(#[from] VirtualFolderError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone)]
pub struct VirtualFolderService {
    repository: VirtualFolderRepository,
}

impl VirtualFolderService {
    #[must_use]
    pub fn new(database: impl Into<jellyfin_data::SharedDatabase>) -> Self {
        Self {
            repository: VirtualFolderRepository::new(database),
        }
    }

    /// Lists virtual folders and merges relational media paths into JSON options.
    ///
    /// # Errors
    ///
    /// Returns a persistence error when the list cannot be loaded.
    pub async fn list(&self) -> Result<Vec<VirtualFolder>, VirtualFolderServiceError> {
        Ok(self
            .repository
            .list()
            .await?
            .into_iter()
            .map(folder_from_model)
            .collect())
    }

    /// Creates a virtual folder after canonicalizing every configured path.
    ///
    /// # Errors
    ///
    /// Returns validation, filesystem, conflict, or persistence errors.
    pub async fn create(
        &self,
        name: &str,
        collection_type: Option<String>,
        options: Value,
        query_paths: Vec<String>,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        let collection_type = collection_type
            .map(|value| {
                canonical_collection_type_option(&value)
                    .map(str::to_owned)
                    .ok_or(VirtualFolderServiceError::InvalidCollectionType)
            })
            .transpose()?;
        let mut options = options;
        object_options(&mut options)?;
        let mut options = normalize_library_options(options);
        let object = object_options(&mut options)?;
        let path_infos = if query_paths.is_empty() {
            object
                .remove("PathInfos")
                .or_else(|| object.remove("pathInfos"))
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default()
        } else {
            query_paths
                .into_iter()
                .map(|path| json!({ "Path": path }))
                .collect()
        };
        object.insert("PathInfos".to_owned(), Value::Array(Vec::new()));
        let paths = canonicalize_path_infos(path_infos).await?;
        self.repository
            .create(
                NewVirtualFolder {
                    name: name.to_owned(),
                    collection_type,
                    library_options: options,
                    refresh_requested,
                },
                paths,
            )
            .await?;
        Ok(())
    }

    /// Renames a virtual folder.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, missing-folder, or persistence errors.
    pub async fn rename(
        &self,
        name: &str,
        new_name: &str,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        validate_name(new_name)?;
        self.repository
            .rename(name, new_name, refresh_requested)
            .await?;
        Ok(())
    }

    /// Deletes a virtual folder and all of its media paths.
    ///
    /// # Errors
    ///
    /// Returns validation, missing-folder, or persistence errors.
    pub async fn delete(
        &self,
        name: &str,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        self.repository.delete(name, refresh_requested).await?;
        Ok(())
    }

    /// Replaces a folder's JSONB library options.
    ///
    /// Existing paths remain relationally authoritative and are reattached on reads.
    ///
    /// # Errors
    ///
    /// Returns invalid-options, missing-folder, or persistence errors.
    pub async fn update_options(
        &self,
        id: Uuid,
        options: Value,
    ) -> Result<(), VirtualFolderServiceError> {
        let mut options = options;
        object_options(&mut options)?;
        let mut options = normalize_library_options(options);
        let object = object_options(&mut options)?;
        object.remove("PathInfos");
        object.remove("pathInfos");
        object.insert("PathInfos".to_owned(), Value::Array(Vec::new()));
        self.repository.update_options(id, options).await?;
        Ok(())
    }

    /// Adds a real canonical directory to a virtual folder.
    ///
    /// # Errors
    ///
    /// Returns filesystem, overlap, missing-folder, or persistence errors.
    pub async fn add_path(
        &self,
        name: &str,
        path_info: Value,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        let path = canonicalize_path_info(path_info).await?;
        self.repository
            .add_path(name, path, refresh_requested)
            .await?;
        Ok(())
    }

    /// Updates metadata for an existing canonical path.
    ///
    /// # Errors
    ///
    /// Returns filesystem, missing-folder/path, or persistence errors.
    pub async fn update_path(
        &self,
        name: &str,
        path_info: Value,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        let path = canonicalize_path_info(path_info).await?;
        self.repository
            .update_path(name, &path.normalized_path, path.path_info)
            .await?;
        Ok(())
    }

    /// Removes an exact canonical path from a virtual folder.
    ///
    /// # Errors
    ///
    /// Returns filesystem, missing-folder/path, or persistence errors.
    pub async fn remove_path(
        &self,
        name: &str,
        path: &str,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
        validate_path(path)?;
        self.repository
            .remove_path(name, path, refresh_requested)
            .await?;
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), VirtualFolderServiceError> {
    if name.trim().is_empty() {
        Err(VirtualFolderError::InvalidName.into())
    } else {
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<(), VirtualFolderServiceError> {
    if path.trim().is_empty() {
        Err(VirtualFolderServiceError::InvalidPath)
    } else {
        Ok(())
    }
}

fn folder_from_model(model: VirtualFolderWithPaths) -> VirtualFolder {
    let mut path_infos = Vec::with_capacity(model.paths.len());
    let mut locations = Vec::with_capacity(model.paths.len());
    for path in model.paths {
        path_infos.push(path.path_info);
        locations.push(path.path);
    }
    // Older Rust releases persisted only the options explicitly submitted by
    // the client. The official server deserializes into `new LibraryOptions()`,
    // so omitted properties retain constructor defaults and are serialized on
    // every read. Normalize legacy rows before exposing this strongly typed DTO.
    let mut options = normalize_library_options(model.folder.library_options);
    options
        .as_object_mut()
        .expect("normalized library options must be an object")
        .insert("PathInfos".to_owned(), Value::Array(path_infos));
    VirtualFolder {
        id: model.folder.id,
        name: model.folder.name,
        collection_type: model
            .folder
            .collection_type
            .as_deref()
            .and_then(canonical_collection_type_option)
            .map(str::to_owned),
        library_options: options,
        locations,
        refresh_requested: model.folder.refresh_requested,
    }
}

/// Applies the defaults from the official `LibraryOptions` constructor while
/// retaining persisted values and extension properties.
///
/// Property matching is case-insensitive because ASP.NET's JSON binding is
/// case-insensitive. Canonical names are emitted so generated mobile SDKs can
/// decode the response. A legacy non-object value cannot represent official
/// `LibraryOptions` and safely falls back to a fresh default instance.
fn normalize_library_options(options: Value) -> Value {
    let mut normalized = default_library_options();
    let Some(submitted) = options.as_object() else {
        return Value::Object(normalized);
    };

    for (name, value) in submitted {
        let canonical_name = normalized
            .keys()
            .find(|candidate| candidate.eq_ignore_ascii_case(name))
            .cloned()
            .unwrap_or_else(|| name.clone());
        // Legacy rows may contain explicit nulls for constructor-backed
        // properties. Keep the official non-null default in that case while
        // still preserving null-valued extension properties verbatim.
        if value.is_null() && normalized.contains_key(&canonical_name) {
            continue;
        }
        normalized.insert(canonical_name, value.clone());
    }

    Value::Object(normalized)
}

fn default_library_options() -> Map<String, Value> {
    json!({
        "Enabled": true,
        "EnablePhotos": true,
        "EnableRealtimeMonitor": false,
        "EnableLUFSScan": false,
        "EnableChapterImageExtraction": false,
        "ExtractChapterImagesDuringLibraryScan": false,
        "EnableTrickplayImageExtraction": false,
        "ExtractTrickplayImagesDuringLibraryScan": false,
        "PathInfos": [],
        "SaveLocalMetadata": false,
        "EnableInternetProviders": false,
        "EnableAutomaticSeriesGrouping": true,
        "EnableEmbeddedTitles": false,
        "EnableEmbeddedExtrasTitles": false,
        "EnableEmbeddedEpisodeInfos": false,
        "AutomaticRefreshIntervalDays": 0,
        "SeasonZeroDisplayName": "Specials",
        "DisabledLocalMetadataReaders": [],
        "DisabledSubtitleFetchers": [],
        "SubtitleFetcherOrder": [],
        "DisabledMediaSegmentProviders": [],
        "MediaSegmentProviderOrder": [],
        "SkipSubtitlesIfEmbeddedSubtitlesPresent": false,
        "SkipSubtitlesIfAudioTrackMatches": true,
        "RequirePerfectSubtitleMatch": true,
        "SaveSubtitlesWithMedia": true,
        "SaveLyricsWithMedia": false,
        "SaveTrickplayWithMedia": false,
        "DisabledLyricFetchers": [],
        "LyricFetcherOrder": [],
        "PreferNonstandardArtistsTag": false,
        "UseCustomTagDelimiters": false,
        "CustomTagDelimiters": ["/", "|", ";", "\\"],
        "DelimiterWhitelist": [],
        "AutomaticallyAddToCollection": false,
        "AllowEmbeddedSubtitles": "AllowAll",
        "TypeOptions": []
    })
    .as_object()
    .expect("library options literal must be an object")
    .clone()
}

pub(crate) fn canonical_collection_type_option(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "movies" => Some("movies"),
        "tvshows" => Some("tvshows"),
        "music" => Some("music"),
        "musicvideos" => Some("musicvideos"),
        "homevideos" => Some("homevideos"),
        "boxsets" => Some("boxsets"),
        "books" => Some("books"),
        "mixed" => Some("mixed"),
        _ => None,
    }
}

fn object_options(
    options: &mut Value,
) -> Result<&mut Map<String, Value>, VirtualFolderServiceError> {
    options
        .as_object_mut()
        .ok_or(VirtualFolderServiceError::InvalidOptions)
}

async fn canonicalize_path_infos(
    infos: Vec<Value>,
) -> Result<Vec<NewMediaPath>, VirtualFolderServiceError> {
    let mut paths = Vec::with_capacity(infos.len());
    for info in infos {
        paths.push(canonicalize_path_info(info).await?);
    }
    Ok(paths)
}

async fn canonicalize_path_info(
    mut path_info: Value,
) -> Result<NewMediaPath, VirtualFolderServiceError> {
    let object = path_info
        .as_object_mut()
        .ok_or(VirtualFolderServiceError::InvalidPath)?;
    let path = object
        .get("Path")
        .or_else(|| object.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .ok_or(VirtualFolderServiceError::InvalidPath)?;
    let canonical = canonical_directory(path).await?;
    object.remove("path");
    object.insert("Path".to_owned(), Value::String(canonical.clone()));
    let ancestors = Path::new(&canonical)
        .ancestors()
        .map(|ancestor| ancestor.to_string_lossy().into_owned())
        .collect();
    Ok(NewMediaPath {
        path: canonical.clone(),
        normalized_path: canonical,
        ancestors,
        path_info,
    })
}

#[cfg(test)]
mod library_options_tests {
    use super::*;

    #[test]
    fn normalized_library_options_preserve_values_and_supply_sdk_defaults() {
        let options = normalize_library_options(json!({
            "Enabled": false,
            "EnablePhotos": null,
            "FutureOption": null
        }));
        assert_eq!(options["Enabled"], false);
        assert_eq!(options["EnablePhotos"], true);
        assert!(options["FutureOption"].is_null());
        for key in [
            "Enabled",
            "EnablePhotos",
            "EnableRealtimeMonitor",
            "EnableLUFSScan",
            "EnableChapterImageExtraction",
            "ExtractChapterImagesDuringLibraryScan",
            "EnableTrickplayImageExtraction",
            "ExtractTrickplayImagesDuringLibraryScan",
            "PathInfos",
            "SaveLocalMetadata",
            "EnableInternetProviders",
            "EnableAutomaticSeriesGrouping",
            "EnableEmbeddedTitles",
            "EnableEmbeddedExtrasTitles",
            "EnableEmbeddedEpisodeInfos",
            "AutomaticRefreshIntervalDays",
            "SeasonZeroDisplayName",
            "DisabledLocalMetadataReaders",
            "DisabledSubtitleFetchers",
            "SubtitleFetcherOrder",
            "DisabledMediaSegmentProviders",
            "MediaSegmentProviderOrder",
            "SkipSubtitlesIfEmbeddedSubtitlesPresent",
            "SkipSubtitlesIfAudioTrackMatches",
            "RequirePerfectSubtitleMatch",
            "SaveSubtitlesWithMedia",
            "DisabledLyricFetchers",
            "LyricFetcherOrder",
            "CustomTagDelimiters",
            "DelimiterWhitelist",
            "AutomaticallyAddToCollection",
            "AllowEmbeddedSubtitles",
            "TypeOptions",
        ] {
            assert!(!options[key].is_null(), "LibraryOptions.{key}");
        }
        assert_eq!(options["AllowEmbeddedSubtitles"], "AllowAll");
        assert_eq!(options["TypeOptions"], json!([]));
    }
}

async fn canonical_directory(path: &str) -> Result<String, VirtualFolderServiceError> {
    validate_path(path)?;
    let canonical = match tokio::fs::canonicalize(path).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VirtualFolderServiceError::PathNotFound);
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = match tokio::fs::metadata(&canonical).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VirtualFolderServiceError::PathNotFound);
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        return Err(VirtualFolderServiceError::PathNotDirectory);
    }
    canonical
        .into_os_string()
        .into_string()
        .map_err(|_| VirtualFolderServiceError::NonUtf8Path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{canonical_collection_type_option, normalize_library_options};

    #[test]
    fn collection_type_options_are_case_insensitive_and_canonical() {
        for expected in [
            "movies",
            "tvshows",
            "music",
            "musicvideos",
            "homevideos",
            "boxsets",
            "books",
            "mixed",
        ] {
            assert_eq!(
                canonical_collection_type_option(&expected.to_ascii_uppercase()),
                Some(expected)
            );
        }
        assert_eq!(
            canonical_collection_type_option("  MoViEs  "),
            Some("movies")
        );
        assert_eq!(canonical_collection_type_option("livetv"), None);
        assert_eq!(canonical_collection_type_option("not-a-collection"), None);
    }

    #[test]
    fn legacy_partial_library_options_receive_official_defaults() {
        let options = normalize_library_options(json!({
            "enabled": false,
            "seasonzerodisplayname": "Bonus",
            "PathInfos": [{ "Path": "/media" }],
            "FutureOption": "preserved"
        }));

        assert_eq!(options["Enabled"], false);
        assert_eq!(options["SeasonZeroDisplayName"], "Bonus");
        assert_eq!(options["PathInfos"], json!([{ "Path": "/media" }]));
        assert_eq!(options["FutureOption"], "preserved");
        assert!(options.get("enabled").is_none());
        assert_eq!(options["EnablePhotos"], true);
        assert_eq!(options["EnableAutomaticSeriesGrouping"], true);
        assert_eq!(options["AllowEmbeddedSubtitles"], "AllowAll");
        assert_eq!(options["CustomTagDelimiters"], json!(["/", "|", ";", "\\"]));
        assert_eq!(options["TypeOptions"], json!([]));
    }

    #[test]
    fn invalid_legacy_library_options_fall_back_to_official_defaults() {
        let options = normalize_library_options(serde_json::Value::Null);

        assert_eq!(options["Enabled"], true);
        assert_eq!(options["PathInfos"], json!([]));
        assert_eq!(options["DisabledSubtitleFetchers"], json!([]));
        assert_eq!(options["SaveSubtitlesWithMedia"], true);
    }
}
