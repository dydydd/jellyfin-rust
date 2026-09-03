use std::collections::HashMap;

use jellyfin_common::ProviderIdParsers;

/// Provider identifiers resolved directly from a media path or file name.
pub type ProviderIdMap = HashMap<String, String>;

pub(crate) fn from_path(path: &str, providers: &[(&str, &str)]) -> ProviderIdMap {
    providers
        .iter()
        .filter_map(|(name, attribute)| {
            ProviderIdParsers::get_attribute_value(path, attribute)
                .ok()
                .flatten()
                .map(|value| ((*name).to_owned(), value.to_owned()))
        })
        .collect()
}
