use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use jellyfin_networking::try_parse_host;

#[test]
fn try_parse_valid_host_strings_returns_true_official_matrix() {
    for address in [
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
        assert!(try_parse_host(address, true, true).is_some(), "{address}");
    }
}

#[test]
fn every_generated_ip_address_parses_without_dns() {
    let mut random = TestRandom::new(0x4a65_6c6c_7966_696e);
    for _ in 0..10_000 {
        let ipv4 = IpAddr::V4(Ipv4Addr::from(random.next_u32()));
        let ipv6 = IpAddr::V6(Ipv6Addr::from(random.next_u128()));
        for address in [ipv4, ipv6] {
            assert_eq!(
                try_parse_host(&address.to_string(), true, true),
                Some(jellyfin_networking::ParsedHost::Address(address))
            );
        }
    }
}

#[test]
fn try_parse_invalid_address_strings_returns_false_official_matrix() {
    for address in [
        "256.128.0.0.0.1",
        "127.0.0.1#",
        "localhost!",
        "fd23:184f:2029:0:3139:7386:67d7:d517:1231",
        "[fd23:184f:2029:0:3139:7386:67d7:d517:1231]",
    ] {
        assert!(try_parse_host(address, true, true).is_none(), "{address}");
    }
}

struct TestRandom(u64);

impl TestRandom {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    fn next_u128(&mut self) -> u128 {
        (u128::from(self.next_u64()) << 64) | u128::from(self.next_u64())
    }
}
