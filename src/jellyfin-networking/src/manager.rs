use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::Arc;

use crate::{
    IpData, IpNetwork, NetworkConfiguration, ParsedHost, format_ip_string, is_ipv6_link_local,
    subnet_contains_address, try_parse_host, try_parse_to_subnet, try_parse_to_subnets,
};

/// Result of applying Jellyfin's remote access policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteAccessPolicyResult {
    Allow,
    RejectDueToRemoteAccessDisabled,
    RejectDueToIpBlocklist,
    RejectDueToNotAllowlistedRemoteIp,
}

/// A usable address discovered on a network interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkInterface {
    pub data: IpData,
    pub index: i32,
    pub name: String,
    pub supports_multicast: bool,
}

impl NetworkInterface {
    #[must_use]
    pub fn new(data: IpData, index: i32, name: impl Into<String>) -> Self {
        Self {
            data,
            index,
            name: name.into(),
            supports_multicast: false,
        }
    }

    #[must_use]
    pub const fn address(&self) -> IpAddr {
        self.data.address
    }

    #[must_use]
    pub const fn subnet(&self) -> IpNetwork {
        self.data.subnet
    }
}

/// Resolves names for bind-address selection. Tests can inject a deterministic
/// implementation and avoid DNS or other network I/O.
pub trait HostResolver: Send + Sync {
    fn resolve(&self, host: &str) -> Vec<IpAddr>;
}

/// Resolver backed by the operating system's normal host lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHostResolver;

impl HostResolver for SystemHostResolver {
    fn resolve(&self, host: &str) -> Vec<IpAddr> {
        (host, 0)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkManagerError {
    InvalidInterfaceFixture(String),
    InvalidInterfaceIndex(String),
}

impl fmt::Display for NetworkManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInterfaceFixture(value) => {
                write!(formatter, "invalid network interface fixture: {value}")
            }
            Self::InvalidInterfaceIndex(value) => {
                write!(formatter, "invalid network interface index: {value}")
            }
        }
    }
}

impl std::error::Error for NetworkManagerError {}

#[derive(Clone, Debug)]
struct PublishedServerUriOverride {
    data: IpData,
    uri: String,
    internal: bool,
    external: bool,
}

/// Deterministic network policy and bind-address manager.
pub struct NetworkManager {
    config: NetworkConfiguration,
    interfaces: Vec<NetworkInterface>,
    lan_subnets: Vec<IpNetwork>,
    excluded_subnets: Vec<IpNetwork>,
    remote_address_filter: Vec<IpNetwork>,
    published_server_urls: Vec<PublishedServerUriOverride>,
    resolver: Arc<dyn HostResolver>,
}

impl NetworkManager {
    #[must_use]
    pub fn new(config: NetworkConfiguration, interfaces: Vec<NetworkInterface>) -> Self {
        Self::with_resolver(config, interfaces, SystemHostResolver)
    }

    #[must_use]
    pub fn with_resolver<R>(
        config: NetworkConfiguration,
        interfaces: Vec<NetworkInterface>,
        resolver: R,
    ) -> Self
    where
        R: HostResolver + 'static,
    {
        let mut manager = Self {
            config,
            interfaces,
            lan_subnets: Vec::new(),
            excluded_subnets: Vec::new(),
            remote_address_filter: Vec::new(),
            published_server_urls: Vec::new(),
            resolver: Arc::new(resolver),
        };
        manager.initialize();
        manager
    }

    /// Constructs a manager from Jellyfin's test fixture format:
    /// `address/prefix,index,name|address/prefix,index,name`.
    pub fn from_fixture<R>(
        config: NetworkConfiguration,
        fixture: &str,
        resolver: R,
    ) -> Result<Self, NetworkManagerError>
    where
        R: HostResolver + 'static,
    {
        let interfaces = parse_interface_fixture(fixture)?;
        Ok(Self::with_resolver(config, interfaces, resolver))
    }

    #[must_use]
    pub const fn config(&self) -> &NetworkConfiguration {
        &self.config
    }

