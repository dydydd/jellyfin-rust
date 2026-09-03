use std::sync::LazyLock;

use regex::Regex;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BookFileNameParserResult {
    pub name: Option<String>,
    pub index: Option<i32>,
    pub parent_index: Option<i32>,
    pub year: Option<i32>,
    pub series_name: Option<String>,
}

pub struct BookFileNameParser;

impl BookFileNameParser {
    #[must_use]
    pub fn parse(name: &str) -> BookFileNameParserResult {
        Self::parse_optional(Some(name))
    }

    #[must_use]
    pub fn parse_optional(name: Option<&str>) -> BookFileNameParserResult {
        let Some(name) = name else {
            return BookFileNameParserResult::default();
        };

        for expression in NAME_MATCHES.iter() {
            let Some(captures) = expression.captures(name) else {
                continue;
            };

            let parsed_name = captures.name("name").map(|value| value.as_str().trim());
            let mut result = BookFileNameParserResult {
                name: parsed_name.map(str::to_owned),
                index: parse_capture(&captures, "index"),
                parent_index: None,
                year: parse_capture(&captures, "year"),
                series_name: captures
                    .name("series_name")
                    .map(|value| value.as_str().trim().to_owned()),
            };

            if let Some(parsed_name) = parsed_name
                && let Some(comic) = COMIC_EXPRESSION.captures(parsed_name)
            {
                result.parent_index = parse_capture(&comic, "volume");
                if let Some(chapter) = parse_capture(&comic, "chapter") {
                    result.index = Some(chapter);
                }
            }

            return result;
        }

        BookFileNameParserResult::default()
    }
}

fn parse_capture(captures: &regex::Captures<'_>, name: &str) -> Option<i32> {
    captures
        .name(name)
        .and_then(|value| value.as_str().parse().ok())
}

static NAME_MATCHES: LazyLock<[Regex; 5]> = LazyLock::new(|| {
    [
        Regex::new(
            r"^(?P<series_name>.+?)(?:\s\([0-9]{4}\))?\s#(?P<index>[0-9]+)(?:\.0)?(?:\s\(of\s[0-9]+\))?(?:\s\((?P<year>[0-9]{4})\))?$",
        )
        .expect("book series expression must be valid"),
        Regex::new(
            r"^(?P<name>.+?)\s\((?P<series_name>.+?),\s#(?P<index>[0-9]+)\)(?:\.0)?(?:\s\((?P<year>[0-9]{4})\))?$",
        )
        .expect("book named-series expression must be valid"),
        Regex::new(
            r"^(?P<index>[0-9]+)(?:\.0)?\s-\s(?P<name>.+?)(?:\s\((?P<year>[0-9]{4})\))?$",
        )
        .expect("book indexed-name expression must be valid"),
        Regex::new(r"(?P<name>.*)\((?P<year>[0-9]{4})\)")
            .expect("book name-year expression must be valid"),
        Regex::new(r"(?P<name>.*)").expect("book fallback expression must be valid"),
    ]
});

static COMIC_EXPRESSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<name>.+?)(?:\sv(?P<volume>[0-9]+))?(?:\sc(?P<chapter>[0-9]+))?$")
        .expect("comic expression must be valid")
});
