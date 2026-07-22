use std::sync::Arc;

use jellyfin_model::PluginInfo;

/// In-memory metadata for plugins discovered by the server runtime.
#[derive(Clone, Default)]
pub struct PluginRegistry {
    plugins: Arc<[PluginInfo]>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new(plugins: Vec<PluginInfo>) -> Self {
        Self {
            plugins: plugins.into(),
        }
    }

    /// Returns an owned, name-ordered snapshot of installed plugins.
    #[must_use]
    pub fn plugins(&self) -> Vec<PluginInfo> {
        let mut plugins = self.plugins.to_vec();
        plugins.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.version.cmp(&right.version))
        });
        plugins
    }
}

#[cfg(test)]
mod tests {
    use jellyfin_model::PluginStatus;
    use uuid::Uuid;

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
}