    #[must_use]
    pub fn interfaces(&self) -> &[NetworkInterface] {
        &self.interfaces
    }

    #[must_use]
    pub fn resolve_host(&self, host: &str) -> Vec<IpAddr> {
        match try_parse_host(host, self.config.enable_ipv4, self.config.enable_ipv6) {
            Some(ParsedHost::Address(address)) => vec![address],
            Some(ParsedHost::Name(name)) => self
                .resolver
                .resolve(&name)
                .into_iter()
                .filter(|address| self.family_enabled(*address))
                .collect(),
            None => Vec::new(),
        }
    }

    #[must_use]
    pub fn try_parse_interface(&self, name: &str) -> Vec<IpData> {
        let mut interfaces = self
            .interfaces
            .iter()
            .filter(|interface| {
                interface.name.eq_ignore_ascii_case(name)
                    && self.family_enabled(interface.address())
            })
            .collect::<Vec<_>>();
        interfaces.sort_by_key(|interface| interface.index);
        interfaces
            .into_iter()
            .map(|interface| interface.data)
            .collect()
    }

    #[must_use]
    pub fn get_loopbacks(&self) -> Vec<IpData> {
        let mut loopbacks = Vec::new();
        if self.config.enable_ipv4 {
            loopbacks.push(IpData {
                address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                subnet: network("127.0.0.0/8"),
            });
        }
        if self.config.enable_ipv6 {
            loopbacks.push(IpData {
                address: IpAddr::V6(Ipv6Addr::LOCALHOST),
                subnet: network("::1/128"),
            });
        }
        loopbacks
    }

    #[must_use]
    pub fn get_internal_bind_addresses(&self) -> Vec<IpData> {
        let mut interfaces = self
            .interfaces
            .iter()
            .filter(|interface| self.is_in_local_network(interface.address()))
            .collect::<Vec<_>>();
        interfaces.sort_by_key(|interface| interface.index);
        interfaces
            .into_iter()
            .map(|interface| interface.data)
            .collect()
    }

    #[must_use]
    pub fn is_in_local_network(&self, address: IpAddr) -> bool {
        let address = map_ipv4_mapped(address);
        address.is_loopback()
            || (self
                .lan_subnets
                .iter()
                .any(|network| subnet_contains_address(*network, address))
                && !self
                    .excluded_subnets
                    .iter()
                    .any(|network| subnet_contains_address(*network, address)))
    }

    #[must_use]
    pub fn should_allow_server_access(&self, remote_ip: IpAddr) -> RemoteAccessPolicyResult {
        if self.is_in_local_network(remote_ip) {
            return RemoteAccessPolicyResult::Allow;
        }
        if !self.config.enable_remote_access {
            return RemoteAccessPolicyResult::RejectDueToRemoteAccessDisabled;
        }
        if self.remote_address_filter.is_empty() {
            return RemoteAccessPolicyResult::Allow;
        }

        let matches = self
            .remote_address_filter
            .iter()
            .any(|network| subnet_contains_address(*network, remote_ip));
        if self.config.is_remote_ip_filter_blacklist {
            if matches {
                RemoteAccessPolicyResult::RejectDueToIpBlocklist
            } else {
                RemoteAccessPolicyResult::Allow
            }
        } else if matches {
            RemoteAccessPolicyResult::Allow
        } else {
            RemoteAccessPolicyResult::RejectDueToNotAllowlistedRemoteIp
        }
    }

    #[must_use]
    pub fn get_bind_address(&self, source: &str) -> String {
        self.get_bind_address_for_ip(self.resolve_host(source).first().copied(), false)
    }

