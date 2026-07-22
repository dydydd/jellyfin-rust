use jellyfin_extensions::json::bool_string;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct Body {
    #[serde(with = "bool_string")]
    value: bool,
}

#[test]
fn deserialize_true_string() {
    assert_eq!(
        serde_json::from_str::<Body>(r#"{ "Value": "true" }"#).unwrap(),
        Body { value: true }
    );
}

#[test]
fn deserialize_false_string() {
    assert_eq!(
        serde_json::from_str::<Body>(r#"{ "Value": "false" }"#).unwrap(),
        Body { value: false }
    );
}

#[test]
fn serialize_true_as_boolean() {
    assert_eq!(
        serde_json::to_string(&Body { value: true }).unwrap(),
        r#"{"Value":true}"#
    );
}

#[test]
fn serialize_false_as_boolean() {
    assert_eq!(
        serde_json::to_string(&Body { value: false }).unwrap(),
        r#"{"Value":false}"#
    );
}
