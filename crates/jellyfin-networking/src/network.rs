use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// Address family used by CIDR and mask helpers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

impl AddressFamily {
    const fn bit_count(self) -> u8 {
        match self {
            Self::Ipv4 => 32,
            Self::Ipv6 => 128,
        }
    }
}

/// Errors produced while constructing or parsing an IP network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkParseError {
    Empty,
    InvalidAddress(String),
    InvalidPrefix(String),
    PrefixOutOfRange { prefix: u8, family: AddressFamily },
    InvalidMask(IpAddr),
}

impl fmt::Display for NetworkParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("network value is empty"),
            Self::InvalidAddress(value) => write!(formatter, "invalid IP address: {value}"),
            Self::InvalidPrefix(value) => write!(formatter, "invalid network prefix: {value}"),
            Self::PrefixOutOfRange { prefix, family } => {
                write!(formatter, "prefix {prefix} is out of range for {family:?}")
            }
            Self::InvalidMask(mask) => write!(formatter, "non-contiguous network mask: {mask}"),
        }
    }
}

impl std::error::Error for NetworkParseError {}

/// An IPv4 or IPv6 network in CIDR notation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct IpNetwork {
    base_address: IpAddr,
    prefix_length: u8,
}

impl IpNetwork {
    /// Constructs a network and canonicalizes host bits in its base address.
    ///
    /// # Errors
    ///
    /// Returns [`NetworkParseError::PrefixOutOfRange`] when the prefix exceeds
    /// the address family's bit width.
    pub fn new(address: IpAddr, prefix_length: u8) -> Result<Self, NetworkParseError> {
        let family = family_of(address);
        if prefix_length > family.bit_count() {
            return Err(NetworkParseError::PrefixOutOfRange {
                prefix: prefix_length,
                family,
            });
        }

        Ok(Self {
            base_address: canonical_address(address, prefix_length),
            prefix_length,
        })
    }

    #[must_use]
    pub const fn base_address(self) -> IpAddr {
        self.base_address
    }

    #[must_use]
    pub const fn prefix_length(self) -> u8 {
        self.prefix_length
    }

    /// Returns whether an address belongs to this network.
    #[must_use]
    pub fn contains(self, address: IpAddr) -> bool {
        match (self.base_address, address) {
            (IpAddr::V4(base), IpAddr::V4(candidate)) => {
                let mask = prefix_mask_v4(self.prefix_length);
                u32::from(base) & mask == u32::from(candidate) & mask
            }
            (IpAddr::V6(base), IpAddr::V6(candidate)) => {
                let mask = prefix_mask_v6(self.prefix_length);
                u128::from(base) & mask == u128::from(candidate) & mask
            }
            _ => false,
        }
    }
}

impl fmt::Display for IpNetwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.base_address, self.prefix_length)
    }
}

impl FromStr for IpNetwork {
    type Err = NetworkParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err(NetworkParseError::Empty);
        }
        let (address, prefix) = value
            .split_once('/')
            .ok_or_else(|| NetworkParseError::InvalidPrefix(value.to_owned()))?;
        if prefix.contains('/') {
            return Err(NetworkParseError::InvalidPrefix(prefix.to_owned()));
        }
        let address = parse_ip_literal(address)
            .ok_or_else(|| NetworkParseError::InvalidAddress(address.to_owned()))?;
        let prefix = prefix
            .parse::<u8>()
            .map_err(|_| NetworkParseError::InvalidPrefix(prefix.to_owned()))?;
        Self::new(address, prefix)
    }
}

/// Original address and the subnet parsed from a Jellyfin configuration entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IpData {
    pub address: IpAddr,
    pub subnet: IpNetwork,
}

/// Syntactically parsed host that can be resolved by a higher networking layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedHost {
    Address(IpAddr),
    Name(String),
}

/// Warning category for an invalid member of a subnet list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubnetParseWarning {
    Ipv6PrefixOnly(String),
    Invalid(String),
}