    #[must_use]
    pub fn get_bind_address_for_ip(&self, source: Option<IpAddr>, skip_overrides: bool) -> String {
        if let Some(source) = source {
            let external = !self.is_in_local_network(source);
            if !skip_overrides
                && let Some(value) = self.match_published_server_url(source, external)
            {
                return value;
            }
            if let Some(value) = self.match_bind_interface(source, external) {
                return value;
            }
            if external && let Some(value) = self.match_external_interface(source) {
                return value;
            }
        }

        let mut available = self
            .interfaces
            .iter()
            .filter(|interface| !interface.address().is_loopback())
            .collect::<Vec<_>>();
        available.sort_by_key(|interface| {
            (
                !self.is_in_local_network(interface.address()),
                interface.index,
            )
        });

        if available.is_empty() {
            return match source {
                Some(IpAddr::V4(_)) if self.config.enable_ipv4 => "127.0.0.1".to_owned(),
                Some(IpAddr::V6(_)) if self.config.enable_ipv6 => "::1".to_owned(),
                _ if self.config.enable_ipv4 => "127.0.0.1".to_owned(),
                _ if self.config.enable_ipv6 => "::1".to_owned(),
                _ => "127.0.0.1".to_owned(),
            };
        }
        let Some(source) = source else {
            return format_ip_string(Some(available[0].address()));
        };

        if let Some(interface) = available
            .iter()
            .find(|interface| subnet_contains_address(interface.subnet(), source))
        {
            return format_ip_string(Some(interface.address()));
        }
        let interface = available
            .iter()
            .find(|interface| same_family(interface.address(), source))
            .unwrap_or(&available[0]);
        format_ip_string(Some(interface.address()))
    }

    fn initialize(&mut self) {
        self.initialize_lan();
        self.initialize_remote();
        self.filter_interfaces();
        self.initialize_overrides();
    }

    fn initialize_lan(&mut self) {
        self.lan_subnets = try_parse_to_subnets(&self.config.local_network_subnets, false)
            .map(|result| result.subnets.into_iter().map(|data| data.subnet).collect())
            .unwrap_or_else(|| default_lan_subnets(&self.config));
        self.excluded_subnets = try_parse_to_subnets(&self.config.local_network_subnets, true)
            .map(|result| result.subnets.into_iter().map(|data| data.subnet).collect())
            .unwrap_or_default();
    }

    fn initialize_remote(&mut self) {
        self.remote_address_filter = self
            .config
            .remote_ip_filter
            .iter()
            .filter_map(|value| try_parse_to_subnet(value, false).map(|data| data.subnet))
            .collect();
    }

    fn filter_interfaces(&mut self) {
        if self
            .config
            .local_network_addresses
            .first()
            .is_some_and(|value| !value.trim().is_empty())
        {
            let mut addresses = HashSet::new();
            for value in &self.config.local_network_addresses {
                if let Some(data) = try_parse_to_subnet(value, false) {
                    addresses.insert(data.address);
                } else {
                    for interface in &self.interfaces {
                        if interface.name.eq_ignore_ascii_case(value.trim()) {
                            addresses.insert(interface.address());
                        }
                    }
                }
            }
            self.interfaces
                .retain(|interface| addresses.contains(&interface.address()));

            if addresses.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST))
                && !self
                    .interfaces
                    .iter()
                    .any(|i| i.address() == IpAddr::V4(Ipv4Addr::LOCALHOST))
                && self.config.enable_ipv4
            {
                self.interfaces.push(NetworkInterface::new(
                    IpData {
                        address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                        subnet: network("127.0.0.0/8"),
                    },
                    1,
                    "lo",
                ));
            }

