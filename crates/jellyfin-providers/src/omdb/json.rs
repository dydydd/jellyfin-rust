use std::{error::Error, fmt};

use serde_json::{Map, Number, Value};

/// Converts `OMDb` JSON payloads without performing any network access.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonOmdbConverter;

impl JsonOmdbConverter {
    /// Deserializes a title, episode, or search-result response.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or an incompatible field type.
    pub fn deserialize_item(input: &str) -> Result<OmdbItem, OmdbJsonError> {
        let value = serde_json::from_str(input)?;
        OmdbItem::from_value(&value)
    }

    /// Deserializes a full-season response.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or an incompatible field type.
    pub fn deserialize_season(input: &str) -> Result<OmdbSeason, OmdbJsonError> {
        let value = serde_json::from_str(input)?;
        OmdbSeason::from_value(&value)
    }

    /// Serializes a title, episode, or search-result response.
    ///
    /// # Errors
    ///
    /// Returns an error if the response cannot be represented as JSON.
    pub fn serialize_item(item: &OmdbItem) -> Result<String, OmdbJsonError> {
        Ok(serde_json::to_string(&item.to_value())?)
    }

    /// Serializes a full-season response.
    ///
    /// # Errors
    ///
    /// Returns an error if the response cannot be represented as JSON.
    pub fn serialize_season(season: &OmdbSeason) -> Result<String, OmdbJsonError> {
        Ok(serde_json::to_string(&season.to_value())?)
    }

    /// Applies the official nullable-string conversion to a JSON value.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or a non-string value.
    pub fn deserialize_nullable_string(input: &str) -> Result<Option<String>, OmdbJsonError> {
        let value = serde_json::from_str(input)?;
        optional_string(&value, "value")
    }

    /// Applies the official nullable-integer conversion to a JSON value.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON or a value that cannot be converted to `i32`.
    pub fn deserialize_nullable_i32(input: &str) -> Result<Option<i32>, OmdbJsonError> {
        let value = serde_json::from_str(input)?;
        optional_i32(&value, "value")
    }
}

/// An `OMDb` title, episode, or search-result response.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OmdbItem {
    pub title: Option<String>,
    pub year: Option<String>,
    pub rated: Option<String>,
    pub released: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub runtime: Option<String>,
    pub genre: Option<String>,
    pub director: Option<String>,
    pub writer: Option<String>,
    pub actors: Option<String>,
    pub plot: Option<String>,
    pub language: Option<String>,
    pub country: Option<String>,
    pub awards: Option<String>,
    pub poster: Option<String>,
    pub ratings: Option<Vec<OmdbRating>>,
    pub metascore: Option<String>,
    pub imdb_rating: Option<String>,
    pub imdb_votes: Option<String>,
    pub imdb_id: Option<String>,
    pub series_id: Option<String>,
    pub media_type: Option<String>,
    pub dvd: Option<String>,
    pub box_office: Option<String>,
    pub production: Option<String>,
    pub website: Option<String>,
    pub response: Option<String>,
    pub error: Option<String>,
}

impl OmdbItem {
    #[must_use]
    pub fn production_year(&self) -> Option<i32> {
        parse_year(self.year.as_deref()?)
    }

    #[must_use]
    pub fn release_date(&self) -> Option<OmdbDate> {
        parse_omdb_date(self.released.as_deref()?)
    }

    #[must_use]
    pub fn dvd_release_date(&self) -> Option<OmdbDate> {
        parse_omdb_date(self.dvd.as_deref()?)
    }

    #[must_use]
    pub fn runtime_minutes(&self) -> Option<u32> {
        parse_runtime(self.runtime.as_deref()?)
    }

    #[must_use]
    pub fn rotten_tomatoes_score(&self) -> Option<f32> {
        let rating = self.ratings.as_ref()?.iter().find(|rating| {
            rating
                .source
                .as_deref()
                .is_some_and(|source| source.eq_ignore_ascii_case("Rotten Tomatoes"))
        })?;
        rating
            .value
            .as_deref()?
            .trim_end_matches('%')
            .trim()
            .parse()
            .ok()
    }

    #[must_use]
    pub fn imdb_score(&self) -> Option<f32> {
        let score = self.imdb_rating.as_deref()?.trim().parse().ok()?;
        (score >= 0.0).then_some(score)
    }

    #[must_use]
    pub fn metascore(&self) -> Option<f32> {
        self.metascore.as_deref()?.trim().parse().ok()
    }

    #[must_use]
    pub fn vote_count(&self) -> Option<u64> {
        self.imdb_votes
            .as_deref()?
            .replace(',', "")
            .trim()
            .parse()
            .ok()
    }

