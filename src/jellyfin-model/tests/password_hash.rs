use indexmap::IndexMap;
use jellyfin_model::{PasswordHash, PasswordHashError};

const HASH_HEX: &str = "62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D";
const SALT_HEX: &str = "69F420";

#[test]
fn constructor_rejects_null_and_empty_ids() {
    assert_eq!(
        PasswordHash::try_new(None, Vec::new()),
        Err(PasswordHashError::MissingId)
    );
    assert_eq!(
        PasswordHash::new("", Vec::new()),
        Err(PasswordHashError::EmptyId)
    );
}

#[test]
fn parse_valid_official_matrix() {
    let cases = [
        ("$PBKDF2".to_owned(), expected(&[], &[], &[])),
        (
            "$PBKDF2$iterations=1000".to_owned(),
            expected(&[], &[], &[("iterations", "1000")]),
        ),
        (
            "$PBKDF2$iterations=1000,m=120".to_owned(),
            expected(&[], &[], &[("iterations", "1000"), ("m", "120")]),
        ),
        (
            format!("$PBKDF2${HASH_HEX}"),
            expected(&decode(HASH_HEX), &[], &[]),
        ),
        (
            format!("$PBKDF2${SALT_HEX}${HASH_HEX}"),
            expected(&decode(HASH_HEX), &decode(SALT_HEX), &[]),
        ),
        (
            format!("$PBKDF2$iterations=1000${HASH_HEX}"),
            expected(&decode(HASH_HEX), &[], &[("iterations", "1000")]),
        ),
        (
            format!("$PBKDF2$iterations=1000,m=120${HASH_HEX}"),
            expected(
                &decode(HASH_HEX),
                &[],
                &[("iterations", "1000"), ("m", "120")],
            ),
        ),
        (
            format!("$PBKDF2$iterations=1000,m=120${SALT_HEX}${HASH_HEX}"),
            expected(
                &decode(HASH_HEX),
                &decode(SALT_HEX),
                &[("iterations", "1000"), ("m", "120")],
            ),
        ),
    ];

    assert_eq!(cases.len(), 8);
    for (source, expected) in cases {
        let parsed = PasswordHash::parse(&source).unwrap();
        assert_eq!(parsed.id(), expected.id(), "{source}");
        assert_eq!(parsed.parameters(), expected.parameters(), "{source}");
        assert_eq!(parsed.salt(), expected.salt(), "{source}");
        assert_eq!(parsed.hash(), expected.hash(), "{source}");
        assert_eq!(parsed.to_string(), expected.to_string(), "{source}");
    }
}

#[test]
fn to_string_round_trips_official_matrix() {
    let cases = [
        "$PBKDF2".to_owned(),
        format!("$PBKDF2${HASH_HEX}"),
        format!("$PBKDF2${SALT_HEX}${HASH_HEX}"),
        format!("$PBKDF2$iterations=1000${HASH_HEX}"),
        format!("$PBKDF2$iterations=1000,m=120${HASH_HEX}"),
        format!("$PBKDF2$iterations=1000,m=120${SALT_HEX}${HASH_HEX}"),
        "$PBKDF2$iterations=1000,m=120".to_owned(),
    ];

    assert_eq!(cases.len(), 7);
    for source in cases {
        assert_eq!(PasswordHash::parse(&source).unwrap().to_string(), source);
    }
}

#[test]
fn parse_rejects_null_and_empty_inputs() {
    assert_eq!(
        PasswordHash::parse_optional(None),
        Err(PasswordHashError::EmptyInput)
    );
    assert_eq!(PasswordHash::parse(""), Err(PasswordHashError::EmptyInput));
}

#[test]
fn parse_rejects_official_invalid_format_matrix() {
    let cases = [
        "$",
        "$$",
        "PBKDF2$",
        "$PBKDF2$$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$iterations=1000$$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$iterations=1000$69F420$",
        "$PBKDF2$=$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$=1000$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$iterations=$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$iterations=1000$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D$",
        "$PBKDF2$iterations=1000$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D$",
        "$PBKDF2$iterations=1000$69F420$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D$anotherone",
        "$PBKDF2$iterations=1000$invalidstalt$62FBA410AFCA5B4475F35137AB2E8596B127E4D927BA23F6CC05C067E897042D",
        "$PBKDF2$iterations=1000$69F420$invalid hash",
        "$PBKDF2$69F420$",
    ];

    assert_eq!(cases.len(), 15);
    for source in cases {
        assert!(PasswordHash::parse(source).is_err(), "{source}");
    }
}

fn expected(hash: &[u8], salt: &[u8], parameters: &[(&str, &str)]) -> PasswordHash {
    let parameters = parameters
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<IndexMap<_, _>>();
    PasswordHash::with_parameters("PBKDF2", hash.to_vec(), salt.to_vec(), parameters).unwrap()
}

fn decode(value: &str) -> Vec<u8> {
    hex::decode(value).unwrap()
}