            if addresses.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST))
                && !self
                    .interfaces
                    .iter()
                    .any(|i| i.address() == IpAddr::V6(Ipv6Addr::LOCALHOST))
                && self.config.enable_ipv6
            {
                self.interfaces.push(NetworkInterface::new(
                    IpData {
                        address: IpAddr::V6(Ipv6Addr::LOCALHOST),
                        subnet: network("::1/128"),
                    },
                    2,
                    "lo",
                ));
            }
        }

        if self.config.ignore_virtual_interfaces {
            let prefixes = self
                .config
                .virtual_interface_names
                .iter()
                .map(|value| value.replace('*', "").to_ascii_lowercase())
                .collect::<Vec<_>>();
            self.interfaces.retain(|interface| {
                let name = interface.name.to_ascii_lowercase();
                !prefixes.iter().any(|prefix| name.starts_with(prefix))
            });
        }
        let enable_ipv4 = self.config.enable_ipv4;
        let enable_ipv6 = self.config.enable_ipv6;
        self.interfaces
            .retain(|interface| match interface.address() {
                IpAddr::V4(_) => enable_ipv4,
                IpAddr::V6(_) => enable_ipv6,
            });

        if self.interfaces.is_empty() {
            if self.config.enable_ipv4 {
                self.interfaces.push(NetworkInterface::new(
                    IpData {
                        address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                        subnet: network("127.0.0.0/8"),
                    },
                    1,
                    "lo",
                ));
            }
            if self.config.enable_ipv6 {
                self.interfaces.push(NetworkInterface::new(
                    IpData {
                        address: IpAddr::V6(Ipv6Addr::LOCALHOST),
                        subnet: network("::1/128"),
                    },
                    2,
                    "lo",
                ));
            }
        }

        let mut addresses = HashSet::new();
        self.interfaces
            .retain(|interface| addresses.insert(interface.address()));
    }

    fn initialize_overrides(&mut self) {
        let entries = std::mem::take(&mut self.config.published_server_uri_by_subnet);
        for entry in &entries {
            let Some((identifier, replacement)) = entry.split_once('=') else {
                continue;
            };
            let identifier = identifier.trim();
            let replacement = replacement.trim().to_owned();
            if identifier.eq_ignore_ascii_case("all") {
                self.published_server_urls.clear();
                self.add_any_override(&replacement, true, true);
                break;
            }
            if identifier.eq_ignore_ascii_case("external") {
                self.add_any_override(&replacement, false, true);
            } else if identifier.eq_ignore_ascii_case("internal") {
                for &subnet in &self.lan_subnets {
                    self.published_server_urls.push(PublishedServerUriOverride {
                        data: IpData {
                            address: subnet.base_address(),
                            subnet,
                        },
                        uri: replacement.to_owned(),
                        internal: true,
                        external: false,
                    });
                }
            } else if let Some(data) = try_parse_to_subnet(identifier, false) {
                self.published_server_urls.push(PublishedServerUriOverride {
                    data,
                    uri: replacement,
                    internal: true,
                    external: true,
                });
            } else {
                for data in self.try_parse_interface(identifier) {
                    self.published_server_urls.push(PublishedServerUriOverride {
                        data,
                        uri: replacement.to_owned(),
                        internal: true,
                        external: true,
                    });
                }
            }
        }
        self.config.published_server_uri_by_subnet = entries;
    }

    fn add_any_override(&mut self, uri: &str, internal: bool, external: bool) {
        for (address, prefix) in [
            (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        ] {
            self.published_server_urls.push(PublishedServerUriOverride {
                data: IpData {
                    address,
                    subnet: IpNetwork::new(address, prefix).expect("zero prefix is valid"),
                },
                uri: uri.to_owned(),
                internal,
                external,
            });
        }
    }

    fn match_published_server_url(&self, source: IpAddr, external: bool) -> Option<String> {
        let mut overrides = self
            .published_server_urls
            .iter()
            .filter(|entry| {
                (if external {
                    entry.external
                } else {
                    entry.internal
                }) && subnet_contains_address(entry.data.subnet, source)
            })
            .collect::<Vec<_>>();
        overrides.sort_by_key(|entry| std::cmp::Reverse(entry.data.subnet.prefix_length()));
        overrides.into_iter().find_map(|entry| {
            let any = entry.data.address.is_unspecified();
            let has_interface = self
                .interfaces
                .iter()
                .any(|interface| subnet_contains_address(entry.data.subnet, interface.address()));
            (any || has_interface).then(|| entry.uri.clone())
        })
    }

    fn match_bind_interface(&self, source: IpAddr, external: bool) -> Option<String> {
        if external {
            let mut interfaces = self
                .interfaces
                .iter()
                .filter(|interface| {
                    !self.is_in_local_network(interface.address())
                        && !is_link_local(interface.address())
                })
                .collect::<Vec<_>>();
            interfaces.sort_by_key(|interface| {
                (
                    !subnet_contains_address(interface.subnet(), source),
                    !same_family(interface.address(), source),
                    std::cmp::Reverse(interface.subnet().prefix_length()),
                    interface.index,
                )
            });
            interfaces
                .first()
                .map(|interface| format_ip_string(Some(interface.address())))
        } else {
            let mut interfaces = self
                .interfaces
                .iter()
                .filter(|interface| self.is_in_local_network(interface.address()))
                .collect::<Vec<_>>();
            interfaces.sort_by_key(|interface| {
                (
                    !subnet_contains_address(interface.subnet(), source),
                    !same_family(interface.address(), source),
                    std::cmp::Reverse(interface.subnet().prefix_length()),
                    interface.index,
                )
            });
            interfaces
                .first()
                .map(|interface| format_ip_string(Some(interface.address())))
        }
    }

    fn match_external_interface(&self, source: IpAddr) -> Option<String> {
        let mut interfaces = self
            .interfaces
            .iter()
            .filter(|interface| {
                !self.is_in_local_network(interface.address())
                    && same_family(interface.address(), source)
                    && !is_link_local(interface.address())
            })
            .collect::<Vec<_>>();
        interfaces.sort_by_key(|interface| interface.index);
        interfaces
            .iter()
            .find(|interface| subnet_contains_address(interface.subnet(), source))
            .copied()
            .or_else(|| interfaces.first().copied())
            .map(|interface| format_ip_string(Some(interface.address())))
    }

    const fn family_enabled(&self, address: IpAddr) -> bool {
        match address {
            IpAddr::V4(_) => self.config.enable_ipv4,
            IpAddr::V6(_) => self.config.enable_ipv6,
        }
    }
}

