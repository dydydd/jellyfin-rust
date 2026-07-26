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

    /// Finds a package install candidate using Jellyfin's install route
    /// filters. The actual plugin installation step is intentionally left to
    /// the host; this service validates that a compatible advertised package
    /// exists before the API returns success.
    ///
    /// # Errors
    ///
    /// Returns [`PackageError::NotFound`] when no package/version matches.
    pub fn install_candidate(
        &self,
        name: &str,
        assembly_guid: Option<Uuid>,
        version: Option<&str>,
        repository_url: Option<&str>,
    ) -> Result<PackageInfo, PackageError> {
        let package = self.get(name, assembly_guid)?;
        if package.versions.iter().any(|candidate| {
            version_matches(candidate, version) && repository_matches(candidate, repository_url)
        }) {
            Ok(package)
        } else {
            Err(PackageError::NotFound)
        }
    }
}

fn version_matches(candidate: &serde_json::Value, version: Option<&str>) -> bool {
    let Some(version) = version.filter(|value| !value.trim().is_empty()) else {
        return true;
    };
    candidate
        .get("version")
        .or_else(|| candidate.get("Version"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(version))
}

fn repository_matches(candidate: &serde_json::Value, repository_url: Option<&str>) -> bool {
    let Some(repository_url) = repository_url.filter(|value| !value.trim().is_empty()) else {
        return true;
    };
    candidate
        .get("repositoryUrl")
        .or_else(|| candidate.get("RepositoryUrl"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(repository_url))
}
