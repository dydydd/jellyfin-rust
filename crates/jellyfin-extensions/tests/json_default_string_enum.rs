use jellyfin_extensions::json::{
    DefaultStringEnum, default_string_enum, nullable_default_string_enum,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum MediaStreamProtocol {
    #[default]
    Http,
    Hls,
}

impl DefaultStringEnum for MediaStreamProtocol {
    fn from_json_name(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("http") {
            Some(Self::Http)
        } else if value.eq_ignore_ascii_case("hls") {
            Some(Self::Hls)
        } else {
            None
        }
    }

    fn as_json_name(&self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Hls => "hls",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Direct(#[serde(with = "default_string_enum")] MediaStreamProtocol);

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct Body {
    #[serde(with = "default_string_enum")]
    enum_value: MediaStreamProtocol,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct NullableBody {
    #[serde(with = "nullable_default_string_enum")]
    enum_value: Option<MediaStreamProtocol>,
}

macro_rules! direct_case {
    ($name:ident, $json:literal, $expected:ident) => {
        #[test]
        fn $name() {
            assert_eq!(
                serde_json::from_str::<Direct>($json).unwrap(),
                Direct(MediaStreamProtocol::$expected)
            );
        }
    };
}

direct_case!(deserialize_direct_empty_as_default, r#""""#, Http);
direct_case!(
    deserialize_direct_http_case_insensitively,
    r#""Http""#,
    Http
);
direct_case!(deserialize_direct_hls_case_insensitively, r#""Hls""#, Hls);

macro_rules! body_case {
    ($name:ident, $json_value:literal, $expected:ident) => {
        #[test]
        fn $name() {
            let json = format!(r#"{{"EnumValue":{}}}"#, $json_value);
            assert_eq!(
                serde_json::from_str::<Body>(&json).unwrap(),
                Body {
                    enum_value: MediaStreamProtocol::$expected,
                }
            );
        }
    };
}

body_case!(deserialize_body_null_as_default, "null", Http);
body_case!(deserialize_body_empty_as_default, r#""""#, Http);
body_case!(deserialize_body_http, r#""Http""#, Http);
body_case!(deserialize_body_hls, r#""Hls""#, Hls);

macro_rules! nullable_case {
    ($name:ident, $json_value:literal, $expected:expr) => {
        #[test]
        fn $name() {
            let json = format!(r#"{{"EnumValue":{}}}"#, $json_value);
            assert_eq!(
                serde_json::from_str::<NullableBody>(&json).unwrap(),
                NullableBody {
                    enum_value: $expected,
                }
            );
        }
    };
}

nullable_case!(deserialize_nullable_null_as_none, "null", None);
nullable_case!(
    deserialize_nullable_empty_as_default,
    r#""""#,
    Some(MediaStreamProtocol::Http)
);
nullable_case!(
    deserialize_nullable_http,
    r#""Http""#,
    Some(MediaStreamProtocol::Http)
);
nullable_case!(
    deserialize_nullable_hls,
    r#""Hls""#,
    Some(MediaStreamProtocol::Hls)
);

macro_rules! roundtrip_case {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let input = $value;
            let json = serde_json::to_string(&input).unwrap();
            assert_eq!(serde_json::from_str::<Body>(&json).unwrap(), input);
        }
    };
}

roundtrip_case!(
    roundtrip_http,
    Body {
        enum_value: MediaStreamProtocol::Http
    }
);
roundtrip_case!(
    roundtrip_hls,
    Body {
        enum_value: MediaStreamProtocol::Hls
    }
);

macro_rules! nullable_roundtrip_case {
    ($name:ident, $value:expr) => {
        #[test]
        fn $name() {
            let input = $value;
            let json = serde_json::to_string(&input).unwrap();
            assert_eq!(serde_json::from_str::<NullableBody>(&json).unwrap(), input);
        }
    };
}

nullable_roundtrip_case!(
    roundtrip_nullable_http,
    NullableBody {
        enum_value: Some(MediaStreamProtocol::Http)
    }
);
nullable_roundtrip_case!(
    roundtrip_nullable_hls,
    NullableBody {
        enum_value: Some(MediaStreamProtocol::Hls)
    }
);
nullable_roundtrip_case!(roundtrip_nullable_none, NullableBody { enum_value: None });
