use std::collections::HashMap;
use std::net::IpAddr;

use jellyfin_networking::HostResolver;

#[derive(Clone, Debug, Default)]
pub struct FixtureResolver {
    addresses: HashMap<String, Vec<IpAddr>>,
}

impl FixtureResolver {
    #[allow(dead_code)]
    pub fn with(mut self, host: &str, addresses: &[&str]) -> Self {
        self.addresses.insert(
            host.to_owned(),
            addresses
                .iter()
                .map(|address| address.parse().expect("fixture IP must be valid"))
                .collect(),
        );
        self
    }
}

impl HostResolver for FixtureResolver {
    fn resolve(&self, host: &str) -> Vec<IpAddr> {
        self.addresses.get(host).cloned().unwrap_or_default()
    }
}

pub fn strings(values: &str, separator: char) -> Vec<String> {
    values.split(separator).map(str::to_owned).collect()
}

pub fn ip(value: &str) -> IpAddr {
    value.parse().expect("test IP must be valid")
}
