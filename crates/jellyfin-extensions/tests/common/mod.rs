use std::{convert::Infallible, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GeneralCommandType {
    MoveUp,
    MoveDown,
}

impl FromStr for GeneralCommandType {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("MoveUp") {
            Ok(Self::MoveUp)
        } else if value.eq_ignore_ascii_case("MoveDown") {
            Ok(Self::MoveDown)
        } else {
            Err("unknown general command type")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text(pub String);

impl FromStr for Text {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(value.to_owned()))
    }
}

impl Serialize for Text {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}
