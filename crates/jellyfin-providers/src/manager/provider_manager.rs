use std::fmt::Display;

use super::item_image_provider::{ImageProvider, ImageRefreshOptions};

const DEFAULT_PROVIDER_ORDER: i32 = 50;

/// Minimal item state used when selecting metadata and image providers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderItem {
    pub type_name: String,
    pub is_locked: bool,
    pub supports_local_metadata: bool,
    pub is_owned: bool,
}

impl Default for ProviderItem {
    fn default() -> Self {
        Self {
            type_name: "Video".to_owned(),
            is_locked: false,
            supports_local_metadata: true,
            is_owned: false,
        }
    }
}

/// One image provider plus its optional `IHasOrder` value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedImageProvider {
    pub provider: ImageProvider,
    pub order: Option<i32>,
}

impl ManagedImageProvider {
    #[must_use]
    pub const fn new(provider: ImageProvider) -> Self {
        Self {
            provider,
            order: None,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match &self.provider {
            ImageProvider::Basic { name }
            | ImageProvider::Local { name }
            | ImageProvider::Dynamic { name, .. }
            | ImageProvider::Remote { name, .. } => name,
        }
    }

    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self.provider, ImageProvider::Local { .. })
    }
}

/// Metadata provider interface category used by `ProviderManager` filtering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataProviderKind {
    Basic,
    Local,
    Remote,
    Custom,
}

/// One metadata provider plus its optional provider marker interfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedMetadataProvider {
    pub name: String,
    pub kind: MetadataProviderKind,
    pub order: Option<i32>,
    pub forced: bool,
}

impl ManagedMetadataProvider {
    #[must_use]
    pub fn new(name: impl Into<String>, kind: MetadataProviderKind) -> Self {
        Self {
            name: name.into(),
            kind,
            order: None,
            forced: false,
        }
    }
}

/// A metadata service considered by `RefreshSingleItem`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetadataService {
    pub name: String,
    pub order: i32,
    pub can_refresh_primary: bool,
    pub can_refresh: bool,
}

impl MetadataService {
    #[must_use]
    pub fn new(name: impl Into<String>, can_refresh_primary: bool, can_refresh: bool) -> Self {
        Self {
            name: name.into(),
            order: 0,
            can_refresh_primary,
            can_refresh,
        }
    }
}

/// Configured provider order and optional enabled remote metadata providers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderOrderOptions {
    pub image_fetcher_order: Option<Vec<String>>,
    pub local_metadata_reader_order: Option<Vec<String>>,
    pub metadata_fetcher_order: Option<Vec<String>>,
    pub metadata_fetchers: Option<Vec<String>>,
}

/// Update flag returned by the selected metadata service.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProviderUpdateType {
    #[default]
    None,
    MetadataDownload,
}

/// Fixture boundary for provider predicates and metadata refresh execution.
pub trait ProviderManagerCapability {
    type Error: Display;

    /// Evaluates an image provider's `Supports(item)` implementation.
    ///
    /// # Errors
    ///
    /// Returns a provider-specific error when its support predicate throws.
    fn image_provider_supports(
        &mut self,
        _provider: &ManagedImageProvider,
        _item: &ProviderItem,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn image_fetcher_enabled(&mut self, _item: &ProviderItem, _provider_name: &str) -> bool {
        true
    }

    fn metadata_fetcher_enabled(&mut self, _item: &ProviderItem, _provider_name: &str) -> bool {
        true
    }

    fn refresh_metadata(
        &mut self,
        _service: &MetadataService,
        _item: &ProviderItem,
    ) -> ProviderUpdateType {
        ProviderUpdateType::MetadataDownload
    }
}

/// Provider registry, ordering, and per-item eligibility logic.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProviderManager {
    image_providers: Vec<ManagedImageProvider>,
    metadata_services: Vec<MetadataService>,
    metadata_providers: Vec<ManagedMetadataProvider>,
    library_options: ProviderOrderOptions,
    server_options: ProviderOrderOptions,
}

impl ProviderManager {
    #[must_use]
    pub const fn new(
        library_options: ProviderOrderOptions,
        server_options: ProviderOrderOptions,
    ) -> Self {
        Self {
            image_providers: Vec::new(),
            metadata_services: Vec::new(),
            metadata_providers: Vec::new(),
            library_options,
            server_options,
        }
    }

    pub fn add_parts(
        &mut self,
        image_providers: Vec<ManagedImageProvider>,
        mut metadata_services: Vec<MetadataService>,
        metadata_providers: Vec<ManagedMetadataProvider>,
    ) {
        metadata_services.sort_by_key(|service| service.order);
        self.image_providers = image_providers;
        self.metadata_services = metadata_services;
        self.metadata_providers = metadata_providers;
    }

