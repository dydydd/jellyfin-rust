//! Deterministic network parsing and address utilities used by Jellyfin.

mod configuration;
mod manager;
mod network;

pub use configuration::NetworkConfiguration;
pub use manager::{
    HostResolver, NetworkInterface, NetworkManager, NetworkManagerError, RemoteAccessPolicyResult,
    SystemHostResolver, parse_interface_fixture,
};
pub use network::{
    AddressFamily, IpData, IpNetwork, NetworkParseError, ParsedHost, SubnetParseResult,
    SubnetParseWarning, broadcast_address, cidr_to_mask, format_ip_string, is_ipv6_link_local,
    mask_to_cidr, subnet_contains_address, try_parse_host, try_parse_to_subnet,
    try_parse_to_subnets,
};
