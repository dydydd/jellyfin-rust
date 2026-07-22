use std::path::Path;

use jellyfin_data::{
    NewMediaPath, NewVirtualFolder, VirtualFolderError, VirtualFolderRepository,
    VirtualFolderWithPaths,
};
use sea_orm::DatabaseConnection;
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
    pub fn new(database: DatabaseConnection) -> Self {
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
        mut options: Value,
        query_paths: Vec<String>,
        refresh_requested: bool,
    ) -> Result<(), VirtualFolderServiceError> {
        validate_name(name)?;
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
        mut options: Value,
    ) -> Result<(), VirtualFolderServiceError> {
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
    let path_infos = model
        .paths
        .iter()
        .map(|path| path.path_info.clone())
        .collect::<Vec<_>>();
    let locations = model.paths.iter().map(|path| path.path.clone()).collect();
    let mut options = model.folder.library_options;
    if let Some(object) = options.as_object_mut() {
        object.insert("PathInfos".to_owned(), Value::Array(path_infos));
    }
    VirtualFolder {
        id: model.folder.id,
        name: model.folder.name,
        collection_type: model.folder.collection_type,
        library_options: options,
        locations,
        refresh_requested: model.folder.refresh_requested,
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
