//! Server-host integration behavior that is shared by the Jellyfin binary and tests.

use std::net::IpAddr;

use jellyfin_networking::{
    HostResolver, IpNetwork, NetworkConfiguration, ParsedHost, try_parse_host,
};

/// Trusted proxy addresses and networks used by forwarded-header middleware.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForwardedHeaderProxyConfig {
    pub known_proxies: Vec<IpAddr>,
    pub known_networks: Vec<IpNetwork>,
}

/// Adds trusted proxy addresses using Jellyfin's IP, CIDR, and DNS parsing order.
///
/// Invalid entries and addresses from disabled families are ignored. Full-width
/// `/32` and `/128` entries are individual proxies; shorter prefixes are networks.
pub fn add_proxy_addresses<I, S, R>(
    config: &NetworkConfiguration,
    allowed_proxies: I,
    resolver: &R,
    options: &mut ForwardedHeaderProxyConfig,
) where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
    R: HostResolver + ?Sized,
{
    for allowed_proxy in allowed_proxies {
        let allowed_proxy = allowed_proxy.as_ref().trim();
        if let Ok(address) = allowed_proxy.parse::<IpAddr>() {
            add_ip_address(config, options, address, full_prefix(address));
            continue;
        }

        if let Ok(network) = allowed_proxy.parse::<IpNetwork>() {
            add_ip_address(
                config,
                options,
                network.base_address(),
                network.prefix_length(),
            );
            continue;
        }

        match try_parse_host(allowed_proxy, config.enable_ipv4, config.enable_ipv6) {
            Some(ParsedHost::Address(address)) => {
                add_ip_address(config, options, address, full_prefix(address));
            }
            Some(ParsedHost::Name(name)) => {
                for address in resolver.resolve(&name) {
                    add_ip_address(config, options, address, full_prefix(address));
                }
            }
            None => {}
        }
    }
}

fn add_ip_address(
    config: &NetworkConfiguration,
    options: &mut ForwardedHeaderProxyConfig,
    address: IpAddr,
    prefix_length: u8,
) {
    let (address, prefix_length) = map_ipv4_mapped(address, prefix_length);
    if !family_enabled(config, address) {
        return;
    }

    if prefix_length == full_prefix(address) {
        options.known_proxies.push(address);
    } else if let Ok(network) = IpNetwork::new(address, prefix_length) {
        options.known_networks.push(network);
    }
}

const fn family_enabled(config: &NetworkConfiguration, address: IpAddr) -> bool {
    match address {
        IpAddr::V4(_) => config.enable_ipv4,
        IpAddr::V6(_) => config.enable_ipv6,
    }
}

const fn full_prefix(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

fn map_ipv4_mapped(address: IpAddr, prefix_length: u8) -> (IpAddr, u8) {
    match address {
        IpAddr::V6(address) => {
            address
                .to_ipv4_mapped()
                .map_or((IpAddr::V6(address), prefix_length), |address| {
                    // IPv4-mapped IPv6 prefixes contain a 96-bit mapping marker.
                    // Drop it together with the marker when converting the address.
                    (IpAddr::V4(address), prefix_length.saturating_sub(96))
                })
        }
        address @ IpAddr::V4(_) => (address, prefix_length),
    }
}