    fn from_value(value: &Value) -> Result<Self, OmdbJsonError> {
        let object = expect_object(value, "root")?;
        Ok(Self {
            title: field_string(object, "Title")?,
            year: field_string(object, "Year")?,
            rated: field_string(object, "Rated")?,
            released: field_string(object, "Released")?,
            season: field_i32(object, "Season")?,
            episode: field_i32(object, "Episode")?,
            runtime: field_string(object, "Runtime")?,
            genre: field_string_or_array(object, "Genre")?,
            director: field_string_or_array(object, "Director")?,
            writer: field_string_or_array(object, "Writer")?,
            actors: field_string_or_array(object, "Actors")?,
            plot: field_string(object, "Plot")?,
            language: field_string_or_array(object, "Language")?,
            country: field_string_or_array(object, "Country")?,
            awards: field_string(object, "Awards")?,
            poster: field_string(object, "Poster")?,
            ratings: field_ratings(object, "Ratings")?,
            metascore: field_string(object, "Metascore")?,
            imdb_rating: field_string(object, "imdbRating")?,
            imdb_votes: field_string(object, "imdbVotes")?,
            imdb_id: field_string(object, "imdbID")?,
            series_id: field_string(object, "seriesID")?,
            media_type: field_string(object, "Type")?,
            dvd: field_string(object, "DVD")?,
            box_office: field_string(object, "BoxOffice")?,
            production: field_string(object, "Production")?,
            website: field_string(object, "Website")?,
            response: field_string(object, "Response")?,
            error: field_string(object, "Error")?,
        })
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        insert_string(&mut object, "Title", self.title.as_deref());
        insert_string(&mut object, "Year", self.year.as_deref());
        insert_string(&mut object, "Rated", self.rated.as_deref());
        insert_string(&mut object, "Released", self.released.as_deref());
        insert_i32(&mut object, "Season", self.season);
        insert_i32(&mut object, "Episode", self.episode);
        insert_string(&mut object, "Runtime", self.runtime.as_deref());
        insert_string(&mut object, "Genre", self.genre.as_deref());
        insert_string(&mut object, "Director", self.director.as_deref());
        insert_string(&mut object, "Writer", self.writer.as_deref());
        insert_string(&mut object, "Actors", self.actors.as_deref());
        insert_string(&mut object, "Plot", self.plot.as_deref());
        insert_string(&mut object, "Language", self.language.as_deref());
        insert_string(&mut object, "Country", self.country.as_deref());
        insert_string(&mut object, "Awards", self.awards.as_deref());
        insert_string(&mut object, "Poster", self.poster.as_deref());
        object.insert(
            "Ratings".to_owned(),
            self.ratings.as_ref().map_or(Value::Null, |ratings| {
                Value::Array(ratings.iter().map(OmdbRating::to_value).collect())
            }),
        );
        insert_string(&mut object, "Metascore", self.metascore.as_deref());
        insert_string(&mut object, "imdbRating", self.imdb_rating.as_deref());
        insert_string(&mut object, "imdbVotes", self.imdb_votes.as_deref());
        insert_string(&mut object, "imdbID", self.imdb_id.as_deref());
        insert_string(&mut object, "seriesID", self.series_id.as_deref());
        insert_string(&mut object, "Type", self.media_type.as_deref());
        insert_string(&mut object, "DVD", self.dvd.as_deref());
        insert_string(&mut object, "BoxOffice", self.box_office.as_deref());
        insert_string(&mut object, "Production", self.production.as_deref());
        insert_string(&mut object, "Website", self.website.as_deref());
        insert_string(&mut object, "Response", self.response.as_deref());
        insert_string(&mut object, "Error", self.error.as_deref());
        Value::Object(object)
    }
}

/// A rating entry returned in an `OMDb` `Ratings` array.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OmdbRating {
    pub source: Option<String>,
    pub value: Option<String>,
}

impl OmdbRating {
    fn from_value(value: &Value) -> Result<Self, OmdbJsonError> {
        let object = expect_object(value, "Ratings[]")?;
        Ok(Self {
            source: field_string(object, "Source")?,
            value: field_string(object, "Value")?,
        })
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        insert_string(&mut object, "Source", self.source.as_deref());
        insert_string(&mut object, "Value", self.value.as_deref());
        Value::Object(object)
    }
}

/// A full-season response returned by `OMDb`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OmdbSeason {
    pub title: Option<String>,
    pub series_id: Option<String>,
    pub season: Option<i32>,
    pub total_seasons: Option<i32>,
    pub episodes: Option<Vec<OmdbItem>>,
    pub response: Option<String>,
    pub error: Option<String>,
}

