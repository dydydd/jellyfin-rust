mod common;

use std::str::FromStr;

use common::{GeneralCommandType, Text};
use jellyfin_extensions::json::CommaDelimited;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    rename_all = "PascalCase",
    bound(
        serialize = "T: Serialize",
        deserialize = "T: Deserialize<'de> + FromStr"
    )
)]
struct Body<T> {
    value: Option<CommaDelimited<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ConcreteListBody {
    #[allow(dead_code)]
    value: Vec<String>,
}

fn text_values(values: &[&str]) -> Option<CommaDelimited<Text>> {
    Some(CommaDelimited(
        values
            .iter()
            .map(|value| Text((*value).to_owned()))
            .collect(),
    ))
}

fn command_values(values: &[GeneralCommandType]) -> Option<CommaDelimited<GeneralCommandType>> {
    Some(CommaDelimited(values.to_vec()))
}

#[test]
fn deserialize_string_null_successfully() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": null }"#).unwrap();
    assert_eq!(value, Body { value: None });
}

#[test]
fn deserialize_empty_string_as_empty_array() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": "" }"#).unwrap();
    assert_eq!(
        value,
        Body {
            value: text_values(&[])
        }
    );
}

#[test]
fn deserialize_empty_string_into_concrete_list_is_an_error() {
    assert!(serde_json::from_str::<ConcreteListBody>(r#"{ "Value": "" }"#).is_err());
}

#[test]
fn deserialize_empty_string_as_empty_read_only_list() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": "" }"#).unwrap();
    assert_eq!(
        value,
        Body {
            value: text_values(&[])
        }
    );
}

#[test]
fn deserialize_comma_delimited_string() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": "a,b,c" }"#).unwrap();
    assert_eq!(
        value,
        Body {
            value: text_values(&["a", "b", "c"])
        }
    );
}

#[test]
fn deserialize_comma_string_into_concrete_list_is_an_error() {
    assert!(serde_json::from_str::<ConcreteListBody>(r#"{ "Value": "a,b,c" }"#).is_err());
}

#[test]
fn deserialize_comma_delimited_string_trims_spaces() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": "a, b, c" }"#).unwrap();
    assert_eq!(
        value,
        Body {
            value: text_values(&["a", "b", "c"])
        }
    );
}

#[test]
fn deserialize_comma_delimited_enum_string() {
    let value =
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": "MoveUp,MoveDown" }"#)
            .unwrap();
    assert_eq!(
        value,
        Body {
            value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
        }
    );
}

#[test]
fn deserialize_comma_delimited_enum_string_ignores_empty_entry() {
    let value =
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": "MoveUp,,MoveDown" }"#)
            .unwrap();
    assert_eq!(
        value,
        Body {
            value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
        }
    );
}

#[test]
fn deserialize_comma_delimited_enum_string_ignores_invalid_entry() {
    let value = serde_json::from_str::<Body<GeneralCommandType>>(
        r#"{ "Value": "MoveUp,TotallyNotAValidCommand,MoveDown" }"#,
    )
    .unwrap();
    assert_eq!(
        value,
        Body {
            value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
        }
    );
}

#[test]
fn deserialize_comma_delimited_enum_string_trims_spaces() {
    let value =
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": "MoveUp, MoveDown" }"#)
            .unwrap();
    assert_eq!(
        value,
        Body {
            value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
        }
    );
}

#[test]
fn deserialize_string_array() {
    let value = serde_json::from_str::<Body<Text>>(r#"{ "Value": ["a","b","c"] }"#).unwrap();
    assert_eq!(
        value,
        Body {
            value: text_values(&["a", "b", "c"])
        }
    );
}

#[test]
fn deserialize_enum_array() {
    let value =
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": ["MoveUp", "MoveDown"] }"#)
            .unwrap();
    assert_eq!(
        value,
        Body {
            value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
        }
    );
}

fn command_body() -> Body<GeneralCommandType> {
    Body {
        value: command_values(&[GeneralCommandType::MoveUp, GeneralCommandType::MoveDown]),
    }
}

#[test]
fn serialize_read_only_command_array() {
    assert_eq!(
        serde_json::to_string(&command_body()).unwrap(),
        r#"{"Value":["MoveUp","MoveDown"]}"#
    );
}

#[test]
fn serialize_immutable_command_array() {
    assert_eq!(
        serde_json::to_string(&command_body()).unwrap(),
        r#"{"Value":["MoveUp","MoveDown"]}"#
    );
}

#[test]
fn serialize_command_list() {
    assert_eq!(
        serde_json::to_string(&command_body()).unwrap(),
        r#"{"Value":["MoveUp","MoveDown"]}"#
    );
}
