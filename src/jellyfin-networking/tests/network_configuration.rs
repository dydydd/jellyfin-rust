use jellyfin_networking::NetworkConfiguration;

#[test]
fn base_url_returns_normalized_official_matrix() {
    for (expected, input) in [
        ("", ""),
        ("/Test", "/Test"),
        ("/Test", "Test"),
        ("/Test", "Test/"),
        ("/Test", "/Test/"),
        ("/Test/2", "/Test/2"),
        ("/Test/2", "Test/2"),
        ("/Test/2", "Test/2/"),
        ("/Test/2", "/Test/2/"),
    ] {
        let config = NetworkConfiguration::default().with_base_url(input);
        assert_eq!(config.base_url(), expected, "input={input:?}");
    }
}

#[test]
fn defaults_match_the_official_configuration_contract() {
    let config = NetworkConfiguration::default();
    assert_eq!(config.internal_http_port, 8096);
    assert_eq!(config.internal_https_port, 8920);
    assert_eq!(config.public_http_port, 8096);
    assert_eq!(config.public_https_port, 8920);
    assert!(config.enable_ipv4);
    assert!(!config.enable_ipv6);
    assert!(config.enable_remote_access);
    assert!(config.auto_discovery);
    assert!(config.ignore_virtual_interfaces);
    assert_eq!(config.virtual_interface_names, ["veth"]);
}
