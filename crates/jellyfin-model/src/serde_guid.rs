use serde::{Deserialize, Deserializer, Serializer};
use uuid::Uuid;

pub(crate) mod single {
    use super::*;

    pub fn serialize<S>(value: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&value.simple())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Uuid::parse_str(&value).map_err(serde::de::Error::custom)
    }
}

pub(crate) mod vec {
    use super::*;

    pub fn serialize<S>(values: &[Uuid], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;

        let mut sequence = serializer.serialize_seq(Some(values.len()))?;
        for value in values {
            sequence.serialize_element(&value.simple().to_string())?;
        }
        sequence.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<String>::deserialize(deserializer)?;
        values
            .into_iter()
            .map(|value| Uuid::parse_str(&value).map_err(serde::de::Error::custom))
            .collect()
    }
}

pub(crate) mod option {
    use super::*;

    pub fn serialize<S>(value: &Option<Uuid>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.collect_str(&value.simple()),
            None => serializer.serialize_none(),
        }
    }
}

pub(crate) mod option_vec {
    use super::*;

    pub fn serialize<S>(values: &Option<Vec<Uuid>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;

        match values {
            Some(values) => {
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&value.simple().to_string())?;
                }
                sequence.end()
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<Uuid>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Vec<String>>::deserialize(deserializer)?
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| Uuid::parse_str(&value).map_err(serde::de::Error::custom))
                    .collect()
            })
            .transpose()
    }
}