/// Successfully parsed subnet entries plus warnings for malformed entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubnetParseResult {
    pub subnets: Vec<IpData>,
    pub warnings: Vec<SubnetParseWarning>,
}

/// Parses a Jellyfin subnet entry, respecting its leading `!` polarity.
#[must_use]
pub fn try_parse_to_subnet(value: &str, negated: bool) -> Option<IpData> {
    let mut value = value.trim();
    let is_negated = value.starts_with('!');
    if is_negated {
        value = value[1..].trim_start();
    }
    if is_negated != negated || value.is_empty() {
        return None;
    }

    if value.contains('/') {
        let (address, _) = value.split_once('/')?;
        let address = parse_ip_literal(address)?;
        let subnet = value.parse::<IpNetwork>().ok()?;
        return Some(IpData { address, subnet });
    }

    let address = parse_ip_literal(value)?;
    let prefix = family_of(address).bit_count();
    let subnet = IpNetwork::new(address, prefix).ok()?;
    Some(IpData { address, subnet })
}

/// Parses subnet entries of one polarity. Entries of the other polarity are
/// deliberately skipped without warnings, matching Jellyfin's two-pass parser.
pub fn try_parse_to_subnets<S: AsRef<str>>(
    values: &[S],
    negated: bool,
) -> Option<SubnetParseResult> {
    if values.is_empty() {
        return None;
    }

    let mut result = SubnetParseResult {
        subnets: Vec::new(),
        warnings: Vec::new(),
    };
    for value in values {
        let value = value.as_ref();
        let trimmed = value.trim();
        if trimmed.starts_with('!') != negated {
            continue;
        }
        if let Some(subnet) = try_parse_to_subnet(value, negated) {
            result.subnets.push(subnet);
        } else {
            result.warnings.push(classify_subnet_warning(value));
        }
    }

    (!result.subnets.is_empty()).then_some(result)
}

/// Parses an IP literal or validates a hostname without performing DNS I/O.
#[must_use]
pub fn try_parse_host(host: &str, ipv4_enabled: bool, ipv6_enabled: bool) -> Option<ParsedHost> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }

    if let Some(address) = parse_ip_literal(left_part(host, '/')) {
        return address_family_enabled(address, ipv4_enabled, ipv6_enabled)
            .then_some(ParsedHost::Address(address));
    }

    let name = host_and_optional_port(host)?;
    if let Some(address) = parse_ip_literal(left_part(name, '/')) {
        return address_family_enabled(address, ipv4_enabled, ipv6_enabled)
            .then_some(ParsedHost::Address(address));
    }
    is_valid_hostname(name).then(|| ParsedHost::Name(name.to_owned()))
}

/// Returns true for the complete IPv6 link-local range `fe80::/10`.
#[must_use]
pub fn is_ipv6_link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V6(address) => address.segments()[0] & 0xffc0 == 0xfe80,
        IpAddr::V4(_) => false,
    }
}

/// Converts a CIDR prefix to a contiguous address mask.
///
/// # Errors
///
/// Returns [`NetworkParseError::PrefixOutOfRange`] when the prefix exceeds
/// the address family's bit width.
pub fn cidr_to_mask(prefix: u8, family: AddressFamily) -> Result<IpAddr, NetworkParseError> {
    if prefix > family.bit_count() {
        return Err(NetworkParseError::PrefixOutOfRange { prefix, family });
    }
    Ok(match family {
        AddressFamily::Ipv4 => IpAddr::V4(Ipv4Addr::from(prefix_mask_v4(prefix))),
        AddressFamily::Ipv6 => IpAddr::V6(Ipv6Addr::from(prefix_mask_v6(prefix))),
    })
}

