/// Jellyfin's network-facing server configuration.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct NetworkConfiguration {
    base_url: String,
    pub enable_https: bool,
    pub require_https: bool,
    pub certificate_path: String,
    pub certificate_password: String,
    pub internal_http_port: u16,
    pub internal_https_port: u16,
    pub public_http_port: u16,
    pub public_https_port: u16,
    pub auto_discovery: bool,
    #[serde(rename = "EnableIPv4", alias = "EnableIpv4")]
    pub enable_ipv4: bool,
    #[serde(rename = "EnableIPv6", alias = "EnableIpv6")]
    pub enable_ipv6: bool,
    pub enable_remote_access: bool,
    pub local_network_subnets: Vec<String>,
    pub local_network_addresses: Vec<String>,
    pub known_proxies: Vec<String>,
    pub ignore_virtual_interfaces: bool,
    pub virtual_interface_names: Vec<String>,
    pub enable_published_server_uri_by_request: bool,
    pub published_server_uri_by_subnet: Vec<String>,
    #[serde(rename = "RemoteIPFilter", alias = "RemoteIpFilter")]
    pub remote_ip_filter: Vec<String>,
    #[serde(
        rename = "IsRemoteIPFilterBlacklist",
        alias = "IsRemoteIpFilterBlacklist"
    )]
    pub is_remote_ip_filter_blacklist: bool,
}

impl NetworkConfiguration {
    pub const DEFAULT_HTTP_PORT: u16 = 8096;
    pub const DEFAULT_HTTPS_PORT: u16 = 8920;

    /// Returns the normalized URL prefix. A non-empty prefix starts with `/`
    /// and never ends with `/`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn set_base_url(&mut self, value: impl AsRef<str>) {
        let value = value.as_ref();
        if value.trim().is_empty() {
            self.base_url.clear();
            return;
        }

        let mut normalized = if value.starts_with('/') {
            value.to_owned()
        } else {
            format!("/{value}")
        };
        if normalized.ends_with('/') {
            normalized.pop();
        }
        self.base_url = normalized;
    }

    #[must_use]
    pub fn with_base_url(mut self, value: impl AsRef<str>) -> Self {
        self.set_base_url(value);
        self
    }
}

impl Default for NetworkConfiguration {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            enable_https: false,
            require_https: false,
            certificate_path: String::new(),
            certificate_password: String::new(),
            internal_http_port: Self::DEFAULT_HTTP_PORT,
            internal_https_port: Self::DEFAULT_HTTPS_PORT,
            public_http_port: Self::DEFAULT_HTTP_PORT,
            public_https_port: Self::DEFAULT_HTTPS_PORT,
            auto_discovery: true,
            enable_ipv4: true,
            enable_ipv6: false,
            enable_remote_access: true,
            local_network_subnets: Vec::new(),
            local_network_addresses: Vec::new(),
            known_proxies: Vec::new(),
            ignore_virtual_interfaces: true,
            virtual_interface_names: vec!["veth".to_owned()],
            enable_published_server_uri_by_request: false,
            published_server_uri_by_subnet: Vec::new(),
            remote_ip_filter: Vec::new(),
            is_remote_ip_filter_blacklist: false,
        }
    }
}
