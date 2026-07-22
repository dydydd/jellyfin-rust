//! Serde adapters for Jellyfin's backwards-compatible JSON representations.

use std::{fmt, marker::PhantomData, str::FromStr};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, SeqAccess, Visitor},
    ser::SerializeSeq,
};
use uuid::Uuid;

pub mod bool_number {
    use super::{BoolNumberVisitor, Deserializer, Serializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(BoolNumberVisitor)
    }

    pub fn serialize<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(*value)
    }
}

struct BoolNumberVisitor;

impl Visitor<'_> for BoolNumberVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a boolean or 32-bit integer")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(value)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        i32::try_from(value)
            .map(|value| value != 0)
            .map_err(E::custom)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        i32::try_from(value)
            .map(|value| value != 0)
            .map_err(E::custom)
    }
}

pub mod bool_string {
    use super::{BoolStringVisitor, Deserializer, Serializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(BoolStringVisitor)
    }

    pub fn serialize<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(*value)
    }
}

struct BoolStringVisitor;

impl Visitor<'_> for BoolStringVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a boolean or the string true or false")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(value)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.eq_ignore_ascii_case("true") {
            Ok(true)
        } else if value.eq_ignore_ascii_case("false") {
            Ok(false)
        } else {
            Err(E::invalid_value(de::Unexpected::Str(value), &self))
        }
    }
}

pub mod guid {
    use super::{Deserializer, GuidVisitor, Serializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(GuidVisitor)
    }

    pub fn serialize<S>(value: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&value.simple())
    }
}

struct GuidVisitor;

impl Visitor<'_> for GuidVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GUID string or null")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Uuid::parse_str(value).map_err(E::custom)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Uuid::nil())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Uuid::nil())
    }
}

pub mod nullable_guid {
    use super::{Deserializer, NullableGuidVisitor, Serializer};
    use uuid::Uuid;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NullableGuidVisitor)
    }

    pub fn serialize<S>(value: &Option<Uuid>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value.filter(|value| !value.is_nil()) {
            Some(value) => serializer.collect_str(&value.simple()),
            None => serializer.serialize_none(),
        }
    }
}

struct NullableGuidVisitor;

impl Visitor<'_> for NullableGuidVisitor {
    type Value = Option<Uuid>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GUID string or null")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Uuid::parse_str(value).map(Some).map_err(E::custom)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

pub mod string {
    use super::{Deserializer, JsonStringVisitor, Serializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonStringVisitor)
    }

    pub fn serialize<S>(value: &str, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(value)
    }
}

struct JsonStringVisitor;

impl Visitor<'_> for JsonStringVisitor {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string, number, or boolean")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(value)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(value.to_string())
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(value.to_string())
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(value.to_string())
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(value.to_string())
    }
}

pub trait DefaultStringEnum: Sized + Default {
    fn from_json_name(value: &str) -> Option<Self>;
    fn as_json_name(&self) -> &'static str;
}

pub mod default_string_enum {
    use super::{DefaultStringEnum, DefaultStringEnumVisitor, Deserializer, Serializer};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: DefaultStringEnum,
    {
        deserializer.deserialize_any(DefaultStringEnumVisitor::<T>::new())
    }

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: DefaultStringEnum,
    {
        serializer.serialize_str(value.as_json_name())
    }
}

struct DefaultStringEnumVisitor<T>(PhantomData<T>);

impl<T> DefaultStringEnumVisitor<T> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: DefaultStringEnum> Visitor<'_> for DefaultStringEnumVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string enum value, an empty string, or null")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.is_empty() {
            return Ok(T::default());
        }
        T::from_json_name(value)
            .ok_or_else(|| E::unknown_variant(value, &["a supported enum name"]))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(T::default())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(T::default())
    }
}

pub mod nullable_default_string_enum {
    use super::{DefaultStringEnum, Deserializer, NullableDefaultStringEnumVisitor, Serializer};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: DefaultStringEnum,
    {
        deserializer.deserialize_any(NullableDefaultStringEnumVisitor::<T>::new())
    }

