use std::{error::Error, fmt, str::FromStr};

use indexmap::IndexMap;

/// A password hash encoded using Jellyfin's hexadecimal PHC string variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordHash {
    id: String,
    parameters: IndexMap<String, String>,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

impl PasswordHash {
    /// Creates a hash without a salt or additional parameters.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::EmptyId`] when `id` is empty.
    pub fn new(id: impl Into<String>, hash: Vec<u8>) -> Result<Self, PasswordHashError> {
        Self::with_parameters(id, hash, Vec::new(), IndexMap::new())
    }

    /// Creates a hash with a salt and no additional parameters.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::EmptyId`] when `id` is empty.
    pub fn with_salt(
        id: impl Into<String>,
        hash: Vec<u8>,
        salt: Vec<u8>,
    ) -> Result<Self, PasswordHashError> {
        Self::with_parameters(id, hash, salt, IndexMap::new())
    }

    /// Creates a hash with a salt and ordered hash-function parameters.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::EmptyId`] when `id` is empty.
    pub fn with_parameters(
        id: impl Into<String>,
        hash: Vec<u8>,
        salt: Vec<u8>,
        parameters: IndexMap<String, String>,
    ) -> Result<Self, PasswordHashError> {
        let id = id.into();
        if id.is_empty() {
            return Err(PasswordHashError::EmptyId);
        }

        Ok(Self {
            id,
            parameters,
            salt,
            hash,
        })
    }

    /// Nullable constructor equivalent used at C# compatibility boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::MissingId`] for `None`, or
    /// [`PasswordHashError::EmptyId`] for an empty identifier.
    pub fn try_new(id: Option<&str>, hash: Vec<u8>) -> Result<Self, PasswordHashError> {
        let id = id.ok_or(PasswordHashError::MissingId)?;
        Self::new(id, hash)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn parameters(&self) -> &IndexMap<String, String> {
        &self.parameters
    }

    #[must_use]
    pub fn salt(&self) -> &[u8] {
        &self.salt
    }

    #[must_use]
    pub fn hash(&self) -> &[u8] {
        &self.hash
    }

    /// Parses Jellyfin's hexadecimal PHC string variant.
    ///
    /// # Errors
    ///
    /// Returns a [`PasswordHashError`] when the input is empty or malformed.
    pub fn parse(value: &str) -> Result<Self, PasswordHashError> {
        Self::parse_optional(Some(value))
    }

    /// Nullable parser equivalent used at C# compatibility boundaries.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordHashError::EmptyInput`] for `None` or an empty value,
    /// and another [`PasswordHashError`] when the value is malformed.
    pub fn parse_optional(value: Option<&str>) -> Result<Self, PasswordHashError> {
        let value = value.ok_or(PasswordHashError::EmptyInput)?;
        if value.is_empty() {
            return Err(PasswordHashError::EmptyInput);
        }
        let Some(mut remaining) = value.strip_prefix('$') else {
            return Err(PasswordHashError::MissingPrefix);
        };

        if remaining.is_empty() || remaining.starts_with('$') {
            return Err(PasswordHashError::InvalidId);
        }
        let Some(id_end) = remaining.find('$') else {
            return Self::new(remaining, Vec::new());
        };
        let id = &remaining[..id_end];
        remaining = &remaining[id_end + 1..];

        let mut parameters = IndexMap::new();
        let mut next_segment = remaining.find('$');
        let parameter_candidate = next_segment.map_or(remaining, |end| &remaining[..end]);
        if parameter_candidate.contains('=') {
            parse_parameters(parameter_candidate, &mut parameters)?;
            let Some(end) = next_segment else {
                return Self::with_parameters(id, Vec::new(), Vec::new(), parameters);
            };
            remaining = &remaining[end + 1..];
            next_segment = remaining.find('$');
        }

        if next_segment == Some(0) {
            return Err(PasswordHashError::EmptySegment);
        }

        let (salt, hash) = match next_segment {
            None => (
                Vec::new(),
                decode_hex(remaining, PasswordHashSegment::Hash)?,
            ),
            Some(salt_end) => {
                let salt = decode_hex(&remaining[..salt_end], PasswordHashSegment::Salt)?;
                let hash_string = &remaining[salt_end + 1..];
                if hash_string.contains('$') {
                    return Err(PasswordHashError::TooManySegments);
                }
                if hash_string.is_empty() {
                    return Err(PasswordHashError::EmptyHash);
                }
                let hash = decode_hex(hash_string, PasswordHashSegment::Hash)?;
                (salt, hash)
            }
        };

        Self::with_parameters(id, hash, salt, parameters)
    }
}

impl FromStr for PasswordHash {
    type Err = PasswordHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for PasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "${}", self.id)?;
        if !self.parameters.is_empty() {
            formatter.write_str("$")?;
            for (index, (key, value)) in self.parameters.iter().enumerate() {
                if index != 0 {
                    formatter.write_str(",")?;
                }
                write!(formatter, "{key}={value}")?;
            }
        }
        if !self.salt.is_empty() {
            write!(formatter, "${}", hex::encode_upper(&self.salt))?;
        }
        if !self.hash.is_empty() {
            write!(formatter, "${}", hex::encode_upper(&self.hash))?;
        }
        Ok(())
    }
}

