use jellyfin_extensions::json::string;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Value(#[serde(with = "string")] String);

macro_rules! string_case {
    ($name:ident, $json:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                serde_json::from_str::<Value>($json).unwrap(),
                Value($expected.to_owned())
            );
        }
    };
}

string_case!(deserialize_string, r#""test""#, "test");
string_case!(deserialize_integer_as_string, "123", "123");
string_case!(deserialize_decimal_as_string, "123.45", "123.45");
string_case!(deserialize_true_as_string, "true", "true");
string_case!(deserialize_false_as_string, "false", "false");

#[test]
fn ordinary_integer_deserialization_is_unchanged() {
    assert_eq!(serde_json::from_str::<i32>("123").unwrap(), 123);
}
