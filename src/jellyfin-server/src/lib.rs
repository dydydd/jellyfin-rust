//! Server-host integration behavior that is shared by the Jellyfin binary and tests.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderValue,
    middleware::Next,
    response::Response,
};

use jellyfin_networking::{
    HostResolver, IpNetwork, NetworkConfiguration, ParsedHost, try_parse_host,
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Forwarded request metadata accepted from an explicitly trusted proxy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ForwardedRequestInfo {
    pub proto: Option<String>,
    pub host: Option<String>,
}

/// Invalid or unsupported HTTP host configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostNetworkConfigurationError {
    InvalidCorsOrigin(String),
    MixedWildcardCorsOrigins,
    TlsUnsupported,
}

impl fmt::Display for HostNetworkConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCorsOrigin(origin) => write!(formatter, "invalid CORS origin: {origin}"),
            Self::MixedWildcardCorsOrigins => {
                formatter.write_str("CORS wildcard cannot be combined with explicit origins")
            }
            Self::TlsUnsupported => formatter.write_str(
                "native TLS is not supported by this server build; terminate TLS at a trusted reverse proxy or disable EnableHttps/RequireHttps",
            ),
        }
    }
}

impl std::error::Error for HostNetworkConfigurationError {}

/// Builds Jellyfin's CORS policy from the persisted server configuration.
///
/// Empty hosts and a single `*` allow any origin. Explicit origins permit
/// credentials, matching the official policy provider.
pub fn cors_layer(hosts: &[String]) -> Result<CorsLayer, HostNetworkConfigurationError> {
    let hosts = hosts
        .iter()
        .map(|host| host.trim())
        .filter(|host| !host.is_empty())
        .collect::<Vec<_>>();
    if hosts.is_empty() || hosts == ["*"] {
        return Ok(CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any));
    }
    if hosts.contains(&"*") {
        return Err(HostNetworkConfigurationError::MixedWildcardCorsOrigins);
    }
    let origins = hosts
        .into_iter()
        .map(|origin| {
            if !(origin.starts_with("http://") || origin.starts_with("https://")) {
                return Err(HostNetworkConfigurationError::InvalidCorsOrigin(
                    origin.to_owned(),
                ));
            }
            HeaderValue::from_str(origin)
                .map_err(|_| HostNetworkConfigurationError::InvalidCorsOrigin(origin.to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods(Any)
        .allow_headers(Any))
}

/// Rejects TLS settings that this Axum host cannot safely honor.
pub fn validate_tls_configuration(
    config: &NetworkConfiguration,
) -> Result<(), HostNetworkConfigurationError> {
    if config.enable_https || config.require_https {
        Err(HostNetworkConfigurationError::TlsUnsupported)
    } else {
        Ok(())
    }
}

/// Applies `X-Forwarded-*` only when the immediate TCP peer is trusted.
///
/// The effective client address is selected by walking `X-Forwarded-For`
/// right-to-left and stopping at the first untrusted hop. Malformed chains are
/// ignored in full so a bad element cannot be skipped to reach spoofed data.
pub async fn apply_forwarded_headers(
    State(network): State<Arc<jellyfin_networking::NetworkManager>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(peer) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .copied()
    else {
        return next.run(request).await;
    };
    if !network.is_known_proxy(peer.0.ip()) {
        return next.run(request).await;
    }

    if let Some(value) = request.headers().get("x-forwarded-for")
        && let Ok(value) = value.to_str()
        && let Some(client) = trusted_forwarded_client(value, peer.0.ip(), &network)
    {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(client, peer.0.port())));
    }

    let info = ForwardedRequestInfo {
        proto: forwarded_value(request.headers().get("x-forwarded-proto")),
        host: forwarded_value(request.headers().get("x-forwarded-host")),
    };
    if info.proto.is_some() || info.host.is_some() {
        request.extensions_mut().insert(info);
    }
    next.run(request).await
}

fn trusted_forwarded_client(
    value: &str,
    peer: IpAddr,
    network: &jellyfin_networking::NetworkManager,
) -> Option<IpAddr> {
    let chain = value
        .split(',')
        .map(|part| part.trim().parse::<IpAddr>())
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut effective = peer;
    for address in chain.into_iter().rev() {
        if !network.is_known_proxy(effective) {
            break;
        }
        effective = address;
    }
    Some(effective)
}

fn forwarded_value(value: Option<&HeaderValue>) -> Option<String> {
    value?
        .to_str()
        .ok()?
        .split(',')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

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
