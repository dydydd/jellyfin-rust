use jellyfin_model::{PackageInfo, RepositoryInfo};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("package was not found")]
    NotFound,
}

#[derive(Clone, Default)]
pub struct PackageService {
    packages: Vec<PackageInfo>,
    repositories: Vec<RepositoryInfo>,
}

impl PackageService {
    #[must_use]
    pub fn new(packages: Vec<PackageInfo>, repositories: Vec<RepositoryInfo>) -> Self {
        Self {
            packages,
            repositories,
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<PackageInfo> {
        self.packages.clone()
    }

    /// Finds a package by assembly GUID when supplied, otherwise by
    /// case-insensitive name, mirroring Jellyfin's `FilterPackages`.
    ///
    /// # Errors
    ///
    /// Returns [`PackageError::NotFound`] when no package matches.
    pub fn get(
        &self,
        name: &str,
        assembly_guid: Option<Uuid>,
    ) -> Result<PackageInfo, PackageError> {
        self.packages
            .iter()
            .find(|package| match assembly_guid.filter(|id| !id.is_nil()) {
                Some(id) => package.id == id,
                None => package.name.eq_ignore_ascii_case(name),
            })
            .cloned()
            .ok_or(PackageError::NotFound)
    }

    #[must_use]
    pub fn repositories(&self) -> Vec<RepositoryInfo> {
        self.repositories.clone()
    }
}
