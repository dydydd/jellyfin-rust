use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use jellyfin_networking::{
    AddressFamily, IpNetwork, NetworkParseError, ParsedHost, SubnetParseWarning, broadcast_address,
    cidr_to_mask, format_ip_string, is_ipv6_link_local, mask_to_cidr, subnet_contains_address,
    try_parse_host, try_parse_to_subnet, try_parse_to_subnets,
};

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
}

#[test]
fn valid_host_strings_are_accepted() {
    for value in [
        "127.0.0.1",
        "127.0.0.1:123",
        "localhost",
        "localhost:1345",
        "fd23:184f:2029:0:3139:7386:67d7:d517",
        "fd23:184f:2029:0:3139:7386:67d7:d517/56",
        "[fd23:184f:2029:0:3139:7386:67d7:d517]:124",
        "fe80::7add:12ff:febb:c67b%16",
        "[fe80::7add:12ff:febb:c67b%16]:123",
        "fe80::7add:12ff:febb:c67b%16:123",
        "[fe80::7add:12ff:febb:c67b%16]",
        "192.168.1.2/255.255.255.0",
        "192.168.1.2/24",
    ] {
        assert!(try_parse_host(value, true, true).is_some(), "host: {value}");
    }
}

#[test]
fn invalid_host_strings_are_rejected() {
    for value in [
        "256.128.0.0.0.1",
        "127.0.0.1#",
        "localhost!",
        "fd23:184f:2029:0:3139:7386:67d7:d517:1231",
        "[fd23:184f:2029:0:3139:7386:67d7:d517:1231]",
    ] {
        assert!(try_parse_host(value, true, true).is_none(), "host: {value}");
    }
}

#[test]
fn host_parser_preserves_names_and_respects_enabled_families() {
    assert_eq!(
        try_parse_host("media.example", true, true),
        Some(ParsedHost::Name("media.example".to_owned()))
    );
    assert!(try_parse_host("192.0.2.1", false, true).is_none());
    assert!(try_parse_host("2001:db8::1", true, false).is_none());
}

#[test]
fn valid_subnet_strings_are_accepted() {
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
        assert!(
            try_parse_to_subnet(value, false).is_some(),
            "subnet: {value}"
        );
        let negated = format!("!{value}");
        assert!(try_parse_to_subnet(&negated, true).is_some());
    }
}

#[test]
fn invalid_subnet_strings_are_rejected() {
    for value in [
        "127.0.0.1#",
        "localhost!",
        "256.128.0.0.0.1",
        "fd23:184f:2029:0:3139:7386:67d7:d517:1231",
        "[fd23:184f:2029:0:3139:7386:67d7:d517:1231]",
        "fd23:184f:2029:0100/56",
    ] {
        assert!(
            try_parse_to_subnet(value, false).is_none(),
            "subnet: {value}"
        );
    }
}

#[test]
fn subnet_list_parser_filters_polarity_without_warning() {
    let values = ["127.0.0.0/8", "192.168.178.0/24", "!10.0.0.0/8"];
    let included = try_parse_to_subnets(&values, false).unwrap();
    assert_eq!(included.subnets.len(), 2);
    assert!(included.warnings.is_empty());

    let excluded = try_parse_to_subnets(&values, true).unwrap();
    assert_eq!(excluded.subnets.len(), 1);
    assert!(excluded.warnings.is_empty());

    let values = ["fd00::/8", "fe80::/10", "!fd12:3456:789a::/48"];
    assert_eq!(
        try_parse_to_subnets(&values, false).unwrap().subnets.len(),
        2
    );
    assert_eq!(
        try_parse_to_subnets(&values, true).unwrap().subnets.len(),
        1
    );
}

#[test]
fn subnet_list_parser_classifies_invalid_entries() {
    let values = ["10.0.0.0/8", "fd23:184f:2029:0100/56", "not-an-address"];
    let result = try_parse_to_subnets(&values, false).unwrap();
    assert_eq!(result.subnets.len(), 1);
    assert_eq!(
        result.warnings,
        [
            SubnetParseWarning::Ipv6PrefixOnly("fd23:184f:2029:0100/56".to_owned()),
            SubnetParseWarning::Invalid("not-an-address".to_owned()),
        ]
    );
}

