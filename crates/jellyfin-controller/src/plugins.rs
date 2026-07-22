use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use jellyfin_model::{MimeTypes, PluginInfo};
use uuid::Uuid;

/// Public plugin metadata together with installation details owned by the
/// server runtime.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    info: PluginInfo,
    installation_directory: Option<PathBuf>,
    image_path: Option<String>,
}

impl InstalledPlugin {
    /// Creates an installed plugin whose image is resolved relative to
    /// `installation_directory`.
    #[must_use]
    pub fn new(
        info: PluginInfo,
        installation_directory: impl Into<PathBuf>,
        image_path: Option<String>,
    ) -> Self {
        Self {
            info,
            installation_directory: Some(installation_directory.into()),
            image_path,
        }
    }

    fn metadata_only(info: PluginInfo) -> Self {
        Self {
            info,
            installation_directory: None,
            image_path: None,
        }
    }

    fn image(&self) -> Option<PluginImage> {
        let installation_directory = self.installation_directory.as_deref()?;
        let image_path = self.image_path.as_deref()?;
        if image_path.trim().is_empty() {
            return None;
        }

        read_plugin_image(installation_directory, image_path)
    }
}

/// Validated plugin image file ready to return through the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginImage {
    /// Canonical path to the validated regular file.
    pub path: PathBuf,
    /// MIME type inferred using Jellyfin's MIME mapping.
    pub mime_type: String,
}

/// In-memory metadata for plugins discovered by the server runtime.
#[derive(Clone, Default)]
pub struct PluginRegistry {
    plugins: Arc<[InstalledPlugin]>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new(plugins: Vec<PluginInfo>) -> Self {
        Self::from_installed(
            plugins
                .into_iter()
                .map(InstalledPlugin::metadata_only)
                .collect(),
        )
    }

    /// Builds a registry that retains runtime installation details.
    #[must_use]
    pub fn from_installed(plugins: Vec<InstalledPlugin>) -> Self {
        Self {
            plugins: plugins.into(),
        }
    }

    /// Returns an owned, name-ordered snapshot of installed plugins.
    #[must_use]
    pub fn plugins(&self) -> Vec<PluginInfo> {
        let mut plugins = self
            .plugins
            .iter()
            .map(|plugin| plugin.info.clone())
            .collect::<Vec<_>>();
        plugins.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.version.cmp(&right.version))
        });
        plugins
    }

    /// Reads the image belonging to the exact plugin id and version.
    #[must_use]
    pub fn image(&self, plugin_id: Uuid, version: &str) -> Option<PluginImage> {
        self.plugins
            .iter()
            .find(|plugin| plugin.info.id == plugin_id && plugin.info.version == version)
            .and_then(InstalledPlugin::image)
    }
}

fn read_plugin_image(installation_directory: &Path, image_path: &str) -> Option<PluginImage> {
    let relative_path = normalize_relative_path(Path::new(image_path))?;
    let plugin_root = installation_directory.canonicalize().ok()?;
    if !plugin_root.metadata().ok()?.is_dir() {
        return None;
    }

    let mut candidate = plugin_root.clone();
    let mut components = relative_path.components().peekable();
    while let Some(Component::Normal(component)) = components.next() {
        candidate.push(component);
        let metadata = fs::symlink_metadata(&candidate).ok()?;
        if metadata.file_type().is_symlink() || (components.peek().is_some() && !metadata.is_dir())
        {
            return None;
        }
    }

    let canonical_candidate = candidate.canonicalize().ok()?;
    if !canonical_candidate.starts_with(&plugin_root)
        || !canonical_candidate.metadata().ok()?.is_file()
    {
        return None;
    }

    Some(PluginImage {
        path: canonical_candidate,
        mime_type: MimeTypes::get_mime_type(image_path).ok()?,
    })
}

fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir if normalized.pop() => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use jellyfin_model::PluginStatus;

    use super::*;

    fn plugin(name: &str, id: u128) -> PluginInfo {
        PluginInfo {
            name: name.to_owned(),
            version: "1.0.0.0".to_owned(),
            configuration_file_name: None,
            description: String::new(),
            id: Uuid::from_u128(id),
            can_uninstall: true,
            has_image: false,
            status: PluginStatus::Active,
        }
    }

    #[test]
    fn snapshots_are_sorted_without_mutating_registration_order() {
        let registry = PluginRegistry::new(vec![
            plugin("Zulu", 3),
            plugin("Alpha", 2),
            plugin("Alpha", 1),
        ]);

        let first = registry.plugins();
        let second = registry.plugins();
        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|plugin| (plugin.name.as_str(), plugin.id.as_u128()))
                .collect::<Vec<_>>(),
            [("Alpha", 1), ("Alpha", 2), ("Zulu", 3)]
        );
    }

    #[test]
    fn image_lookup_requires_the_exact_id_and_version() {
        let directory = TempDirectory::new();
        let image_bytes = [0x89, b'P', b'N', b'G'];
        fs::write(directory.path().join("logo.png"), image_bytes).unwrap();
        let plugin = plugin("Test", 1);
        let plugin_id = plugin.id;
        let registry = PluginRegistry::from_installed(vec![InstalledPlugin::new(
            plugin,
            directory.path(),
            Some("logo.png".to_owned()),
        )]);

        assert!(registry.image(Uuid::from_u128(2), "1.0.0.0").is_none());
        assert!(registry.image(plugin_id, "2.0.0.0").is_none());
        assert_eq!(
            registry.image(plugin_id, "1.0.0.0"),
            Some(PluginImage {
                path: directory.path().join("logo.png").canonicalize().unwrap(),
                mime_type: "image/png".to_owned(),
            })
        );
        assert_eq!(
            fs::read(registry.image(plugin_id, "1.0.0.0").unwrap().path).unwrap(),
            image_bytes
        );
    }

    #[test]
    fn image_path_may_normalize_within_the_plugin_directory() {
        let directory = TempDirectory::new();
        fs::write(directory.path().join("logo.png"), b"image").unwrap();
        let plugin = plugin("Test", 1);
        let plugin_id = plugin.id;
        let registry = PluginRegistry::from_installed(vec![InstalledPlugin::new(
            plugin,
            directory.path(),
            Some("unused/../logo.png".to_owned()),
        )]);

        let image = registry.image(plugin_id, "1.0.0.0").unwrap();
        assert_eq!(
            image.path,
            directory.path().join("logo.png").canonicalize().unwrap()
        );
        assert_eq!(fs::read(image.path).unwrap(), b"image");
    }

    #[test]
    fn image_paths_cannot_escape_the_plugin_directory() {
        let directory = TempDirectory::new();
        let sibling = directory.sibling("-evil");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("logo.png"), b"outside").unwrap();
        let sibling_name = sibling.file_name().unwrap().to_string_lossy();

        for image_path in [
            format!("../{sibling_name}/logo.png"),
            sibling.join("logo.png").to_string_lossy().into_owned(),
            "subdirectory/../../logo.png".to_owned(),
        ] {
            let plugin = plugin("Test", 1);
            let plugin_id = plugin.id;
            let registry = PluginRegistry::from_installed(vec![InstalledPlugin::new(
                plugin,
                directory.path(),
                Some(image_path),
            )]);
            assert!(registry.image(plugin_id, "1.0.0.0").is_none());
        }

        fs::remove_dir_all(sibling).unwrap();
    }

    #[test]
    fn blank_and_missing_image_paths_do_not_resolve() {
        let directory = TempDirectory::new();

        for image_path in [None, Some(String::new()), Some("   ".to_owned())] {
            let plugin = plugin("Test", 1);
            let plugin_id = plugin.id;
            let registry = PluginRegistry::from_installed(vec![InstalledPlugin::new(
                plugin,
                directory.path(),
                image_path,
            )]);
            assert!(registry.image(plugin_id, "1.0.0.0").is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_image_components_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = TempDirectory::new();
        let outside_directory = directory.sibling("-outside");
        fs::create_dir_all(&outside_directory).unwrap();
        fs::write(outside_directory.join("logo.png"), b"outside").unwrap();
        symlink(
            outside_directory.join("logo.png"),
            directory.path().join("logo.png"),
        )
        .unwrap();
        symlink(
            &outside_directory,
            directory.path().join("linked-directory"),
        )
        .unwrap();

        for image_path in ["logo.png", "linked-directory/logo.png"] {
            let plugin = plugin("Test", 1);
            let plugin_id = plugin.id;
            let registry = PluginRegistry::from_installed(vec![InstalledPlugin::new(
                plugin,
                directory.path(),
                Some(image_path.to_owned()),
            )]);
            assert!(registry.image(plugin_id, "1.0.0.0").is_none());
        }

        fs::remove_dir_all(outside_directory).unwrap();
    }

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "jellyfin-plugin-controller-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn sibling(&self, suffix: &str) -> PathBuf {
            self.0.with_file_name(format!(
                "{}{}",
                self.0.file_name().unwrap().to_string_lossy(),
                suffix
            ))
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
