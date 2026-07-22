mod common;

use std::str::FromStr;

use common::{GeneralCommandType, Text};
use jellyfin_extensions::json::{CommaDelimited, CommaDelimitedReadOnlyList};
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
    value: CommaDelimitedReadOnlyList<T>,
}

fn text_body(values: &[&str]) -> Body<Text> {
    Body {
        value: CommaDelimited(
            values
                .iter()
                .map(|value| Text((*value).to_owned()))
                .collect(),
        ),
    }
}

fn command_body() -> Body<GeneralCommandType> {
    Body {
        value: CommaDelimited(vec![
            GeneralCommandType::MoveUp,
            GeneralCommandType::MoveDown,
        ]),
    }
}

#[test]
fn deserialize_comma_delimited_string() {
    assert_eq!(
        serde_json::from_str::<Body<Text>>(r#"{ "Value": "a,b,c" }"#).unwrap(),
        text_body(&["a", "b", "c"])
    );
}

#[test]
fn deserialize_comma_delimited_string_trims_spaces() {
    assert_eq!(
        serde_json::from_str::<Body<Text>>(r#"{ "Value": "a, b, c" }"#).unwrap(),
        text_body(&["a", "b", "c"])
    );
}

#[test]
fn deserialize_comma_delimited_enum_string() {
    assert_eq!(
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": "MoveUp,MoveDown" }"#)
            .unwrap(),
        command_body()
    );
}

#[test]
fn deserialize_comma_delimited_enum_string_trims_spaces() {
    assert_eq!(
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": "MoveUp, MoveDown" }"#)
            .unwrap(),
        command_body()
    );
}

#[test]
fn deserialize_string_array() {
    assert_eq!(
        serde_json::from_str::<Body<Text>>(r#"{ "Value": ["a","b","c"] }"#).unwrap(),
        text_body(&["a", "b", "c"])
    );
}

#[test]
fn deserialize_enum_array() {
    assert_eq!(
        serde_json::from_str::<Body<GeneralCommandType>>(r#"{ "Value": ["MoveUp", "MoveDown"] }"#)
            .unwrap(),
        command_body()
    );
}

#[test]
fn serialize_read_only_command_list() {
    assert_eq!(
        serde_json::to_string(&command_body()).unwrap(),
        r#"{"Value":["MoveUp","MoveDown"]}"#
    );
}