pub fn parse_interface_fixture(value: &str) -> Result<Vec<NetworkInterface>, NetworkManagerError> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split('|')
        .map(|entry| {
            let mut parts = entry.split(',');
            let address = parts.next().unwrap_or_default();
            let index = parts
                .next()
                .ok_or_else(|| NetworkManagerError::InvalidInterfaceFixture(entry.to_owned()))?;
            let name = parts
                .next()
                .ok_or_else(|| NetworkManagerError::InvalidInterfaceFixture(entry.to_owned()))?;
            if parts.next().is_some() {
                return Err(NetworkManagerError::InvalidInterfaceFixture(
                    entry.to_owned(),
                ));
            }
            let data = try_parse_to_subnet(address, false)
                .ok_or_else(|| NetworkManagerError::InvalidInterfaceFixture(entry.to_owned()))?;
            let index = index
                .parse()
                .map_err(|_| NetworkManagerError::InvalidInterfaceIndex(index.to_owned()))?;
            Ok(NetworkInterface::new(data, index, name))
        })
        .collect()
}

fn default_lan_subnets(config: &NetworkConfiguration) -> Vec<IpNetwork> {
    let mut networks = Vec::new();
    if config.enable_ipv6 {
        networks.extend([
            network("::1/128"),
            network("fe80::/10"),
            network("fc00::/7"),
        ]);
    }
    if config.enable_ipv4 {
        networks.extend([
            network("127.0.0.0/8"),
            network("10.0.0.0/8"),
            network("172.16.0.0/12"),
            network("192.168.0.0/16"),
        ]);
    }
    networks
}

fn network(value: &str) -> IpNetwork {
    value.parse().expect("built-in network must be valid")
}

fn is_link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => network("169.254.0.0/16").contains(IpAddr::V4(address)),
        IpAddr::V6(_) => is_ipv6_link_local(address),
    }
}

fn map_ipv4_mapped(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address => address,
    }
}

const fn same_family(left: IpAddr, right: IpAddr) -> bool {
    matches!(
        (left, right),
        (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
    )
}