    pub fn refresh_single_item<C: ProviderManagerCapability + ?Sized>(
        &self,
        item: &ProviderItem,
        capability: &mut C,
    ) -> ProviderUpdateType {
        self.metadata_services
            .iter()
            .find(|service| service.can_refresh_primary)
            .or_else(|| {
                self.metadata_services
                    .iter()
                    .find(|service| service.can_refresh)
            })
            .map_or(ProviderUpdateType::None, |service| {
                capability.refresh_metadata(service, item)
            })
    }

    pub fn get_image_providers<'a, C: ProviderManagerCapability + ?Sized>(
        &'a self,
        item: &ProviderItem,
        refresh_options: &ImageRefreshOptions,
        capability: &mut C,
    ) -> Vec<&'a ManagedImageProvider> {
        let configured_order = self
            .library_options
            .image_fetcher_order
            .as_deref()
            .or(self.server_options.image_fetcher_order.as_deref())
            .unwrap_or_default();
        let mut providers = self
            .image_providers
            .iter()
            .enumerate()
            .filter(|(_, provider)| can_refresh_images(provider, item, refresh_options, capability))
            .collect::<Vec<_>>();
        providers.sort_by_key(|(index, provider)| {
            (
                configured_order_value(configured_order, provider.name()),
                provider.order.unwrap_or(DEFAULT_PROVIDER_ORDER),
                *index,
            )
        });
        providers
            .into_iter()
            .map(|(_, provider)| provider)
            .collect()
    }

    pub fn get_metadata_providers<'a, C: ProviderManagerCapability + ?Sized>(
        &'a self,
        item: &ProviderItem,
        capability: &mut C,
    ) -> Vec<&'a ManagedMetadataProvider> {
        let local_order = self
            .library_options
            .local_metadata_reader_order
            .as_deref()
            .or(self.server_options.local_metadata_reader_order.as_deref())
            .unwrap_or_default();
        let remote_order = self
            .library_options
            .metadata_fetcher_order
            .as_deref()
            .or(self.server_options.metadata_fetcher_order.as_deref())
            .unwrap_or_default();
        let mut providers = self
            .metadata_providers
            .iter()
            .enumerate()
            .filter(|(_, provider)| self.can_cache_metadata_provider(provider))
            .collect::<Vec<_>>();
        providers.sort_by_key(|(index, provider)| {
            let configured = match provider.kind {
                MetadataProviderKind::Local => configured_order_value(local_order, &provider.name),
                MetadataProviderKind::Remote => {
                    configured_order_value(remote_order, &provider.name)
                }
                MetadataProviderKind::Basic | MetadataProviderKind::Custom => usize::MAX,
            };
            (
                configured,
                provider.order.unwrap_or(DEFAULT_PROVIDER_ORDER),
                *index,
            )
        });
        providers
            .into_iter()
            .filter(|(_, provider)| can_refresh_metadata(provider, item, capability))
            .map(|(_, provider)| provider)
            .collect()
    }

    fn can_cache_metadata_provider(&self, provider: &ManagedMetadataProvider) -> bool {
        if provider.kind != MetadataProviderKind::Remote {
            return true;
        }
        self.library_options
            .metadata_fetchers
            .as_ref()
            .filter(|fetchers| !fetchers.is_empty())
            .is_none_or(|fetchers| {
                fetchers
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&provider.name))
            })
    }
}

fn can_refresh_images<C: ProviderManagerCapability + ?Sized>(
    provider: &ManagedImageProvider,
    item: &ProviderItem,
    refresh_options: &ImageRefreshOptions,
    capability: &mut C,
) -> bool {
    if !matches!(capability.image_provider_supports(provider, item), Ok(true)) {
        return false;
    }
    provider.is_local()
        || ((!item.is_locked || refresh_options.full_refresh)
            && capability.image_fetcher_enabled(item, provider.name()))
}

fn can_refresh_metadata<C: ProviderManagerCapability + ?Sized>(
    provider: &ManagedMetadataProvider,
    item: &ProviderItem,
    capability: &mut C,
) -> bool {
    if !item.supports_local_metadata && provider.kind == MetadataProviderKind::Local {
        return false;
    }
    if item.is_locked && provider.kind != MetadataProviderKind::Local && !provider.forced {
        return false;
    }
    provider.kind != MetadataProviderKind::Remote
        || capability.metadata_fetcher_enabled(item, &provider.name)
}

fn configured_order_value(order: &[String], provider_name: &str) -> usize {
    order
        .iter()
        .position(|name| name == provider_name)
        .unwrap_or(usize::MAX)
}
