use jellyfin_model::MediaStreamType;
use jellyfin_naming::{DlnaProfileType, LocalizationManager, NamingOptions};

use super::resolver::{ExternalStreamResolver, MediaResolveRequest, ResolvedExternalStream};

pub type SubtitleResolveRequest<'a> = MediaResolveRequest<'a>;
pub type ResolvedSubtitleStream = ResolvedExternalStream;

/// Resolves external subtitle files associated with a local video file.
pub struct SubtitleResolver<'a, L: LocalizationManager + ?Sized> {
    resolver: ExternalStreamResolver<'a, L>,
}

impl<'a, L: LocalizationManager + ?Sized> SubtitleResolver<'a, L> {
    pub fn new(naming_options: NamingOptions, localization_manager: &'a L) -> Self {
        Self {
            resolver: ExternalStreamResolver::new(
                naming_options,
                localization_manager,
                DlnaProfileType::Subtitle,
                MediaStreamType::Subtitle,
            ),
        }
    }

    /// Resolves external subtitle candidates without probing their contents.
    #[must_use]
    pub fn resolve(&self, request: SubtitleResolveRequest<'_>) -> Vec<ResolvedSubtitleStream> {
        self.resolver.resolve(request)
    }
}
