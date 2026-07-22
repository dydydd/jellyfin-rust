use jellyfin_model::ProviderIdMap;

/// Item kinds whose runtime type changes an external provider URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalUrlItemKind {
    Audio,
    Book,
    BoxSet,
    Episode,
    Movie,
    MusicAlbum,
    MusicArtist,
    Person,
    Season,
    Series,
    Other,
}

/// Provider-facing item projection with already-resolved series context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalUrlItem {
    pub kind: ExternalUrlItemKind,
    pub provider_ids: ProviderIdMap,
    pub index_number: Option<i32>,
    pub series_provider_ids: ProviderIdMap,
    pub season_index_number: Option<i32>,
    pub series_display_order: Option<String>,
}

impl ExternalUrlItem {
    #[must_use]
    pub fn new(kind: ExternalUrlItemKind) -> Self {
        Self {
            kind,
            provider_ids: ProviderIdMap::new(),
            index_number: None,
            series_provider_ids: ProviderIdMap::new(),
            season_index_number: None,
            series_display_order: None,
        }
    }

    #[must_use]
    pub fn with_provider_id(
        mut self,
        provider: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.provider_ids.insert(provider.into(), value.into());
        self
    }

    #[must_use]
    pub fn with_series_provider_id(
        mut self,
        provider: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.series_provider_ids
            .insert(provider.into(), value.into());
        self
    }

    #[must_use]
    pub const fn with_index_number(mut self, index_number: i32) -> Self {
        self.index_number = Some(index_number);
        self
    }

    #[must_use]
    pub const fn with_season_index_number(mut self, index_number: i32) -> Self {
        self.season_index_number = Some(index_number);
        self
    }

    #[must_use]
    pub fn with_series_display_order(mut self, display_order: impl Into<String>) -> Self {
        self.series_display_order = Some(display_order.into());
        self
    }
}