    pub fn serialize<S, T>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: DefaultStringEnum,
    {
        match value {
            Some(value) => serializer.serialize_str(value.as_json_name()),
            None => serializer.serialize_none(),
        }
    }
}

struct NullableDefaultStringEnumVisitor<T>(PhantomData<T>);

impl<T> NullableDefaultStringEnumVisitor<T> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: DefaultStringEnum> Visitor<'_> for NullableDefaultStringEnumVisitor<T> {
    type Value = Option<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a nullable string enum value")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if value.is_empty() {
            return Ok(Some(T::default()));
        }
        T::from_json_name(value)
            .map(Some)
            .ok_or_else(|| E::unknown_variant(value, &["a supported enum name"]))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

pub trait FlagEnum: Copy {
    fn bits(self) -> u64;
    fn ordered_flags() -> &'static [(u64, &'static str)];
}

pub mod flags {
    use super::{FlagEnum, SerializeSeq, Serializer};
    use serde::ser::Error as _;

    pub fn serialize<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: FlagEnum + 'static,
    {
        let bits = value.bits();
        let mut sequence = serializer.serialize_seq(None)?;
        for &(flag, name) in T::ordered_flags() {
            if flag != 0 && bits & flag == flag {
                sequence.serialize_element(name)?;
            }
        }
        sequence.end().map_err(S::Error::custom)
    }
}

/// A collection that accepts either a JSON sequence or a comma-delimited string.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommaDelimited<T>(pub Vec<T>);

impl<T: Serialize> Serialize for CommaDelimited<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for CommaDelimited<T>
where
    T: Deserialize<'de> + FromStr,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(CommaDelimitedVisitor::<T>::new())
    }
}

struct CommaDelimitedVisitor<T>(PhantomData<T>);

impl<T> CommaDelimitedVisitor<T> {
    const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<'de, T> Visitor<'de> for CommaDelimitedVisitor<T>
where
    T: Deserialize<'de> + FromStr,
{
    type Value = CommaDelimited<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON sequence or comma-delimited string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(CommaDelimited(
            value
                .split(',')
                .filter(|entry| !entry.is_empty())
                .filter_map(|entry| entry.trim().parse().ok())
                .collect(),
        ))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or_default());
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(CommaDelimited(values))
    }
}

pub type CommaDelimitedReadOnlyList<T> = CommaDelimited<T>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonVersion {
    pub major: u32,
    pub minor: u32,
    pub build: Option<u32>,
    pub revision: Option<u32>,
}

impl JsonVersion {
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self {
            major,
            minor,
            build: None,
            revision: None,
        }
    }

    #[must_use]
    pub const fn with_build(mut self, build: u32) -> Self {
        self.build = Some(build);
        self
    }

    #[must_use]
    pub const fn with_revision(mut self, revision: u32) -> Self {
        self.revision = Some(revision);
        self
    }
}

impl fmt::Display for JsonVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)?;
        if let Some(build) = self.build {
            write!(formatter, ".{build}")?;
        }
        if let Some(revision) = self.revision {
            write!(formatter, ".{revision}")?;
        }
        Ok(())
    }
}

impl FromStr for JsonVersion {
    type Err = JsonVersionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let components = value
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| JsonVersionParseError)?;
        if !(2..=4).contains(&components.len()) {
            return Err(JsonVersionParseError);
        }
        Ok(Self {
            major: components[0],
            minor: components[1],
            build: components.get(2).copied(),
            revision: components.get(3).copied(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonVersionParseError;

impl fmt::Display for JsonVersionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("version must have two to four numeric components")
    }
}

impl std::error::Error for JsonVersionParseError {}

impl Serialize for JsonVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for JsonVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct JsonVersionVisitor;

        impl Visitor<'_> for JsonVersionVisitor {
            type Value = JsonVersion;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a dotted numeric version string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(JsonVersionVisitor)
    }
}
