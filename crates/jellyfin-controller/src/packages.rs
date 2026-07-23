use jellyfin_model::PackageInfo;
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
}

impl PackageService {
    #[must_use]
    pub fn new(packages: Vec<PackageInfo>) -> Self {
        Self { packages }
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
}