impl OmdbSeason {
    fn from_value(value: &Value) -> Result<Self, OmdbJsonError> {
        let object = expect_object(value, "root")?;
        Ok(Self {
            title: field_string(object, "Title")?,
            series_id: field_string(object, "seriesID")?,
            season: field_i32(object, "Season")?,
            total_seasons: field_i32(object, "totalSeasons")?,
            episodes: field_items(object, "Episodes")?,
            response: field_string(object, "Response")?,
            error: field_string(object, "Error")?,
        })
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        insert_string(&mut object, "Title", self.title.as_deref());
        insert_string(&mut object, "seriesID", self.series_id.as_deref());
        insert_i32(&mut object, "Season", self.season);
        insert_i32(&mut object, "totalSeasons", self.total_seasons);
        object.insert(
            "Episodes".to_owned(),
            self.episodes.as_ref().map_or(Value::Null, |items| {
                Value::Array(items.iter().map(OmdbItem::to_value).collect())
            }),
        );
        insert_string(&mut object, "Response", self.response.as_deref());
        insert_string(&mut object, "Error", self.error.as_deref());
        Value::Object(object)
    }
}

/// Calendar date parsed from `OMDb`'s invariant-culture date strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OmdbDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// Error returned for malformed JSON or an incompatible `OMDb` field type.
#[derive(Debug)]
pub enum OmdbJsonError {
    Json(serde_json::Error),
    InvalidType {
        field: &'static str,
        expected: &'static str,
    },
    InvalidInteger {
        field: &'static str,
        value: String,
    },
}

impl fmt::Display for OmdbJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::InvalidType { field, expected } => {
                write!(formatter, "OMDb field {field} must be {expected}")
            }
            Self::InvalidInteger { field, value } => {
                write!(formatter, "OMDb field {field} is not a valid i32: {value}")
            }
        }
    }
}

impl Error for OmdbJsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::InvalidType { .. } | Self::InvalidInteger { .. } => None,
        }
    }
}

impl From<serde_json::Error> for OmdbJsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn expect_object<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a Map<String, Value>, OmdbJsonError> {
    value.as_object().ok_or(OmdbJsonError::InvalidType {
        field,
        expected: "an object",
    })
}

fn field_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, OmdbJsonError> {
    object
        .get(field)
        .map_or(Ok(None), |value| optional_string(value, field))
}

fn optional_string(value: &Value, field: &'static str) -> Result<Option<String>, OmdbJsonError> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) if value.eq_ignore_ascii_case("N/A") => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        _ => Err(OmdbJsonError::InvalidType {
            field,
            expected: "a string or null",
        }),
    }
}

fn field_string_or_array(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, OmdbJsonError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if !value.is_array() {
        return optional_string(value, field);
    }

    let values = value
        .as_array()
        .expect("array checked above")
        .iter()
        .map(|value| optional_string(value, field))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Ok((!values.is_empty()).then(|| values.join(", ")))
}

fn field_i32(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<i32>, OmdbJsonError> {
    object
        .get(field)
        .map_or(Ok(None), |value| optional_i32(value, field))
}

fn optional_i32(value: &Value, field: &'static str) -> Result<Option<i32>, OmdbJsonError> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) if value.eq_ignore_ascii_case("N/A") => Ok(None),
        Value::String(value) => {
            value
                .trim()
                .parse::<i32>()
                .map(Some)
                .map_err(|_| OmdbJsonError::InvalidInteger {
                    field,
                    value: value.clone(),
                })
        }
        Value::Number(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| OmdbJsonError::InvalidInteger {
                field,
                value: value.to_string(),
            }),
        _ => Err(OmdbJsonError::InvalidType {
            field,
            expected: "an integer, integer string, or null",
        }),
    }
}

fn field_ratings(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<Vec<OmdbRating>>, OmdbJsonError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(value) if value.eq_ignore_ascii_case("N/A") => Ok(None),
        Value::Array(values) => values
            .iter()
            .map(OmdbRating::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err(OmdbJsonError::InvalidType {
            field,
            expected: "an array, N/A, or null",
        }),
    }
}

fn field_items(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<Vec<OmdbItem>>, OmdbJsonError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(value) if value.eq_ignore_ascii_case("N/A") => Ok(None),
        Value::Array(values) => values
            .iter()
            .map(OmdbItem::from_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err(OmdbJsonError::InvalidType {
            field,
            expected: "an array, N/A, or null",
        }),
    }
}

fn insert_string(object: &mut Map<String, Value>, field: &str, value: Option<&str>) {
    object.insert(
        field.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
}

fn insert_i32(object: &mut Map<String, Value>, field: &str, value: Option<i32>) {
    object.insert(
        field.to_owned(),
        value.map_or(Value::Null, |value| Value::Number(Number::from(value))),
    );
}

fn parse_year(value: &str) -> Option<i32> {
    value.get(..4)?.trim().parse().ok()
}

fn parse_omdb_date(value: &str) -> Option<OmdbDate> {
    let mut parts = value.split_whitespace();
    let day = parts.next()?.parse::<u8>().ok()?;
    let month = match parts.next()?.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    Some(OmdbDate { year, month, day })
}

const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 31,
    }
}

const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn parse_runtime(value: &str) -> Option<u32> {
    let value = value.trim();
    let minutes = value.strip_suffix(" min")?.trim();
    minutes.parse().ok()
}
