//! Cross-platform string and filesystem path helpers.

pub mod file;
pub mod json;
pub mod path;
pub mod string;

pub use file::{FileHelper, create_empty};
pub use path::{PathHelper, get_safe_leaf_file_name, is_contained_in};
pub use string::{StringExtensions, has_diacritics, remove_diacritics};
