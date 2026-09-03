use jellyfin_model::ImageType;
use jellyfin_providers::manager::{
    item_image_provider::{ImageProvider, ImageRefreshOptions},
    provider_manager::{
        ManagedImageProvider, ManagedMetadataProvider, MetadataProviderKind, MetadataService,
        ProviderItem, ProviderManager, ProviderManagerCapability, ProviderOrderOptions,
        ProviderUpdateType,
    },
};

#[derive(Clone, Copy, Default)]
enum ImageSupport {
    #[default]
    Supported,
    Unsupported,
    Error,
}

#[derive(Default)]
struct FixtureCapability {
    image_support: ImageSupport,
    image_enabled: bool,
    metadata_enabled: bool,
    refreshed_services: Vec<String>,
}

impl FixtureCapability {
    fn enabled() -> Self {
        Self {
            image_support: ImageSupport::Supported,
            image_enabled: true,
            metadata_enabled: true,
            ..Self::default()
        }
    }
}

impl ProviderManagerCapability for FixtureCapability {
    type Error = String;

    fn image_provider_supports(
        &mut self,
        _provider: &ManagedImageProvider,
        _item: &ProviderItem,
    ) -> Result<bool, Self::Error> {
        match self.image_support {
            ImageSupport::Supported => Ok(true),
            ImageSupport::Unsupported => Ok(false),
            ImageSupport::Error => Err("provider failed in Supports(item)".to_owned()),
        }
    }

    fn image_fetcher_enabled(&mut self, _item: &ProviderItem, _provider_name: &str) -> bool {
        self.image_enabled
    }

    fn metadata_fetcher_enabled(&mut self, _item: &ProviderItem, _provider_name: &str) -> bool {
        self.metadata_enabled
    }

    fn refresh_metadata(
        &mut self,
        service: &MetadataService,
        _item: &ProviderItem,
    ) -> ProviderUpdateType {
        self.refreshed_services.push(service.name.clone());
        ProviderUpdateType::MetadataDownload
    }
}

fn service(primary: bool, refresh: bool, order: i32, index: usize) -> MetadataService {
    let mut service = MetadataService::new(format!("Service{index}"), primary, refresh);
    service.order = order;
    service
}

fn manager(
    image_providers: Vec<ManagedImageProvider>,
    metadata_services: Vec<MetadataService>,
    metadata_providers: Vec<ManagedMetadataProvider>,
    library_options: ProviderOrderOptions,
    server_options: ProviderOrderOptions,
) -> ProviderManager {
    let mut manager = ProviderManager::new(library_options, server_options);
    manager.add_parts(image_providers, metadata_services, metadata_providers);
    manager
}

#[test]
fn refresh_single_item_service_ordering_follows_priority() {
    let cases = vec![
        (
            vec![service(true, true, 0, 0), service(true, true, 0, 1)],
            0,
        ),
        (
            vec![
                service(true, true, 1, 0),
                service(true, true, 0, 1),
                service(true, true, 2, 2),
            ],
            1,
        ),
        (
            vec![service(false, true, 0, 0), service(true, true, 0, 1)],
            1,
        ),
        (
            vec![service(false, false, 0, 0), service(false, true, 0, 1)],
            1,
        ),
    ];

    for (services, expected_index) in cases {
        let manager = manager(
            Vec::new(),
            services,
            Vec::new(),
            ProviderOrderOptions::default(),
            ProviderOrderOptions::default(),
        );
        let mut capability = FixtureCapability::enabled();
        let update = manager.refresh_single_item(&ProviderItem::default(), &mut capability);
        assert_eq!(update, ProviderUpdateType::MetadataDownload);
        assert_eq!(
            capability.refreshed_services,
            vec![format!("Service{expected_index}")]
        );
    }
}

#[test]
fn refresh_single_item_refresh_metadata_when_service_found() {
    for found in [true, false] {
        let manager = manager(
            Vec::new(),
            vec![service(false, found, 0, 0)],
            Vec::new(),
            ProviderOrderOptions::default(),
            ProviderOrderOptions::default(),
        );
        let mut capability = FixtureCapability::enabled();
        let update = manager.refresh_single_item(&ProviderItem::default(), &mut capability);
        assert_eq!(
            update,
            if found {
                ProviderUpdateType::MetadataDownload
            } else {
                ProviderUpdateType::None
            }
        );
        assert_eq!(capability.refreshed_services.len(), usize::from(found));
    }
}

