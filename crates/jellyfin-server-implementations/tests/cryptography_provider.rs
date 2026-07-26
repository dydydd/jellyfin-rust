use jellyfin_model::PasswordHash;
use jellyfin_server_implementations::{
    CryptographyError, CryptographyProvider, DEFAULT_HASH_METHOD, DEFAULT_SALT_LENGTH,
};

#[test]
fn create_password_hash_with_password_returns_hash_with_iterations() {
    let cryptography = CryptographyProvider::new();
    let hash = cryptography.create_password_hash("testpassword");

    assert_eq!(hash.id(), DEFAULT_HASH_METHOD);
    assert!(hash.parameters().contains_key("iterations"));
    assert!(!hash.salt().is_empty());
    assert!(!hash.hash().is_empty());
}

#[test]
fn verify_with_valid_and_wrong_password_matches_official_behavior() {
    let cryptography = CryptographyProvider::new();
    let hash = cryptography.create_password_hash("testpassword");

    assert!(cryptography.verify(&hash, "testpassword").unwrap());
    assert!(!cryptography.verify(&hash, "wrongpassword").unwrap());
}

#[test]
fn verify_pbkdf2_missing_iterations_reports_format_error() {
    let cryptography = CryptographyProvider::new();
    let hash = PasswordHash::parse(
        "$PBKDF2$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();

    assert!(matches!(
        cryptography.verify(&hash, "password"),
        Err(CryptographyError::MissingIterations(method)) if method == "PBKDF2"
    ));
}

#[test]
fn verify_pbkdf2_sha512_invalid_iterations_reports_format_error() {
    let cryptography = CryptographyProvider::new();
    let hash = PasswordHash::parse(
        "$PBKDF2-SHA512$iterations=notanumber$69F420$\
         62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();

    assert!(matches!(
        cryptography.verify(&hash, "password"),
        Err(CryptographyError::InvalidIterations { method, value })
            if method == "PBKDF2-SHA512" && value == "notanumber"
    ));
}

#[test]
fn verify_unsupported_hash_id_reports_not_supported() {
    let cryptography = CryptographyProvider::new();
    let hash = PasswordHash::parse(
        "$UNKNOWN$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();

    assert!(matches!(
        cryptography.verify(&hash, "password"),
        Err(CryptographyError::UnsupportedHashMethod(method)) if method == "UNKNOWN"
    ));
}

#[test]
fn generate_salt_returns_non_empty_default_length_array() {
    let salt = CryptographyProvider::new().generate_default_salt();

    assert_eq!(salt.len(), DEFAULT_SALT_LENGTH);
    assert!(salt.iter().any(|byte| *byte != 0));
}

#[test]
fn generate_salt_with_length_returns_array_of_specified_length() {
    let cryptography = CryptographyProvider::new();

    for length in [16, 32, 64] {
        assert_eq!(cryptography.generate_salt(length).len(), length);
    }
}
