use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardPage {
    pub name: String,
    pub resource_root: PathBuf,
    pub resource_path: PathBuf,
    pub enable_in_main_menu: bool,
    pub menu_section: Option<String>,
    pub menu_icon: Option<String>,
    pub display_name: Option<String>,
    pub plugin_id: Option<Uuid>,
}

impl DashboardPage {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        resource_root: impl Into<PathBuf>,
        resource_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            resource_root: resource_root.into(),
            resource_path: resource_path.into(),
            enable_in_main_menu: false,
            menu_section: None,
            menu_icon: None,
            display_name: None,
            plugin_id: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum DashboardError {
    #[error("dashboard page was not found")]
    NotFound,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Default)]
pub struct DashboardService {
    pages: Arc<[DashboardPage]>,
}

impl DashboardService {
    #[must_use]
    pub fn new(pages: Vec<DashboardPage>) -> Self {
        Self {
            pages: pages.into(),
        }
    }

    #[must_use]
    pub fn configuration_pages(&self, enable_in_main_menu: Option<bool>) -> Vec<DashboardPage> {
        self.pages
            .iter()
            .filter(|page| {
                enable_in_main_menu.is_none_or(|enabled| page.enable_in_main_menu == enabled)
            })
            .cloned()
            .collect()
    }

    /// Resolves a registered plugin resource without allowing it to escape its root.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for unknown, missing, non-file, absolute, traversing, or
    /// symlink-escaping resources. Other filesystem errors are preserved.
    pub async fn resolve_page(&self, name: &str) -> Result<PathBuf, DashboardError> {
        let page = self
            .pages
            .iter()
            .find(|page| page.name.eq_ignore_ascii_case(name))
            .ok_or(DashboardError::NotFound)?;
        if !safe_relative_path(&page.resource_path) {
            return Err(DashboardError::NotFound);
        }
        let root = canonicalize_or_not_found(&page.resource_root).await?;
        if !metadata_or_not_found(&root).await?.is_dir() {
            return Err(DashboardError::NotFound);
        }
        let resource = canonicalize_or_not_found(&root.join(&page.resource_path)).await?;
        if !resource.starts_with(&root) || !metadata_or_not_found(&resource).await?.is_file() {
            return Err(DashboardError::NotFound);
        }
        Ok(resource)
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

async fn canonicalize_or_not_found(path: &Path) -> Result<PathBuf, DashboardError> {
    match tokio::fs::canonicalize(path).await {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(DashboardError::NotFound),
        Err(error) => Err(error.into()),
    }
}

async fn metadata_or_not_found(path: &Path) -> Result<std::fs::Metadata, DashboardError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(DashboardError::NotFound),
        Err(error) => Err(error.into()),
    }
}
