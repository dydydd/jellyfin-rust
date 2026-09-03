use std::{error::Error, fmt};

/// Failure returned when a complete slice cannot be copied at the requested index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyToError {
    /// A destination index cannot be represented by a Rust slice index.
    NegativeIndex { index: isize },
    /// The destination has fewer remaining elements than the source requires.
    InsufficientDestinationSpace {
        index: usize,
        source_len: usize,
        destination_len: usize,
    },
}

impl fmt::Display for CopyToError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeIndex { index } => {
                write!(formatter, "destination index {index} cannot be negative")
            }
            Self::InsufficientDestinationSpace {
                index,
                source_len,
                destination_len,
            } => write!(
                formatter,
                "source length {source_len} does not fit at index {index} in destination length {destination_len}"
            ),
        }
    }
}

impl Error for CopyToError {}

/// Copies every source element into `destination`, starting at `index`.
///
/// The destination is not modified unless the complete source fits. Elements
/// before and after the copied range retain their previous values.
///
/// # Errors
///
/// Returns [`CopyToError::NegativeIndex`] for a negative index, or
/// [`CopyToError::InsufficientDestinationSpace`] when the source does not fit.
pub fn copy_to<T: Clone>(
    source: &[T],
    destination: &mut [T],
    index: isize,
) -> Result<(), CopyToError> {
    let index = usize::try_from(index).map_err(|_| CopyToError::NegativeIndex { index })?;
    if source.len() > destination.len().saturating_sub(index) || index > destination.len() {
        return Err(CopyToError::InsufficientDestinationSpace {
            index,
            source_len: source.len(),
            destination_len: destination.len(),
        });
    }

    destination[index..index + source.len()].clone_from_slice(source);
    Ok(())
}

/// Jellyfin-style `CopyTo` operation for slices and slice-backed collections.
pub trait CopyToExtensions<T> {
    /// Copies every element into `destination`, starting at `index`.
    ///
    /// # Errors
    ///
    /// Returns a typed bounds error without modifying `destination` when the
    /// complete source cannot be copied.
    fn copy_to(&self, destination: &mut [T], index: isize) -> Result<(), CopyToError>
    where
        T: Clone;
}

impl<T> CopyToExtensions<T> for [T] {
    fn copy_to(&self, destination: &mut [T], index: isize) -> Result<(), CopyToError>
    where
        T: Clone,
    {
        copy_to(self, destination, index)
    }
}
