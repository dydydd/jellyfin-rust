use jellyfin_model::MediaStreamType;
use jellyfin_naming::{DlnaProfileType, LocalizationManager, NamingOptions};

use super::resolver::{
    ExternalMediaInfoCapability, ExternalStreamResolver, MediaResolveRequest,
    ResolvedExternalStream,
};

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

    /// Resolves candidates and merges stream details supplied by a media-info capability.
    #[must_use]
    pub fn resolve_with_media_info<C: ExternalMediaInfoCapability + ?Sized>(
        &self,
        request: SubtitleResolveRequest<'_>,
        capability: &C,
    ) -> Vec<ResolvedSubtitleStream> {
        self.resolver.resolve_with_media_info(request, capability)
    }
}
