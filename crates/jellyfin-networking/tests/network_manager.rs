mod common;

use common::{FixtureResolver, ip, strings};
use jellyfin_networking::{NetworkConfiguration, NetworkManager};

fn manager(networks: &str) -> NetworkManager {
    let mut config = NetworkConfiguration::default();
    config.enable_ipv4 = true;
    config.enable_ipv6 = true;
    config.local_network_subnets = strings(networks, ',');
    NetworkManager::with_resolver(config, Vec::new(), FixtureResolver::default())
}

#[test]
fn in_network_true_official_matrix() {
    for (network, address) in [
        ("192.168.2.1/24", "192.168.2.123"),
        ("192.168.2.1/24, !192.168.2.122/32", "192.168.2.123"),
        (
            "fd23:184f:2029:0::/56",
            "fd23:184f:2029:0:3139:7386:67d7:d517",
        ),
        (
            "fd23:184f:2029:0::/56, !fd23:184f:2029:0:3139:7386:67d7:d518/128",
            "fd23:184f:2029:0:3139:7386:67d7:d517",
        ),
    ] {
        assert!(manager(network).is_in_local_network(ip(address)));
    }
}

#[test]
fn in_network_false_official_matrix() {
    for (network, address) in [
        ("192.168.10.0/24", "192.168.11.1"),
        ("192.168.10.0/24, !192.168.10.60/32", "192.168.10.60"),
        ("192.168.10.0/24", "fd23:184f:2029:0:3139:7386:67d7:d517"),
        (
            "fd23:184f:2029:0::/56",
            "fd24:184f:2029:0:3139:7386:67d7:d517",
        ),
        (
            "fd23:184f:2029:0::/56, !fd23:184f:2029:0:3139:7386:67d7:d500/120",
            "fd23:184f:2029:0:3139:7386:67d7:d517",
        ),
        ("fd23:184f:2029:0::/56", "192.168.10.60"),
        ("2001:abcd:abcd:6b40::0/60", "192.168.10.60"),
    ] {
        assert!(!manager(network).is_in_local_network(ip(address)));
    }
}

#[test]
fn synthesizes_loopback_when_no_interfaces_available() {
    let mut config = NetworkConfiguration::default();
    config.enable_ipv4 = true;
    config.enable_ipv6 = true;
    let manager = NetworkManager::with_resolver(config, Vec::new(), FixtureResolver::default());
    let loopbacks = manager.get_loopbacks();
    assert_eq!(loopbacks.len(), 2);
    assert_eq!(loopbacks[0].address.to_string(), "127.0.0.1");
    assert_eq!(loopbacks[1].address.to_string(), "::1");
    assert_eq!(manager.interfaces().len(), 2);
    assert_eq!(manager.get_bind_address_for_ip(Some(ip("192.168.1.50")), false), "127.0.0.1");
    assert_eq!(manager.get_bind_address_for_ip(Some(ip("2001:db8::1")), false), "::1");
}