struct ImageOrderCase {
    library: Option<Vec<usize>>,
    server: Option<Vec<usize>>,
    default_orders: Option<Vec<Option<i32>>>,
    expected: Vec<usize>,
}

fn provider_names(indices: Option<&[usize]>) -> Option<Vec<String>> {
    indices.map(|indices| {
        indices
            .iter()
            .map(|index| format!("Provider{index}"))
            .collect()
    })
}

fn image_provider(index: usize, order: Option<i32>) -> ManagedImageProvider {
    let mut provider = ManagedImageProvider::new(ImageProvider::local(format!("Provider{index}")));
    provider.order = order;
    provider
}

fn provider_index(name: &str) -> usize {
    name.strip_prefix("Provider").unwrap().parse().unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn get_image_providers_provider_order_matches_expected() {
    let cases = vec![
        ImageOrderCase {
            library: None,
            server: None,
            default_orders: None,
            expected: vec![0, 1, 2],
        },
        ImageOrderCase {
            library: Some(vec![]),
            server: None,
            default_orders: None,
            expected: vec![0, 1, 2],
        },
        ImageOrderCase {
            library: Some(vec![1]),
            server: None,
            default_orders: None,
            expected: vec![1, 0, 2],
        },
        ImageOrderCase {
            library: Some(vec![2, 1, 0]),
            server: None,
            default_orders: None,
            expected: vec![2, 1, 0],
        },
        ImageOrderCase {
            library: None,
            server: Some(vec![]),
            default_orders: None,
            expected: vec![0, 1, 2],
        },
        ImageOrderCase {
            library: None,
            server: Some(vec![1]),
            default_orders: None,
            expected: vec![1, 0, 2],
        },
        ImageOrderCase {
            library: None,
            server: Some(vec![2, 1, 0]),
            default_orders: None,
            expected: vec![2, 1, 0],
        },
        ImageOrderCase {
            library: None,
            server: None,
            default_orders: Some(vec![None, Some(1), None]),
            expected: vec![1, 0, 2],
        },
        ImageOrderCase {
            library: None,
            server: None,
            default_orders: Some(vec![Some(2), Some(1), Some(0)]),
            expected: vec![2, 1, 0],
        },
        ImageOrderCase {
            library: Some(vec![1]),
            server: Some(vec![2, 0, 1]),
            default_orders: None,
            expected: vec![1, 0, 2],
        },
        ImageOrderCase {
            library: Some(vec![1]),
            server: None,
            default_orders: Some(vec![Some(2), Some(0), Some(1)]),
            expected: vec![1, 2, 0],
        },
        ImageOrderCase {
            library: Some(vec![2, 1, 0]),
            server: Some(vec![1, 2, 0]),
            default_orders: Some(vec![Some(2), Some(0), Some(1)]),
            expected: vec![2, 1, 0],
        },
    ];

    for case in cases {
        let orders = case
            .default_orders
            .unwrap_or_else(|| vec![None, None, None]);
        let providers = orders
            .into_iter()
            .enumerate()
            .map(|(index, order)| image_provider(index, order))
            .collect();
        let manager = manager(
            providers,
            Vec::new(),
            Vec::new(),
            ProviderOrderOptions {
                image_fetcher_order: provider_names(case.library.as_deref()),
                ..ProviderOrderOptions::default()
            },
            ProviderOrderOptions {
                image_fetcher_order: provider_names(case.server.as_deref()),
                ..ProviderOrderOptions::default()
            },
        );
        let mut capability = FixtureCapability::enabled();
        let actual = manager
            .get_image_providers(
                &ProviderItem::default(),
                &ImageRefreshOptions::default(),
                &mut capability,
            )
            .iter()
            .map(|provider| provider_index(provider.name()))
            .collect::<Vec<_>>();
        assert_eq!(actual, case.expected);
    }
}

#[derive(Clone, Copy)]
enum ImageProviderKind {
    Basic,
    Local,
    Remote,
    Dynamic,
}

fn managed_image_provider(kind: ImageProviderKind) -> ManagedImageProvider {
    let provider = match kind {
        ImageProviderKind::Basic => ImageProvider::basic("provider"),
        ImageProviderKind::Local => ImageProvider::local("provider"),
        ImageProviderKind::Remote => ImageProvider::remote("provider", vec![ImageType::Primary]),
        ImageProviderKind::Dynamic => ImageProvider::dynamic("provider", vec![ImageType::Primary]),
    };
    ManagedImageProvider::new(provider)
}

fn image_eligibility(
    kind: ImageProviderKind,
    support: ImageSupport,
    item: &ProviderItem,
    refresh_options: &ImageRefreshOptions,
    enabled: bool,
) -> bool {
    let manager = manager(
        vec![managed_image_provider(kind)],
        Vec::new(),
        Vec::new(),
        ProviderOrderOptions::default(),
        ProviderOrderOptions::default(),
    );
    let mut capability = FixtureCapability {
        image_support: support,
        image_enabled: enabled,
        metadata_enabled: true,
        refreshed_services: Vec::new(),
    };
    let providers = manager.get_image_providers(item, refresh_options, &mut capability);
    providers.len() == 1
}

#[test]
fn get_image_providers_can_refresh_images_basic_when_supports_without_error() {
    for (supports, error, expected) in [
        (true, false, true),
        (false, false, false),
        (true, true, false),
    ] {
        let support = if error {
            ImageSupport::Error
        } else if supports {
            ImageSupport::Supported
        } else {
            ImageSupport::Unsupported
        };
        assert_eq!(
            image_eligibility(
                ImageProviderKind::Basic,
                support,
                &ProviderItem::default(),
                &ImageRefreshOptions::default(),
                true
            ),
            expected
        );
    }
}

#[test]
fn get_image_providers_can_refresh_images_locked_when_local_or_full_refresh() {
    for (kind, full_refresh, expected) in [
        (ImageProviderKind::Local, false, true),
        (ImageProviderKind::Local, true, true),
        (ImageProviderKind::Basic, false, false),
        (ImageProviderKind::Basic, true, true),
    ] {
        assert_eq!(
            image_eligibility(
                kind,
                ImageSupport::Supported,
                &ProviderItem {
                    is_locked: true,
                    ..ProviderItem::default()
                },
                &ImageRefreshOptions {
                    full_refresh,
                    ..ImageRefreshOptions::default()
                },
                true
            ),
            expected
        );
    }
}

#[test]
fn get_image_providers_can_refresh_images_base_item_enabled_when_local_or_enabled() {
    for (kind, enabled, expected) in [
        (ImageProviderKind::Local, false, true),
        (ImageProviderKind::Remote, true, true),
        (ImageProviderKind::Dynamic, true, true),
        (ImageProviderKind::Remote, false, false),
        (ImageProviderKind::Dynamic, false, false),
    ] {
        assert_eq!(
            image_eligibility(
                kind,
                ImageSupport::Supported,
                &ProviderItem::default(),
                &ImageRefreshOptions::default(),
                enabled
            ),
            expected
        );
    }
}

struct MetadataOrderCase {
    kinds: Vec<MetadataProviderKind>,
    library_local: Option<Vec<usize>>,
    library_remote: Option<Vec<usize>>,
    server_local: Option<Vec<usize>>,
    server_remote: Option<Vec<usize>>,
    default_orders: Option<Vec<Option<i32>>>,
    expected: Vec<usize>,
}

fn metadata_provider(
    index: usize,
    kind: MetadataProviderKind,
    order: Option<i32>,
) -> ManagedMetadataProvider {
    let mut provider = ManagedMetadataProvider::new(format!("Provider{index}"), kind);
    provider.order = order;
    provider
}

fn lr_kinds(kinds: &str) -> Vec<MetadataProviderKind> {
    kinds
        .chars()
        .map(|kind| match kind {
            'l' => MetadataProviderKind::Local,
            'r' => MetadataProviderKind::Remote,
            _ => unreachable!(),
        })
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn get_metadata_providers_provider_order_matches_expected() {
    let cases = vec![
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: None,
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![0, 1, 2, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: Some(vec![]),
            library_remote: Some(vec![]),
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![0, 1, 2, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("rlll"),
            library_local: Some(vec![2]),
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![2, 0, 1, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("rlll"),
            library_local: Some(vec![3, 2, 1]),
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![3, 2, 1, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lrrr"),
            library_local: None,
            library_remote: Some(vec![2]),
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![2, 0, 1, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lrrr"),
            library_local: None,
            library_remote: Some(vec![3, 2, 1]),
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![3, 2, 1, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: Some(vec![1]),
            library_remote: Some(vec![3]),
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![1, 3, 0, 2],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lllrrr"),
            library_local: Some(vec![2, 1, 0]),
            library_remote: Some(vec![5, 4, 3]),
            server_local: None,
            server_remote: None,
            default_orders: None,
            expected: vec![2, 5, 1, 4, 0, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: None,
            library_remote: None,
            server_local: Some(vec![]),
            server_remote: Some(vec![]),
            default_orders: None,
            expected: vec![0, 1, 2, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("rlll"),
            library_local: None,
            library_remote: None,
            server_local: Some(vec![2]),
            server_remote: None,
            default_orders: None,
            expected: vec![2, 0, 1, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("rlll"),
            library_local: None,
            library_remote: None,
            server_local: Some(vec![3, 2, 1]),
            server_remote: None,
            default_orders: None,
            expected: vec![3, 2, 1, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lrrr"),
            library_local: None,
            library_remote: None,
            server_local: None,
            server_remote: Some(vec![2]),
            default_orders: None,
            expected: vec![2, 0, 1, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lrrr"),
            library_local: None,
            library_remote: None,
            server_local: None,
            server_remote: Some(vec![3, 2, 1]),
            default_orders: None,
            expected: vec![3, 2, 1, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: None,
            library_remote: None,
            server_local: Some(vec![1]),
            server_remote: Some(vec![3]),
            default_orders: None,
            expected: vec![1, 3, 0, 2],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lllrrr"),
            library_local: None,
            library_remote: None,
            server_local: Some(vec![2, 1, 0]),
            server_remote: Some(vec![5, 4, 3]),
            default_orders: None,
            expected: vec![2, 5, 1, 4, 0, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: None,
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: Some(vec![Some(2), None, Some(1), None]),
            expected: vec![2, 0, 1, 3],
        },
        MetadataOrderCase {
            kinds: lr_kinds("llrr"),
            library_local: None,
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: Some(vec![Some(3), Some(2), Some(1), Some(0)]),
            expected: vec![3, 2, 1, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lllrrr"),
            library_local: Some(vec![1]),
            library_remote: Some(vec![4]),
            server_local: Some(vec![2, 1, 0]),
            server_remote: Some(vec![5, 4, 3]),
            default_orders: None,
            expected: vec![1, 4, 0, 2, 3, 5],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lll"),
            library_local: Some(vec![1]),
            library_remote: None,
            server_local: None,
            server_remote: None,
            default_orders: Some(vec![Some(2), Some(0), Some(1)]),
            expected: vec![1, 2, 0],
        },
        MetadataOrderCase {
            kinds: lr_kinds("lllrrr"),
            library_local: Some(vec![2, 1, 0]),
            library_remote: Some(vec![5, 4, 3]),
            server_local: Some(vec![1, 2, 0]),
            server_remote: Some(vec![4, 5, 3]),
            default_orders: Some(vec![Some(5), Some(4), Some(1), Some(6), Some(3), Some(2)]),
            expected: vec![2, 5, 4, 1, 0, 3],
        },
    ];

    for case in cases {
        let orders = case
            .default_orders
            .unwrap_or_else(|| vec![None; case.kinds.len()]);
        let providers = case
            .kinds
            .into_iter()
            .zip(orders)
            .enumerate()
            .map(|(index, (kind, order))| metadata_provider(index, kind, order))
            .collect();
        let manager = manager(
            Vec::new(),
            Vec::new(),
            providers,
            ProviderOrderOptions {
                local_metadata_reader_order: provider_names(case.library_local.as_deref()),
                metadata_fetcher_order: provider_names(case.library_remote.as_deref()),
                ..ProviderOrderOptions::default()
            },
            ProviderOrderOptions {
                local_metadata_reader_order: provider_names(case.server_local.as_deref()),
                metadata_fetcher_order: provider_names(case.server_remote.as_deref()),
                ..ProviderOrderOptions::default()
            },
        );
        let mut capability = FixtureCapability::enabled();
        let actual = manager
            .get_metadata_providers(&ProviderItem::default(), &mut capability)
            .iter()
            .map(|provider| provider_index(&provider.name))
            .collect::<Vec<_>>();
        assert_eq!(actual, case.expected);
    }
}

fn metadata_eligibility(
    kind: MetadataProviderKind,
    item: &ProviderItem,
    enabled: bool,
    attach_forced_marker: bool,
) -> bool {
    let mut provider = ManagedMetadataProvider::new("provider", kind);
    provider.forced = attach_forced_marker;
    let manager = manager(
        Vec::new(),
        Vec::new(),
        vec![provider],
        ProviderOrderOptions::default(),
        ProviderOrderOptions::default(),
    );
    let mut capability = FixtureCapability {
        metadata_enabled: enabled,
        ..FixtureCapability::enabled()
    };
    manager.get_metadata_providers(item, &mut capability).len() == 1
}

const fn official_forced_marker_attached(_requested: bool) -> bool {
    false
}

#[test]
fn get_metadata_providers_can_refresh_metadata_basic_returns_true() {
    for kind in [
        MetadataProviderKind::Basic,
        MetadataProviderKind::Local,
        MetadataProviderKind::Remote,
        MetadataProviderKind::Custom,
    ] {
        assert!(metadata_eligibility(
            kind,
            &ProviderItem::default(),
            true,
            false
        ));
    }
}

#[test]
fn get_metadata_providers_can_refresh_metadata_locked_when_local_or_forced() {
    for (kind, requested_forced, expected) in [
        (MetadataProviderKind::Local, false, true),
        (MetadataProviderKind::Remote, false, false),
        (MetadataProviderKind::Custom, false, false),
        (MetadataProviderKind::Local, true, true),
        (MetadataProviderKind::Custom, true, false),
    ] {
        // The official Moq helper allocates the forced mock but does not attach
        // it to the returned provider unless an order interface is also used.
        let forced_marker_attached = official_forced_marker_attached(requested_forced);
        assert_eq!(
            metadata_eligibility(
                kind,
                &ProviderItem {
                    is_locked: true,
                    ..ProviderItem::default()
                },
                true,
                forced_marker_attached
            ),
            expected
        );
    }
}

#[test]
fn get_metadata_providers_can_refresh_metadata_base_item_enabled_when_enabled_or_not_remote() {
    for (kind, enabled, expected) in [
        (MetadataProviderKind::Local, false, true),
        (MetadataProviderKind::Custom, false, true),
        (MetadataProviderKind::Remote, false, false),
        (MetadataProviderKind::Remote, true, true),
    ] {
        assert_eq!(
            metadata_eligibility(kind, &ProviderItem::default(), enabled, false),
            expected
        );
    }
}

#[test]
fn get_metadata_providers_can_refresh_metadata_supports_local_when_supports_or_not_local() {
    for (kind, supports_local, expected) in [
        (MetadataProviderKind::Remote, false, true),
        (MetadataProviderKind::Custom, false, true),
        (MetadataProviderKind::Local, false, false),
        (MetadataProviderKind::Local, true, true),
    ] {
        assert_eq!(
            metadata_eligibility(
                kind,
                &ProviderItem {
                    supports_local_metadata: supports_local,
                    ..ProviderItem::default()
                },
                true,
                false
            ),
            expected
        );
    }
}

#[test]
fn get_metadata_providers_can_refresh_metadata_owned() {
    for kind in [
        MetadataProviderKind::Custom,
        MetadataProviderKind::Remote,
        MetadataProviderKind::Local,
    ] {
        assert!(metadata_eligibility(
            kind,
            &ProviderItem {
                is_owned: true,
                ..ProviderItem::default()
            },
            true,
            false
        ));
    }
}
