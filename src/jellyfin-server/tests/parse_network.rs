use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use jellyfin_networking::{HostResolver, IpNetwork, NetworkConfiguration};
use jellyfin_server::{ForwardedHeaderProxyConfig, add_proxy_addresses};

#[derive(Clone, Copy)]
struct LocalhostResolver;

impl HostResolver for LocalhostResolver {
    fn resolve(&self, host: &str) -> Vec<IpAddr> {
        if host.eq_ignore_ascii_case("localhost") {
            vec![
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ]
        } else {
            Vec::new()
        }
    }
}

#[test]
fn add_proxy_addresses_matches_official_matrix() {
    let cases: &[(bool, bool, &[&str], &[IpAddr])] = &[
        (
            true,
            true,
            &["192.168.t", "127.0.0.1", "::1", "1234.1232.12.1234"],
            &[
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ],
        ),
        (
            true,
            false,
            &["192.168.x", "127.0.0.1", "1234.1232.12.1234"],
            &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        ),
        (true, true, &["::1"], &[IpAddr::V6(Ipv6Addr::LOCALHOST)]),
        (false, false, &["localhost"], &[]),
        (
            true,
            false,
            &["localhost"],
            &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
        ),
        (
            false,
            true,
            &["localhost"],
            &[IpAddr::V6(Ipv6Addr::LOCALHOST)],
        ),
        (
            true,
            true,
            &["localhost"],
            &[
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            ],
        ),
    ];

    for &(enable_ipv4, enable_ipv6, allowed_proxies, expected_proxies) in cases {
        let mut config = NetworkConfiguration::default();
        config.enable_ipv4 = enable_ipv4;
        config.enable_ipv6 = enable_ipv6;
        let mut options = ForwardedHeaderProxyConfig::default();

        add_proxy_addresses(
            &config,
            allowed_proxies.iter().copied(),
            &LocalhostResolver,
            &mut options,
        );

        assert_eq!(options.known_proxies, expected_proxies);
        assert!(options.known_networks.is_empty());
    }
}

#[test]
fn cidr_entries_are_known_networks_and_respect_enabled_families() {
    let mut config = NetworkConfiguration::default();
    config.enable_ipv4 = true;
    config.enable_ipv6 = false;
    let mut options = ForwardedHeaderProxyConfig::default();

    add_proxy_addresses(
        &config,
        ["192.168.10.23/24", "2001:db8::8/64", "10.0.0.1/32"],
        &LocalhostResolver,
        &mut options,
    );

    assert_eq!(
        options.known_networks,
        vec!["192.168.10.0/24".parse::<IpNetwork>().unwrap()]
    );
    assert_eq!(
        options.known_proxies,
        vec!["10.0.0.1".parse::<IpAddr>().unwrap()]
    );
}

#[test]
fn ipv4_mapped_ipv6_addresses_are_added_as_ipv4_proxies() {
    let mut config = NetworkConfiguration::default();
    config.enable_ipv4 = true;
    config.enable_ipv6 = false;
    let mut options = ForwardedHeaderProxyConfig::default();

    add_proxy_addresses(
        &config,
        ["::ffff:192.0.2.15"],
        &LocalhostResolver,
        &mut options,
    );

    assert_eq!(
        options.known_proxies,
        vec!["192.0.2.15".parse::<IpAddr>().unwrap()]
    );
    assert!(options.known_networks.is_empty());
}

#[test]
fn addresses_are_appended_and_invalid_or_unresolved_hosts_are_ignored() {
    let config = NetworkConfiguration::default();
    let existing = "10.0.0.1".parse::<IpAddr>().unwrap();
    let mut options = ForwardedHeaderProxyConfig {
        known_proxies: vec![existing],
        known_networks: Vec::new(),
    };

    add_proxy_addresses(
        &config,
        ["missing.invalid", "not a host", "192.0.2.1"],
        &LocalhostResolver,
        &mut options,
    );

    assert_eq!(
        options.known_proxies,
        vec![existing, "192.0.2.1".parse::<IpAddr>().unwrap()]
    );
    assert!(options.known_networks.is_empty());
}