#[test]
fn ipv4_network_membership_matches_official_matrix() {
    for (network, address) in [
        ("192.168.5.85/24", "192.168.5.1"),
        ("192.168.5.85/24", "192.168.5.254"),
        ("10.128.240.50/30", "10.128.240.48"),
        ("10.128.240.50/30", "10.128.240.49"),
        ("10.128.240.50/30", "10.128.240.50"),
        ("10.128.240.50/30", "10.128.240.51"),
        ("127.0.0.1/8", "127.0.0.1"),
    ] {
        assert!(IpNetwork::from_str(network).unwrap().contains(ip(address)));
    }
    for (network, address) in [
        ("192.168.5.85/24", "192.168.4.254"),
        ("192.168.5.85/24", "191.168.5.254"),
        ("10.128.240.50/30", "10.128.240.47"),
        ("10.128.240.50/30", "10.128.240.52"),
        ("10.128.240.50/30", "10.128.239.50"),
        ("10.128.240.50/30", "10.127.240.51"),
    ] {
        assert!(!IpNetwork::from_str(network).unwrap().contains(ip(address)));
    }
}

#[test]
fn ipv6_network_membership_matches_official_matrix() {
    let network = IpNetwork::from_str("2001:db8:abcd:0012::0/64").unwrap();
    for address in [
        "2001:0DB8:ABCD:0012:0000:0000:0000:0000",
        "2001:0DB8:ABCD:0012:FFFF:FFFF:FFFF:FFFF",
        "2001:0DB8:ABCD:0012:0001:0000:0000:0000",
        "2001:0DB8:ABCD:0012:FFFF:FFFF:FFFF:FFF0",
    ] {
        assert!(network.contains(ip(address)));
    }
    for address in [
        "2001:0DB8:ABCD:0011:FFFF:FFFF:FFFF:FFFF",
        "2001:0DB8:ABCD:0013:0000:0000:0000:0000",
        "2001:0DB8:ABCD:0013:0001:0000:0000:0000",
        "2001:0DB8:ABCD:0011:FFFF:FFFF:FFFF:FFF0",
    ] {
        assert!(!network.contains(ip(address)));
    }

    let exact = IpNetwork::from_str("2001:db8:abcd:0012::0/128").unwrap();
    assert!(exact.contains(ip("2001:db8:abcd:12::")));
    assert!(!exact.contains(ip("2001:db8:abcd:12::1")));
}

#[test]
fn masks_broadcast_formatting_and_link_local_match_network_utils() {
    assert_eq!(
        cidr_to_mask(24, AddressFamily::Ipv4).unwrap(),
        ip("255.255.255.0")
    );
    assert_eq!(mask_to_cidr(ip("255.255.255.0")).unwrap(), 24);
    assert!(matches!(
        mask_to_cidr(ip("255.0.255.0")),
        Err(NetworkParseError::InvalidMask(_))
    ));
    assert_eq!(
        broadcast_address(IpNetwork::from_str("192.168.5.85/24").unwrap()),
        Some(Ipv4Addr::new(192, 168, 5, 255))
    );
    assert_eq!(format_ip_string(Some(ip("192.0.2.1"))), "192.0.2.1");
    assert_eq!(format_ip_string(Some(ip("2001:db8::1"))), "[2001:db8::1]");
    assert!(is_ipv6_link_local(ip("fe80::1")));
    assert!(is_ipv6_link_local(ip("febf::1")));
    assert!(!is_ipv6_link_local(ip("fec0::1")));
}

#[test]
fn mapped_ipv4_addresses_match_ipv4_networks() {
    let network = IpNetwork::from_str("192.168.1.0/24").unwrap();
    let mapped = IpAddr::V6(Ipv6Addr::from_str("::ffff:192.168.1.42").unwrap());
    assert!(!network.contains(mapped));
    assert!(subnet_contains_address(network, mapped));
}

#[test]
fn networks_are_canonicalized_and_prefixes_validated() {
    let network = IpNetwork::from_str("192.168.5.85/24").unwrap();
    assert_eq!(network.base_address(), ip("192.168.5.0"));
    assert_eq!(network.to_string(), "192.168.5.0/24");
    assert!(IpNetwork::from_str("192.168.1.1/33").is_err());
    assert!(IpNetwork::from_str("2001:db8::1/129").is_err());
}
