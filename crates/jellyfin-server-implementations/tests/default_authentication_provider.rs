use chrono::Utc;
use jellyfin_data::entities::user;
use jellyfin_model::PasswordHash;
use jellyfin_server_implementations::{
    AuthenticationError, CryptographyError, CryptographyProvider, DEFAULT_HASH_METHOD,
    DEFAULT_ITERATIONS, DEFAULT_OUTPUT_LENGTH, DEFAULT_SALT_LENGTH, DefaultAuthenticationProvider,
};
use serde_json::json;
use uuid::Uuid;

#[test]
fn current_hash_round_trip_and_wrong_password_match_official_behavior() {
    let cryptography = CryptographyProvider::new();
    let hash = cryptography.create_password_hash("testpassword");

    assert_eq!(hash.id(), DEFAULT_HASH_METHOD);
    assert_eq!(
        hash.parameters().get("iterations").map(String::as_str),
        Some("210000")
    );
    assert_eq!(hash.salt().len(), DEFAULT_SALT_LENGTH);
    assert_eq!(hash.hash().len(), DEFAULT_OUTPUT_LENGTH);
    assert!(cryptography.verify(&hash, "testpassword").unwrap());
    assert!(!cryptography.verify(&hash, "wrongpassword").unwrap());
}

#[test]
fn fixed_sha1_and_sha512_vectors_are_jellyfin_compatible() {
    let cryptography = CryptographyProvider::new();
    let legacy = PasswordHash::parse(
        "$PBKDF2$iterations=1$73616C74$\
         0C60C80F961F0E71F3A9B524AF6012062FE037A6E0F0EB94FE8FC46BDC637164",
    )
    .unwrap();
    let current = PasswordHash::parse(
        "$PBKDF2-SHA512$iterations=1$73616C74$\
         867F70CF1ADE02CFF3752599A3A53DC4AF34C7A669815AE5D513554E1C8CF252\
         C02D470A285A0501BAD999BFE943C08F050235D7D68B1DA55E63F73B60A57FCE",
    )
    .unwrap();

    assert!(cryptography.verify(&legacy, "password").unwrap());
    assert!(cryptography.verify(&current, "password").unwrap());
    assert!(cryptography.needs_rehash(&legacy).unwrap());
    assert!(cryptography.needs_rehash(&current).unwrap());
}

#[test]
fn malformed_iterations_and_unknown_methods_are_reported() {
    let cryptography = CryptographyProvider::new();
    let missing = PasswordHash::parse(
        "$PBKDF2$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();
    let invalid = PasswordHash::parse(
        "$PBKDF2-SHA512$iterations=notanumber$69F420$\
         62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();
    let unknown = PasswordHash::parse(
        "$UNKNOWN$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
    )
    .unwrap();

    assert!(matches!(
        cryptography.verify(&missing, "password"),
        Err(CryptographyError::MissingIterations(method)) if method == "PBKDF2"
    ));
    assert!(matches!(
        cryptography.verify(&invalid, "password"),
        Err(CryptographyError::InvalidIterations { .. })
    ));
    assert!(matches!(
        cryptography.verify(&unknown, "password"),
        Err(CryptographyError::UnsupportedHashMethod(method)) if method == "UNKNOWN"
    ));
}

#[test]
fn passwordless_users_only_accept_empty_passwords() {
    let provider = DefaultAuthenticationProvider::new();
    let mut user = test_user(None);

    let result = provider.authenticate("Alice", "", Some(&mut user)).unwrap();
    assert_eq!(result.username, "Alice");
    assert!(!result.password_hash_upgraded);
    assert!(matches!(
        provider.authenticate("Alice", "not-empty", Some(&mut user)),
        Err(AuthenticationError::InvalidCredentials)
    ));
    assert!(matches!(
        provider.authenticate("Alice", "", None),
        Err(AuthenticationError::InvalidCredentials)
    ));

    user.password_hash = Some(String::new());
    assert!(provider.authenticate("Alice", "", Some(&mut user)).is_ok());
}

#[test]
fn change_password_sets_verifiable_hash_and_empty_password_clears_it() {
    let provider = DefaultAuthenticationProvider::new();
    let cryptography = CryptographyProvider::new();
    let mut user = test_user(None);

    provider.change_password(&mut user, "new password");
    let hash = PasswordHash::parse(user.password_hash.as_deref().unwrap()).unwrap();
    assert!(cryptography.verify(&hash, "new password").unwrap());
    assert!(!cryptography.verify(&hash, "wrong password").unwrap());
    let stored_hash = user.password_hash.clone();
    let result = provider
        .authenticate("Alice", "new password", Some(&mut user))
        .unwrap();
    assert!(!result.password_hash_upgraded);
    assert_eq!(user.password_hash, stored_hash);

    provider.change_password(&mut user, "   ");
    let whitespace_hash = PasswordHash::parse(user.password_hash.as_deref().unwrap()).unwrap();
    assert!(cryptography.verify(&whitespace_hash, "   ").unwrap());

    provider.change_password(&mut user, "");
    assert_eq!(user.password_hash, None);
}

#[test]
fn successful_obsolete_login_upgrades_hash_but_wrong_password_does_not() {
    let provider = DefaultAuthenticationProvider::new();
    let legacy = "$PBKDF2$iterations=1$73616C74$\
                  0C60C80F961F0E71F3A9B524AF6012062FE037A6E0F0EB94FE8FC46BDC637164";
    let mut user = test_user(Some(legacy));

    assert!(matches!(
        provider.authenticate("Alice", "wrong", Some(&mut user)),
        Err(AuthenticationError::InvalidCredentials)
    ));
    assert_eq!(user.password_hash.as_deref(), Some(legacy));

    let old_sha512 = "$PBKDF2-SHA512$iterations=1$73616C74$\
                      867F70CF1ADE02CFF3752599A3A53DC4AF34C7A669815AE5D513554E1C8CF252\
                      C02D470A285A0501BAD999BFE943C08F050235D7D68B1DA55E63F73B60A57FCE";
    for obsolete_hash in [legacy, old_sha512] {
        let mut user = test_user(Some(obsolete_hash));
        let result = provider
            .authenticate("Alice", "password", Some(&mut user))
            .unwrap();
        assert!(result.password_hash_upgraded);
        let upgraded = PasswordHash::parse(user.password_hash.as_deref().unwrap()).unwrap();
        assert_eq!(upgraded.id(), DEFAULT_HASH_METHOD);
        assert_eq!(
            upgraded
                .parameters()
                .get("iterations")
                .unwrap()
                .parse::<u32>(),
            Ok(DEFAULT_ITERATIONS)
        );
        assert!(
            CryptographyProvider::new()
                .verify(&upgraded, "password")
                .unwrap()
        );
    }
}

fn test_user(password_hash: Option<&str>) -> user::Model {
    let now = Utc::now();
    user::Model {
        id: Uuid::new_v4(),
        username: "Alice".to_owned(),
        normalized_username: "ALICE".to_owned(),
        password_hash: password_hash.map(str::to_owned),
        must_update_password: false,
        is_administrator: false,
        is_hidden: false,
        is_disabled: false,
        enable_auto_login: false,
        last_login_date: None,
        last_activity_date: None,
        policy: json!({}),
        preferences: json!({}),
        row_version: 1,
        created_at: now,
        updated_at: now,
    }
}
