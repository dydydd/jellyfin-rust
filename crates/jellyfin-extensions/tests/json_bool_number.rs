use jellyfin_extensions::json::bool_number;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Value(#[serde(with = "bool_number")] bool);

macro_rules! deserialize_case {
    ($name:ident, $json:literal, $expected:literal) => {
        #[test]
        fn $name() {
            assert_eq!(
                serde_json::from_str::<Value>($json).unwrap(),
                Value($expected)
            );
        }
    };
}

deserialize_case!(deserialize_one_as_true, "1", true);
deserialize_case!(deserialize_zero_as_false, "0", false);
deserialize_case!(deserialize_two_as_true, "2", true);
deserialize_case!(deserialize_true, "true", true);
deserialize_case!(deserialize_false, "false", false);

#[test]
fn serialize_true_as_boolean() {
    assert_eq!(serde_json::to_string(&Value(true)).unwrap(), "true");
}

#[test]
fn serialize_false_as_boolean() {
    assert_eq!(serde_json::to_string(&Value(false)).unwrap(), "false");
}

#[test]
fn every_sampled_nonzero_i32_deserializes_as_true() {
    let boundary_values = [i32::MIN, -1_000_000, -1, 1, 1_000_000, i32::MAX];
    for input in boundary_values
        .into_iter()
        .chain((-1_000..=1_000).filter(|value| *value != 0))
    {
        assert_eq!(
            serde_json::from_str::<Value>(&input.to_string()).unwrap(),
            Value(true)
        );
    }
}
