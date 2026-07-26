use std::{fmt, marker::PhantomData, str::FromStr};

use jellyfin_data::ItemValueOrder;
use jellyfin_model::SortOrder;
use serde::{Deserializer, de::SeqAccess};

use crate::ApiError;

pub mod comma {
    use serde::Deserializer;

    /// Deserializes comma-delimited or repeated query values into a list.
    ///
    /// Empty and unparseable elements are omitted to match Jellyfin's model
    /// binder behavior.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying serde deserializer cannot read the
    /// query value or sequence.
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: std::str::FromStr,
    {
        super::deserialize_delimited::<D, T, ','>(deserializer)
    }
}

pub mod pipe {
    use serde::Deserializer;

    /// Deserializes pipe-delimited or repeated query values into a list.
    ///
    /// Empty and unparseable elements are omitted to match Jellyfin's model
    /// binder behavior.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying serde deserializer cannot read the
    /// query value or sequence.
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: std::str::FromStr,
    {
        super::deserialize_delimited::<D, T, '|'>(deserializer)
    }
}

pub(crate) fn item_value_order(sort_by: &[String]) -> Result<ItemValueOrder, ApiError> {
    let Some(order) = sort_by.first() else {
        return Ok(ItemValueOrder::CleanValue);
    };

    if order.eq_ignore_ascii_case("Default")
        || order.eq_ignore_ascii_case("SortName")
        || order.eq_ignore_ascii_case("Name")
    {
        Ok(ItemValueOrder::CleanValue)
    } else if order.eq_ignore_ascii_case("Random") {
        Ok(ItemValueOrder::Random)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

/// Pairs each sort field with its requested direction.
///
/// Jellyfin applies the first provided sort direction to every remaining field
/// when fewer sort directions than fields are specified. With no requested
/// directions, all fields sort ascending.
pub(crate) fn get_order_by<T>(
    sort_by: &[T],
    requested_sort_order: &[SortOrder],
) -> Vec<(T, SortOrder)>
where
    T: Clone,
{
    if sort_by.is_empty() {
        return Vec::new();
    }

    let default_order = requested_sort_order.first().copied().unwrap_or_default();
    sort_by
        .iter()
        .enumerate()
        .map(|(index, sort)| {
            (
                sort.clone(),
                requested_sort_order
                    .get(index)
                    .copied()
                    .unwrap_or(default_order),
            )
        })
        .collect()
}

pub(crate) fn parse_sort_order(order: &str) -> Result<SortOrder, ApiError> {
    if order.eq_ignore_ascii_case("Ascending") {
        Ok(SortOrder::Ascending)
    } else if order.eq_ignore_ascii_case("Descending") {
        Ok(SortOrder::Descending)
    } else {
        Err(ApiError::InvalidRequest)
    }
}

fn deserialize_delimited<'de, D, T, const DELIMITER: char>(
    deserializer: D,
) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
{
    deserializer.deserialize_any(DelimitedVisitor::<T, DELIMITER>(PhantomData))
}

struct DelimitedVisitor<T, const DELIMITER: char>(PhantomData<T>);

impl<'de, T, const DELIMITER: char> serde::de::Visitor<'de> for DelimitedVisitor<T, DELIMITER>
where
    T: FromStr,
{
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a string or repeated strings delimited by {DELIMITER:?}"
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(parse_values::<T, DELIMITER>(value))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_seq<A>(self, mut values: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut parsed = Vec::new();
        while let Some(value) = values.next_element::<String>()? {
            if let Some(value) = parse_value(&value) {
                parsed.push(value);
            }
        }
        Ok(parsed)
    }
}

fn parse_value<T>(value: &str) -> Option<T>
where
    T: FromStr,
{
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

fn parse_values<T, const DELIMITER: char>(value: &str) -> Vec<T>
where
    T: FromStr,
{
    value.split(DELIMITER).filter_map(parse_value).collect()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use axum::{Json, Router, body::Body, http::Request, routing::get};
    use axum_extra::extract::Query;
    use jellyfin_model::SortOrder;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{comma, get_order_by, parse_sort_order, pipe};

    #[derive(Debug, Deserialize)]
    struct CommaStrings {
        #[serde(default, deserialize_with = "comma::deserialize")]
        test: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct CommaIntegers {
        #[serde(default, deserialize_with = "comma::deserialize")]
        test: Vec<i32>,
    }

    #[derive(Debug, Deserialize)]
    struct CommaEnums {
        #[serde(default, deserialize_with = "comma::deserialize")]
        test: Vec<TestType>,
    }

    #[derive(Debug, Deserialize)]
    struct PipeStrings {
        #[serde(default, deserialize_with = "pipe::deserialize")]
        test: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct PipeIntegers {
        #[serde(default, deserialize_with = "pipe::deserialize")]
        test: Vec<i32>,
    }

    #[derive(Debug, Deserialize)]
    struct PipeEnums {
        #[serde(default, deserialize_with = "pipe::deserialize")]
        test: Vec<TestType>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestType {
        How,
        Much,
    }

    impl FromStr for TestType {
        type Err = ();

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "How" => Ok(Self::How),
                "Much" => Ok(Self::Much),
                _ => Err(()),
            }
        }
    }

    fn query<T>(value: &str) -> T
    where
        T: serde::de::DeserializeOwned,
    {
        let uri = format!("http://localhost/?{value}").parse().unwrap();
        Query::<T>::try_from_uri(&uri).unwrap().0
    }

    #[test]
    fn comma_binds_valid_string_array() {
        assert_eq!(query::<CommaStrings>("test=lol%2Cxd").test, ["lol", "xd"]);
    }

    #[test]
    fn comma_binds_valid_integer_array() {
        assert_eq!(query::<CommaIntegers>("test=42%2C0").test, [42, 0]);
    }

    #[test]
    fn comma_binds_valid_enum_array() {
        assert_eq!(
            query::<CommaEnums>("test=How%2CMuch").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn comma_ignores_empty_segments() {
        assert_eq!(
            query::<CommaEnums>("test=How%2C%2CMuch").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn comma_binds_repeated_keys() {
        assert_eq!(
            query::<CommaEnums>("test=How&test=Much").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn comma_does_not_resplit_repeated_values() {
        assert_eq!(
            query::<CommaStrings>("test=lol%2Cxd&test=separate").test,
            ["lol,xd", "separate"]
        );
    }

    #[test]
    fn comma_binds_empty_value_as_empty_array() {
        assert!(query::<CommaEnums>("test").test.is_empty());
    }

    #[test]
    fn comma_filters_all_invalid_enum_values() {
        assert!(
            query::<CommaEnums>("test=%F0%9F%94%A5%2C%F0%9F%98%A2")
                .test
                .is_empty()
        );
    }

    #[test]
    fn comma_keeps_valid_repeated_values_and_filters_invalid_ones() {
        assert_eq!(
            query::<CommaEnums>("test=How&test=%F0%9F%98%B1").test,
            [TestType::How]
        );
    }

    #[test]
    fn pipe_binds_valid_string_array() {
        assert_eq!(query::<PipeStrings>("test=lol%7Cxd").test, ["lol", "xd"]);
    }

    #[test]
    fn pipe_binds_valid_integer_array() {
        assert_eq!(query::<PipeIntegers>("test=42%7C0").test, [42, 0]);
    }

    #[test]
    fn pipe_binds_valid_enum_array() {
        assert_eq!(
            query::<PipeEnums>("test=How%7CMuch").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn pipe_ignores_empty_segments() {
        assert_eq!(
            query::<PipeEnums>("test=How%7C%7CMuch").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn pipe_binds_repeated_keys() {
        assert_eq!(
            query::<PipeEnums>("test=How&test=Much").test,
            [TestType::How, TestType::Much]
        );
    }

    #[test]
    fn get_order_by_matches_official_empty_case() {
        assert_eq!(
            get_order_by::<TestType>(&[], &[]),
            Vec::<(TestType, SortOrder)>::new()
        );
    }

    #[test]
    fn get_order_by_defaults_every_field_to_ascending() {
        assert_eq!(
            get_order_by(&[TestType::How, TestType::Much], &[]),
            [
                (TestType::How, SortOrder::Ascending),
                (TestType::Much, SortOrder::Ascending),
            ]
        );
    }

    #[test]
    fn get_order_by_reuses_first_requested_order_for_remaining_fields() {
        assert_eq!(
            get_order_by(&[TestType::How, TestType::Much], &[SortOrder::Descending],),
            [
                (TestType::How, SortOrder::Descending),
                (TestType::Much, SortOrder::Descending),
            ]
        );
    }

    #[test]
    fn parse_sort_order_accepts_official_names_case_insensitively() {
        assert_eq!(
            parse_sort_order("descending").unwrap(),
            SortOrder::Descending
        );
        assert_eq!(parse_sort_order("Ascending").unwrap(), SortOrder::Ascending);
        assert!(parse_sort_order("sideways").is_err());
    }

    #[test]
    fn pipe_does_not_resplit_repeated_values() {
        assert_eq!(
            query::<PipeStrings>("test=lol%7Cxd&test=separate").test,
            ["lol|xd", "separate"]
        );
    }

    #[test]
    fn pipe_binds_empty_value_as_empty_array() {
        assert!(query::<PipeEnums>("test").test.is_empty());
    }

    #[test]
    fn pipe_filters_all_invalid_enum_values() {
        assert!(
            query::<PipeEnums>("test=%F0%9F%94%A5%7C%F0%9F%98%A2")
                .test
                .is_empty()
        );
    }

    #[test]
    fn pipe_keeps_valid_repeated_values_and_filters_invalid_ones() {
        assert_eq!(
            query::<PipeEnums>("test=How&test=%F0%9F%98%B1").test,
            [TestType::How]
        );
    }

    #[derive(Debug, Deserialize)]
    struct CombinedQuery {
        #[serde(default, deserialize_with = "comma::deserialize")]
        comma: Vec<TestType>,
        #[serde(default, deserialize_with = "pipe::deserialize")]
        pipe: Vec<i32>,
    }

    async fn echo_query(Query(query): Query<CombinedQuery>) -> Json<Value> {
        Json(json!({
            "comma": query.comma.iter().map(|value| format!("{value:?}")).collect::<Vec<_>>(),
            "pipe": query.pipe,
        }))
    }

    #[tokio::test]
    async fn axum_extractor_binds_delimited_and_repeated_real_query_values() {
        let app = Router::new().route("/", get(echo_query));
        let response = app
            .oneshot(
                Request::get("/?comma=How&comma=Much&pipe=42%7Cinvalid%7C0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "comma": ["How", "Much"], "pipe": [42, 0] })
        );
    }
}
