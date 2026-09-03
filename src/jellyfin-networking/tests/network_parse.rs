mod common;

use std::str::FromStr;

use common::{FixtureResolver, ip, strings};
use jellyfin_networking::{
    IpNetwork, NetworkConfiguration, NetworkManager, RemoteAccessPolicyResult, SubnetParseWarning,
    try_parse_to_subnet, try_parse_to_subnets,
};

const TWO_INTERFACES: &str = "192.168.1.208/24,-16,eth16|200.200.200.200/24,11,eth11";

fn resolver() -> FixtureResolver {
    FixtureResolver::default().with("jellyfin.org", &["203.0.113.10"])
}

fn fixture_manager(config: NetworkConfiguration, fixture: &str) -> NetworkManager {
    NetworkManager::from_fixture(config, fixture, resolver()).unwrap()
}

#[test]
fn ignore_virtual_interfaces_official_matrix() {
    let cases = [
        (
            TWO_INTERFACES,
            "192.168.1.0/24;200.200.200.0/24",
            "[192.168.1.208/24,200.200.200.200/24]",
        ),
        (TWO_INTERFACES, "192.168.1.0/24", "[192.168.1.208/24]"),
        (
            "192.168.1.208,-16,eth16|200.200.200.200,11,eth11",
            "192.168.1.0/24",
            "[192.168.1.208/32]",
        ),
        (
            "192.168.1.208/24,-16,vEthernet1|192.168.2.208/24,-16,vEthernet212|200.200.200.200/24,11,eth11",
            "192.168.1.0/24",
            "[]",
        ),
        (
            "192.168.1.200/24,-20,vEthernet1|192.168.2.208/24,-16,vEthernet212|200.200.200.200/24,11,eth11",
            "192.168.1.0/24;200.200.200.200/24",
            "[200.200.200.200/24]",
        ),
        (
            "192.168.1.110/24,-20,br0|192.168.1.10/24,-16,br0|200.200.200.200/24,11,eth11",
            "192.168.1.0/24",
            "[192.168.1.110/24,192.168.1.10/24]",
        ),
    ];

    for (interfaces, lan, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.enable_ipv4 = true;
        config.enable_ipv6 = true;
        config.local_network_subnets = strings(lan, ';');
        let manager = fixture_manager(config, interfaces);
        let actual = manager
            .get_internal_bind_addresses()
            .iter()
            .map(|data| format!("{}/{}", data.address, data.subnet.prefix_length()))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(format!("[{actual}]"), expected);
    }
}

#[test]
fn subnet_parsing_and_warning_matrices_match_official_tests() {
    for value in [
        "127.0.0.1",
        "127.0.0.1/8",
        "192.168.1.2",
        "192.168.1.2/24",
        "fd23:184f:2029:0:3139:7386:67d7:d517",
        "[fd23:184f:2029:0:3139:7386:67d7:d517]",
        "fe80::7add:12ff:febb:c67b%16",
        "[fe80::7add:12ff:febb:c67b%16]:123",
        "fe80::7add:12ff:febb:c67b%16:123",
        "[fe80::7add:12ff:febb:c67b%16]",
        "fd23:184f:2029:0:3139:7386:67d7:d517/56",
    ] {
        assert!(try_parse_to_subnet(value, false).is_some());
        assert!(try_parse_to_subnet(&format!("!{value}"), true).is_some());
    }
    for value in [
        "127.0.0.1#",
        "localhost!",
        "256.128.0.0.0.1",
        "fd23:184f:2029:0:3139:7386:67d7:d517:1231",
        "[fd23:184f:2029:0:3139:7386:67d7:d517:1231]",
        "fd23:184f:2029:0100/56",
    ] {
        assert!(try_parse_to_subnet(value, false).is_none());
    }

    let invalid = ["10.0.0.0/8", "fd23:184f:2029:0100/56", "not-an-address"];
    let result = try_parse_to_subnets(&invalid, false).unwrap();
    assert_eq!(result.subnets.len(), 1);
    assert_eq!(
        result.warnings,
        [
            SubnetParseWarning::Ipv6PrefixOnly("fd23:184f:2029:0100/56".to_owned()),
            SubnetParseWarning::Invalid("not-an-address".to_owned()),
        ]
    );

    for values in [
        vec!["127.0.0.0/8", "192.168.178.0/24", "!10.0.0.0/8"],
        vec!["fd00::/8", "fe80::/10", "!fd12:3456:789a::/48"],
    ] {
        let included = try_parse_to_subnets(&values, false).unwrap();
        let excluded = try_parse_to_subnets(&values, true).unwrap();
        assert_eq!(included.subnets.len(), 2);
        assert_eq!(excluded.subnets.len(), 1);
        assert!(included.warnings.is_empty());
        assert!(excluded.warnings.is_empty());
    }
}