/// Converts a contiguous IPv4 or IPv6 mask to its CIDR prefix.
///
/// # Errors
///
/// Returns [`NetworkParseError::InvalidMask`] when the mask contains a one bit
/// after its first zero bit.
pub fn mask_to_cidr(mask: IpAddr) -> Result<u8, NetworkParseError> {
    let (value, bits) = match mask {
        IpAddr::V4(mask) => (u128::from(u32::from(mask)), 32_u8),
        IpAddr::V6(mask) => (u128::from(mask), 128_u8),
    };
    let shift = 128 - u32::from(bits);
    let value = value << shift;
    let prefix =
        u8::try_from(value.leading_ones()).map_err(|_| NetworkParseError::InvalidMask(mask))?;
    let expected = prefix_mask_v6(prefix);
    if value == expected {
        Ok(prefix)
    } else {
        Err(NetworkParseError::InvalidMask(mask))
    }
}

/// Formats an IP for use as the host component of a URI.
#[must_use]
pub fn format_ip_string(address: Option<IpAddr>) -> String {
    match address {
        None => String::new(),
        Some(IpAddr::V4(address)) => address.to_string(),
        Some(IpAddr::V6(address)) => format!("[{address}]"),
    }
}

/// Returns the IPv4 broadcast address of a network.
#[must_use]
pub fn broadcast_address(network: IpNetwork) -> Option<Ipv4Addr> {
    let IpAddr::V4(base) = network.base_address else {
        return None;
    };
    let mask = prefix_mask_v4(network.prefix_length);
    Some(Ipv4Addr::from(u32::from(base) | !mask))
}

/// Checks network membership and maps IPv4-mapped IPv6 addresses to IPv4.
pub fn subnet_contains_address(network: IpNetwork, address: IpAddr) -> bool {
    let address = match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address @ IpAddr::V4(_) => address,
    };
    network.contains(address)
}

fn canonical_address(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(address) => {
            IpAddr::V4(Ipv4Addr::from(u32::from(address) & prefix_mask_v4(prefix)))
        }
        IpAddr::V6(address) => {
            IpAddr::V6(Ipv6Addr::from(u128::from(address) & prefix_mask_v6(prefix)))
        }
    }
}

const fn prefix_mask_v4(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

const fn prefix_mask_v6(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

const fn family_of(address: IpAddr) -> AddressFamily {
    match address {
        IpAddr::V4(_) => AddressFamily::Ipv4,
        IpAddr::V6(_) => AddressFamily::Ipv6,
    }
}

const fn address_family_enabled(address: IpAddr, ipv4_enabled: bool, ipv6_enabled: bool) -> bool {
    match address {
        IpAddr::V4(_) => ipv4_enabled || !ipv6_enabled,
        IpAddr::V6(_) => ipv6_enabled || !ipv4_enabled,
    }
}

fn parse_ip_literal(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    let value = if let Some(without_open) = value.strip_prefix('[') {
        let closing = without_open.find(']')?;
        let trailing = &without_open[closing + 1..];
        if !trailing.is_empty() && !trailing.strip_prefix(':').is_some_and(is_valid_port_string) {
            return None;
        }
        &without_open[..closing]
    } else {
        value
    };
    let without_scope = value.split_once('%').map_or(value, |(address, _)| address);
    without_scope.parse().ok()
}

fn host_and_optional_port(host: &str) -> Option<&str> {
    let colon_count = host.bytes().filter(|byte| *byte == b':').count();
    if colon_count == 0 {
        return Some(host);
    }
    if colon_count == 1 {
        let (name, port) = host.split_once(':')?;
        return is_valid_port_string(port).then_some(name);
    }
    None
}

fn is_valid_port_string(port: &str) -> bool {
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_valid_hostname(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty()
        || name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return false;
    }
    name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn classify_subnet_warning(value: &str) -> SubnetParseWarning {
    let trimmed = value.trim().strip_prefix('!').unwrap_or(value.trim());
    if trimmed.contains('/') && trimmed.contains(':') && !trimmed.contains("::") {
        SubnetParseWarning::Ipv6PrefixOnly(value.to_owned())
    } else {
        SubnetParseWarning::Invalid(value.to_owned())
    }
}

fn left_part(value: &str, delimiter: char) -> &str {
    value.split_once(delimiter).map_or(value, |(left, _)| left)
}
