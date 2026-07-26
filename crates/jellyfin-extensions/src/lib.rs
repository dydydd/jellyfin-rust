//! Cross-platform string and filesystem path helpers.

pub mod copy_to;
pub mod file;
pub mod formatting_stream_writer;
pub mod json;
pub mod path;
pub mod shuffle;
pub mod stream;
pub mod string;

pub use copy_to::{CopyToError, CopyToExtensions, copy_to};
pub use file::{FileHelper, create_empty};
pub use formatting_stream_writer::{FormattingStreamWriter, escape_concat_file_path};
pub use path::{PathHelper, get_safe_leaf_file_name, is_contained_in};
pub use shuffle::{shuffle, shuffle_with};
pub use stream::{ComparableStream, StreamCompareError, is_file_identical, is_stream_identical};
pub use string::{StringExtensions, has_diacritics, remove_diacritics};