#[test]
fn ipv4_and_ipv6_subnet_membership_official_matrix() {
    let matching = [
        ("192.168.5.85/24", "192.168.5.1"),
        ("192.168.5.85/24", "192.168.5.254"),
        ("10.128.240.50/30", "10.128.240.48"),
        ("10.128.240.50/30", "10.128.240.49"),
        ("10.128.240.50/30", "10.128.240.50"),
        ("10.128.240.50/30", "10.128.240.51"),
        ("127.0.0.1/8", "127.0.0.1"),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0012:0000:0000:0000:0000",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0012:FFFF:FFFF:FFFF:FFFF",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0012:0001:0000:0000:0000",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0012:FFFF:FFFF:FFFF:FFF0",
        ),
        (
            "2001:db8:abcd:0012::0/128",
            "2001:0DB8:ABCD:0012:0000:0000:0000:0000",
        ),
    ];
    for (network, address) in matching {
        assert!(IpNetwork::from_str(network).unwrap().contains(ip(address)));
    }

    let not_matching = [
        ("192.168.5.85/24", "192.168.4.254"),
        ("192.168.5.85/24", "191.168.5.254"),
        ("10.128.240.50/30", "10.128.240.47"),
        ("10.128.240.50/30", "10.128.240.52"),
        ("10.128.240.50/30", "10.128.239.50"),
        ("10.128.240.50/30", "10.127.240.51"),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0011:FFFF:FFFF:FFFF:FFFF",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0013:0000:0000:0000:0000",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0013:0001:0000:0000:0000",
        ),
        (
            "2001:db8:abcd:0012::0/64",
            "2001:0DB8:ABCD:0011:FFFF:FFFF:FFFF:FFF0",
        ),
        (
            "2001:db8:abcd:0012::0/128",
            "2001:0DB8:ABCD:0012:0000:0000:0000:0001",
        ),
    ];
    for (network, address) in not_matching {
        assert!(!IpNetwork::from_str(network).unwrap().contains(ip(address)));
    }
}

#[test]
fn bind_interface_selection_official_matrix_uses_fixture_dns() {
    let cases = [
        ("192.168.1.1", "eth16,eth11", "192.168.1.208"),
        ("8.8.8.8", "eth16,eth11", "200.200.200.200"),
        ("10.10.10.10", "eth16", "192.168.1.208"),
        ("192.168.1.1", "", "192.168.1.208"),
        ("jellyfin.org", "eth16", "192.168.1.208"),
        ("jellyfin.org", "", "200.200.200.200"),
        ("", "", "192.168.1.208"),
    ];
    for (source, binds, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.enable_ipv4 = true;
        config.local_network_addresses = strings(binds, ',');
        let manager = fixture_manager(config, TWO_INTERFACES);
        assert_eq!(
            manager.get_bind_address(source),
            expected,
            "source={source}"
        );
    }

    let manager = fixture_manager(NetworkConfiguration::default(), TWO_INTERFACES);
    assert!(manager.resolve_host("invalid.domain.test").is_empty());
}

#[test]
fn published_server_uri_overrides_official_matrix() {
    let cases = [
        (
            "192.168.1.1",
            "192.168.1.0/24",
            "eth16,eth11",
            "192.168.1.0/24=internal.jellyfin",
            "internal.jellyfin",
        ),
        (
            "8.8.8.8",
            "192.168.1.0/24",
            "eth16,eth11",
            "all=http://helloworld.com",
            "http://helloworld.com",
        ),
        (
            "10.10.10.10",
            "192.168.1.0/24",
            "eth16",
            "external=http://internalButNotDefinedAsLan.com",
            "http://internalButNotDefinedAsLan.com",
        ),
        (
            "192.168.1.1",
            "192.168.1.0/24",
            "",
            "external=http://helloworld.com",
            "192.168.1.208",
        ),
        (
            "jellyfin.org",
            "192.168.1.0/24",
            "eth16",
            "external=http://helloworld.com",
            "http://helloworld.com",
        ),
        (
            "jellyfin.org",
            "192.168.1.0/24",
            "",
            "external=http://helloworld.com",
            "http://helloworld.com",
        ),
        (
            "",
            "192.168.1.0/24",
            "",
            "all=http://helloworld.com",
            "192.168.1.208",
        ),
        (
            "192.168.1.1",
            "192.168.1.0/24",
            "",
            "eth16=http://helloworld.com",
            "http://helloworld.com",
        ),
    ];
    for (source, lan, binds, published, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.enable_ipv4 = true;
        config.local_network_subnets = strings(lan, ',');
        config.local_network_addresses = strings(binds, ',');
        config.published_server_uri_by_subnet = vec![published.to_owned()];
        let manager = fixture_manager(config, TWO_INTERFACES);
        assert_eq!(
            manager.get_bind_address(source),
            expected,
            "source={source}"
        );
    }
}

