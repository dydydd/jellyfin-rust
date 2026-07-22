//! Common, dependency-free utilities shared by Jellyfin services.

pub mod crc32;
pub mod path;
pub mod providers;

pub use crc32::Crc32;
pub use path::{
    InvalidPathSeparator, normalize_path, normalize_path_default,
    normalize_path_with_detected_separator, try_replace_sub_path,
};
pub use providers::{
    AttributeValueError, AttributeValueInput, ProviderIdParsers, get_attribute_value,
};
