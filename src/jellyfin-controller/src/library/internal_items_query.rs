use thiserror::Error;

/// A predefined filter accepted by [`InternalItemsQuery::apply_filters`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ItemFilter {
    IsFolder = 1,
    IsNotFolder = 2,
    IsUnplayed = 3,
    IsPlayed = 4,
    IsFavorite = 5,
    IsResumable = 7,
    Likes = 8,
    Dislikes = 9,
    IsFavoriteOrLikes = 10,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InternalItemsQueryError {
    #[error("conflicting filters: {first:?} and {second:?}")]
    ConflictingFilters {
        first: ItemFilter,
        second: ItemFilter,
    },
}

/// Internal item criteria assembled by controller and repository workflows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InternalItemsQuery {
    pub is_folder: Option<bool>,
    pub is_favorite: Option<bool>,
    pub is_favorite_or_liked: Option<bool>,
    pub is_liked: Option<bool>,
    pub is_played: Option<bool>,
    pub is_resumable: Option<bool>,
}

impl InternalItemsQuery {
    /// Returns whether this query contains any item-selection criteria.
    #[must_use]
    pub const fn has_filters(&self) -> bool {
        self.is_folder.is_some()
            || self.is_favorite.is_some()
            || self.is_favorite_or_liked.is_some()
            || self.is_liked.is_some()
            || self.is_played.is_some()
            || self.is_resumable.is_some()
    }

    /// Applies Jellyfin's predefined item filters to this query.
    ///
    /// # Errors
    ///
    /// Returns [`InternalItemsQueryError::ConflictingFilters`] when both sides
    /// of a mutually exclusive filter pair are present.
    pub fn apply_filters(&mut self, filters: &[ItemFilter]) -> Result<(), InternalItemsQueryError> {
        for &filter in filters {
            match filter {
                ItemFilter::IsFolder => {
                    reject_conflict(filters, ItemFilter::IsNotFolder)?;
                    self.is_folder = Some(true);
                }
                ItemFilter::IsNotFolder => {
                    reject_conflict(filters, ItemFilter::IsFolder)?;
                    self.is_folder = Some(false);
                }
                ItemFilter::IsUnplayed => {
                    reject_conflict(filters, ItemFilter::IsPlayed)?;
                    self.is_played = Some(false);
                }
                ItemFilter::IsPlayed => {
                    reject_conflict(filters, ItemFilter::IsUnplayed)?;
                    self.is_played = Some(true);
                }
                ItemFilter::IsFavorite => self.is_favorite = Some(true),
                ItemFilter::IsResumable => self.is_resumable = Some(true),
                ItemFilter::Likes => {
                    reject_conflict(filters, ItemFilter::Dislikes)?;
                    self.is_liked = Some(true);
                }
                ItemFilter::Dislikes => {
                    reject_conflict(filters, ItemFilter::Likes)?;
                    self.is_liked = Some(false);
                }
                ItemFilter::IsFavoriteOrLikes => self.is_favorite_or_liked = Some(true),
            }
        }

        Ok(())
    }
}

fn reject_conflict(
    filters: &[ItemFilter],
    conflicting_filter: ItemFilter,
) -> Result<(), InternalItemsQueryError> {
    if !filters.contains(&conflicting_filter) {
        return Ok(());
    }

    let (first, second) = match conflicting_filter {
        ItemFilter::IsFolder | ItemFilter::IsNotFolder => {
            (ItemFilter::IsFolder, ItemFilter::IsNotFolder)
        }
        ItemFilter::IsPlayed | ItemFilter::IsUnplayed => {
            (ItemFilter::IsPlayed, ItemFilter::IsUnplayed)
        }
        ItemFilter::Likes | ItemFilter::Dislikes => (ItemFilter::Likes, ItemFilter::Dislikes),
        ItemFilter::IsFavorite | ItemFilter::IsResumable | ItemFilter::IsFavoriteOrLikes => {
            unreachable!("these filters have no mutually exclusive counterpart")
        }
    };

    Err(InternalItemsQueryError::ConflictingFilters { first, second })
}
