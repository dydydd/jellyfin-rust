use jellyfin_extensions::json::guid;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Value(#[serde(with = "guid")] Uuid);

const SIMPLE: &str = "a852a27afe324084ae66db579ee3ee18";
const DASHED: &str = "e9b2dcaa-529c-426e-9433-5e9981f27f2e";
const SERIALIZED: &str = "531797e9945740e088bcb1d6d38752fa";

#[test]
fn deserialize_valid_simple_guid() {
    assert_eq!(
        serde_json::from_str::<Value>(&format!(r#""{SIMPLE}""#)).unwrap(),
        Value(Uuid::parse_str(SIMPLE).unwrap())
    );
}

#[test]
fn deserialize_valid_dashed_guid() {
    assert_eq!(
        serde_json::from_str::<Value>(&format!(r#""{DASHED}""#)).unwrap(),
        Value(Uuid::parse_str(DASHED).unwrap())
    );
}

#[test]
fn roundtrip_valid_guid() {
    let value = Value(Uuid::parse_str(SIMPLE).unwrap());
    assert_eq!(
        serde_json::from_str::<Value>(&serde_json::to_string(&value).unwrap()).unwrap(),
        value
    );
}

#[test]
fn deserialize_null_as_empty_guid() {
    assert_eq!(
        serde_json::from_str::<Value>("null").unwrap(),
        Value(Uuid::nil())
    );
}

#[test]
fn serialize_empty_guid_as_empty_guid() {
    assert_eq!(
        serde_json::to_string(&Value(Uuid::nil())).unwrap(),
        r#""00000000000000000000000000000000""#
    );
}

#[test]
fn serialize_valid_guid_without_dashes() {
    let guid = Uuid::parse_str(SERIALIZED).unwrap();
    assert_eq!(
        serde_json::to_string(&Value(guid)).unwrap(),
        format!(r#""{SERIALIZED}""#)
    );
}

#[test]
fn serialize_nullable_guid_uses_nonnullable_format() {
    let guid = Uuid::parse_str(SERIALIZED).unwrap();
    assert_eq!(
        serde_json::to_string(&Value(guid)).unwrap(),
        format!(r#""{SERIALIZED}""#)
    );
}
