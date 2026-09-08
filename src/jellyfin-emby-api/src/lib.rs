//! Compatibility contract shared by Emby Android and iOS clients.
//!
//! The generated clients in `Emby.ApiClients` identify themselves with either
//! the `Emby` or `MediaBrowser` scheme. Deployed clients may use the root API,
//! Jellyfin's `/api` prefix, or Emby's `/emby` prefix.

use axum::Router;

/// Jellyfin's historical API base path.
pub const JELLYFIN_API_PREFIX: &str = "/api";

/// Emby's generated Android and iOS clients' API base path.
pub const EMBY_API_PREFIX: &str = "/emby";

/// Every supported API base path, including the unprefixed server API.
pub const API_PREFIXES: [&str; 3] = ["", JELLYFIN_API_PREFIX, EMBY_API_PREFIX];

/// Mounts one API router on every base path used by Jellyfin and Emby clients.
pub fn mount_routes<S>(base: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .nest(JELLYFIN_API_PREFIX, base.clone())
        .nest(EMBY_API_PREFIX, base.clone())
        .merge(base)
}

/// Returns whether an HTTP authorization scheme carries Emby-compatible client metadata.
#[must_use]
pub fn is_authorization_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("MediaBrowser") || scheme.eq_ignore_ascii_case("Emby")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_client_contract_keeps_emby_path_and_schemes() {
        assert_eq!(API_PREFIXES, ["", "/api", "/emby"]);
        assert!(is_authorization_scheme("Emby"));
        assert!(is_authorization_scheme("mediabrowser"));
        assert!(!is_authorization_scheme("Bearer"));
    }
}