#[test]
fn remote_access_policy_official_matrices() {
    let cases = [
        (
            "185.10.10.10,200.200.200.200",
            false,
            true,
            "79.2.3.4",
            RemoteAccessPolicyResult::RejectDueToNotAllowlistedRemoteIp,
        ),
        (
            "185.10.10.10",
            false,
            true,
            "185.10.10.10",
            RemoteAccessPolicyResult::Allow,
        ),
        (
            "",
            false,
            true,
            "100.100.100.100",
            RemoteAccessPolicyResult::Allow,
        ),
        (
            "185.10.10.10,200.200.200.200",
            false,
            false,
            "79.2.3.4",
            RemoteAccessPolicyResult::RejectDueToRemoteAccessDisabled,
        ),
        (
            "185.10.10.10",
            false,
            false,
            "127.0.0.1",
            RemoteAccessPolicyResult::Allow,
        ),
        (
            "",
            false,
            false,
            "100.100.100.100",
            RemoteAccessPolicyResult::RejectDueToRemoteAccessDisabled,
        ),
        (
            "185.10.10.10",
            true,
            true,
            "79.2.3.4",
            RemoteAccessPolicyResult::Allow,
        ),
        (
            "185.10.10.10",
            true,
            true,
            "185.10.10.10",
            RemoteAccessPolicyResult::RejectDueToIpBlocklist,
        ),
        (
            "",
            true,
            true,
            "100.100.100.100",
            RemoteAccessPolicyResult::Allow,
        ),
    ];
    for (filter, blacklist, remote, address, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.remote_ip_filter = strings(filter, ',');
        config.is_remote_ip_filter_blacklist = blacklist;
        config.enable_remote_access = remote;
        let manager = fixture_manager(config, "");
        assert_eq!(manager.should_allow_server_access(ip(address)), expected);
    }
}

#[test]
fn get_bind_interface_without_source_official_matrix() {
    let cases = [
        (
            "192.168.1.209/24,-16,eth16",
            "192.168.1.0/24",
            "",
            "192.168.1.209",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "192.168.1.0/24",
            "",
            "192.168.1.208",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "192.168.1.0/24",
            "10.0.0.1",
            "10.0.0.1",
        ),
    ];
    for (interfaces, lan, bind, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.local_network_subnets = strings(lan, ',');
        config.local_network_addresses = strings(bind, ',');
        let manager = fixture_manager(config, interfaces);
        assert_eq!(manager.get_bind_address(""), expected);
    }
}

#[test]
fn get_bind_interface_with_source_official_matrix() {
    let cases = [
        (
            "192.168.1.209/24,-16,eth16",
            "",
            "192.168.1.210",
            "192.168.1.209",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "",
            "192.168.1.209",
            "192.168.1.208",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "",
            "8.8.8.8",
            "10.0.0.1",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "10.0.0.1",
            "192.168.1.209",
            "10.0.0.1",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "192.168.1.208,10.0.0.1",
            "8.8.8.8",
            "10.0.0.1",
        ),
        (
            "192.168.1.208/24,-16,eth16|10.0.0.1/24,10,eth7",
            "192.168.1.208,10.0.0.1",
            "192.168.1.210",
            "192.168.1.208",
        ),
        (
            "192.168.1.208/24,-16,eth16|fd00::1/64,10,eth7",
            "",
            "192.168.2.100",
            "192.168.1.208",
        ),
    ];
    for (interfaces, bind, source, expected) in cases {
        let mut config = NetworkConfiguration::default();
        config.local_network_subnets = vec!["192.168.1.0/24".to_owned()];
        config.local_network_addresses = strings(bind, ',');
        let manager = fixture_manager(config, interfaces);
        assert_eq!(
            manager.get_bind_address(source),
            expected,
            "source={source}"
        );
    }
}
