use jellyfin_model::MediaStreamType;
use jellyfin_naming::{DlnaProfileType, LocalizationManager, NamingOptions};

use super::resolver::{ExternalStreamResolver, MediaResolveRequest, ResolvedExternalStream};

pub type AudioResolveRequest<'a> = MediaResolveRequest<'a>;
pub type ResolvedAudioStream = ResolvedExternalStream;

/// Resolves external audio files associated with a local video file.
pub struct AudioResolver<'a, L: LocalizationManager + ?Sized> {
    resolver: ExternalStreamResolver<'a, L>,
}

impl<'a, L: LocalizationManager + ?Sized> AudioResolver<'a, L> {
    pub fn new(naming_options: NamingOptions, localization_manager: &'a L) -> Self {
        Self {
            resolver: ExternalStreamResolver::new(
                naming_options,
                localization_manager,
                DlnaProfileType::Audio,
                MediaStreamType::Audio,
            ),
        }
    }

    /// Resolves external audio candidates without probing their contents.
    #[must_use]
    pub fn resolve(&self, request: AudioResolveRequest<'_>) -> Vec<ResolvedAudioStream> {
        self.resolver.resolve(request)
    }
}