fn parse_parameters(
    mut value: &str,
    parameters: &mut IndexMap<String, String>,
) -> Result<(), PasswordHashError> {
    while !value.is_empty() {
        let (parameter, rest) = value
            .split_once(',')
            .map_or((value, ""), |(parameter, rest)| (parameter, rest));
        value = rest;

        let Some((key, parameter_value)) = parameter.split_once('=') else {
            return Err(PasswordHashError::MalformedParameter);
        };
        if key.is_empty() || parameter_value.is_empty() {
            return Err(PasswordHashError::MalformedParameter);
        }
        if parameters
            .insert(key.to_owned(), parameter_value.to_owned())
            .is_some()
        {
            return Err(PasswordHashError::DuplicateParameter(key.to_owned()));
        }
    }
    Ok(())
}

fn decode_hex(value: &str, segment: PasswordHashSegment) -> Result<Vec<u8>, PasswordHashError> {
    hex::decode(value).map_err(|_| PasswordHashError::InvalidHex(segment))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordHashSegment {
    Salt,
    Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordHashError {
    MissingId,
    EmptyId,
    EmptyInput,
    MissingPrefix,
    InvalidId,
    EmptySegment,
    MalformedParameter,
    DuplicateParameter(String),
    InvalidHex(PasswordHashSegment),
    TooManySegments,
    EmptyHash,
}

impl fmt::Display for PasswordHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingId => formatter.write_str("password hash id is missing"),
            Self::EmptyId => formatter.write_str("password hash id cannot be empty"),
            Self::EmptyInput => formatter.write_str("password hash string cannot be empty"),
            Self::MissingPrefix => formatter.write_str("password hash string must start with '$'"),
            Self::InvalidId => formatter.write_str("password hash string must contain a valid id"),
            Self::EmptySegment => {
                formatter.write_str("password hash string contains an empty segment")
            }
            Self::MalformedParameter => formatter.write_str("malformed password hash parameter"),
            Self::DuplicateParameter(key) => {
                write!(formatter, "duplicate password hash parameter: {key}")
            }
            Self::InvalidHex(PasswordHashSegment::Salt) => {
                formatter.write_str("password hash salt is not valid hexadecimal")
            }
            Self::InvalidHex(PasswordHashSegment::Hash) => {
                formatter.write_str("password hash is not valid hexadecimal")
            }
            Self::TooManySegments => {
                formatter.write_str("password hash string contains too many segments")
            }
            Self::EmptyHash => formatter.write_str("password hash segment is empty"),
        }
    }
}

impl Error for PasswordHashError {}
