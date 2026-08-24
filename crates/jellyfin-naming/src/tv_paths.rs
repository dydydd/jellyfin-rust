use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::{NamingOptions, ProviderIdMap, provider_ids};

const SEASON_KEYWORDS: &[&str] = &[
    "시즌",
    "シーズン",
    "сезон",
    "season",
    "sæson",
    "saison",
    "staffel",
    "series",
    "stagione",
    "säsong",
    "seizoen",
    "seasong",
    "sezon",
    "sezona",
    "sezóna",
    "sezonul",
    "série",
    "séria",
    "serie",
    "seria",
    "temporada",
    "kausi",
];

static SEASON_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"s([0-9]{1,4})(?:[._\-\[\]\s]|$)")
        .case_insensitive(true)
        .build()
        .expect("season prefix expression must be valid")
});

static CLEAN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ ._\-\[\]]").expect("season clean-name expression must be valid")
});

static SERIES_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"((?P<a>[^\._]{2,})[\._]*)|([\._](?P<b>[^\._]{2,}))")
        .expect("series-name expression must be valid")
});

static TITLE_WITH_YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<title>.+?)\s*\((?P<year>[0-9]{4})\)")
        .expect("series title-with-year expression must be valid")
});

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeasonPathParserResult {
    pub season_number: Option<i32>,
    pub success: bool,
    pub is_season_folder: bool,
    pub provider_ids: ProviderIdMap,
}

pub struct SeasonPathParser;

impl SeasonPathParser {
    #[must_use]
    pub fn parse(
        path: &str,
        parent_path: Option<&str>,
        support_special_aliases: bool,
        support_numeric_season_folders: bool,
    ) -> SeasonPathParserResult {
        let parent_name = parent_path.map(file_name);
        let (season_number, is_season_folder) = parse_season_number(
            path,
            parent_name,
            support_special_aliases,
            support_numeric_season_folders,
        );
        SeasonPathParserResult {
            season_number,
            success: season_number.is_some(),
            is_season_folder: season_number.is_some() && is_season_folder,
            provider_ids: provider_ids::from_path(
                file_name(path),
                &[
                    ("Tvdb", "tvdbid"),
                    ("TvMaze", "tvmazeid"),
                    ("Tmdb", "tmdbid"),
                ],
            ),
        }
    }
}

fn parse_season_number(
    path: &str,
    parent_name: Option<&str>,
    support_special_aliases: bool,
    support_numeric_season_folders: bool,
) -> (Option<i32>, bool) {
    let name = file_name(path);
    if let Some(captures) = SEASON_PREFIX.captures(name)
        && let Some(number) = capture_i32(&captures, 1)
    {
        return (Some(number), true);
    }

    let mut cleaned = CLEAN_NAME.replace_all(name, "").into_owned();
    if let Some(parent_name) = parent_name {
        let parent = CLEAN_NAME.replace_all(parent_name, "");
        if !parent.is_empty() {
            cleaned = remove_case_insensitive(&cleaned, &parent);
        }
    }

    if support_special_aliases
        && (cleaned.eq_ignore_ascii_case("specials") || cleaned.eq_ignore_ascii_case("extras"))
    {
        return (Some(0), true);
    }
    if support_numeric_season_folders && let Ok(number) = cleaned.parse() {
        return (Some(number), true);
    }

    let mixed_library = !support_numeric_season_folders && !support_special_aliases;
    let lowered = cleaned.to_lowercase();
    let original_contains_keyword = contains_season_keyword(name);

    if let Some(number) = parse_number_before_keyword(&lowered) {
        if mixed_library && !original_contains_keyword {
            return (None, false);
        }
        return (Some(number), true);
    }
    if let Some(number) = parse_number_after_keyword(&lowered) {
        if mixed_library && !original_contains_keyword {
            return (None, false);
        }
        return (Some(number), true);
    }
    (None, false)
}

fn parse_number_before_keyword(value: &str) -> Option<i32> {
    let digit_end = value.find(|character: char| !character.is_ascii_digit())?;
    if digit_end == 0 {
        return None;
    }
    let suffix = &value[digit_end..];
    let direct_match = SEASON_KEYWORDS
        .iter()
        .any(|keyword| suffix.starts_with(keyword));
    let ordinal_match = ["st", "nd", "rd", "th"].iter().any(|ordinal| {
        suffix.strip_prefix(ordinal).is_some_and(|rest| {
            SEASON_KEYWORDS
                .iter()
                .any(|keyword| rest.starts_with(keyword))
        })
    });
    if direct_match || ordinal_match {
        value[..digit_end].parse().ok()
    } else {
        None
    }
}

fn parse_number_after_keyword(value: &str) -> Option<i32> {
    let remainder = SEASON_KEYWORDS
        .iter()
        .filter_map(|keyword| value.strip_prefix(keyword))
        .filter(|remainder| remainder.starts_with(|character: char| character.is_ascii_digit()))
        .min_by_key(|remainder| remainder.len())?;
    let digit_count = remainder.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let digits = &remainder[..digit_count];
    let quality_digits = remainder
        .as_bytes()
        .get(digit_count)
        .is_some_and(|value| matches!(value, b'p' | b'P'))
        .then(|| {
            (3..=4)
                .rev()
                .find(|length| digits.len() > *length)
                .map(|length| digits.len() - length)
        })
        .flatten();
    let season_end = quality_digits.unwrap_or(digit_count);
    digits[..season_end].parse().ok()
}

fn contains_season_keyword(value: &str) -> bool {
    let lowered = value.to_lowercase();
    SEASON_KEYWORDS
        .iter()
        .any(|keyword| lowered.contains(keyword))
}

fn remove_case_insensitive(value: &str, needle: &str) -> String {
    let mut result = value.to_owned();
    while let Some((start, end)) = find_case_insensitive(&result, needle) {
        result.replace_range(start..end, "");
    }
    result
}

fn find_case_insensitive(value: &str, needle: &str) -> Option<(usize, usize)> {
    let needle = needle.to_lowercase();
    for (start, _) in value.char_indices() {
        let mut candidate = String::new();
        for (offset, character) in value[start..].char_indices() {
            candidate.extend(character.to_lowercase());
            let end = start + offset + character.len_utf8();
            if candidate == needle {
                return Some((start, end));
            }
            if candidate.len() >= needle.len() {
                break;
            }
        }
    }
    None
}

fn capture_i32(captures: &regex::Captures<'_>, index: usize) -> Option<i32> {
    captures.get(index)?.as_str().parse().ok()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeriesPathParserResult {
    pub series_name: Option<String>,
    pub success: bool,
}

pub struct SeriesPathParser;

impl SeriesPathParser {
    #[must_use]
    pub fn parse(options: &NamingOptions, path: &str) -> SeriesPathParserResult {
        for expression in &options.episode_expressions {
            if !expression.is_named {
                continue;
            }
            let Some(captures) = expression.regex().captures(path).ok().flatten() else {
                continue;
            };
            let Some(series) = captures.name("seriesname") else {
                continue;
            };
            if series.as_str().is_empty() || captures.name("seasonnumber").is_none() {
                continue;
            }
            return SeriesPathParserResult {
                series_name: Some(
                    series
                        .as_str()
                        .trim_matches([' ', '_', '.', '-'])
                        .to_owned(),
                ),
                success: true,
            };
        }
        SeriesPathParserResult::default()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SeriesInfo {
    pub path: String,
    pub name: Option<String>,
    pub year: Option<i32>,
    pub provider_ids: ProviderIdMap,
}

pub struct SeriesResolver;

impl SeriesResolver {
    #[must_use]
    pub fn resolve(options: &NamingOptions, path: &str) -> SeriesInfo {
        let mut name = file_name(path).to_owned();
        let provider_ids = provider_ids::from_path(
            &name,
            &[
                ("Imdb", "imdbid"),
                ("Tvdb", "tvdbid"),
                ("TvMaze", "tvmazeid"),
                ("Tmdb", "tmdbid"),
                ("AniDB", "anidbid"),
                ("AniList", "anilistid"),
                ("AniSearch", "anisearchid"),
            ],
        );
        if let Some(captures) = TITLE_WITH_YEAR.captures(&name)
            && let Some(title) = captures.name("title")
        {
            return SeriesInfo {
                path: path.to_owned(),
                name: Some(title.as_str().trim().to_owned()),
                year: captures
                    .name("year")
                    .and_then(|value| value.as_str().parse().ok()),
                provider_ids,
            };
        }

        let parsed = SeriesPathParser::parse(options, path);
        if parsed.success
            && let Some(series_name) = parsed.series_name
        {
            name = series_name;
        }
        name = SERIES_NAME
            .replace_all(&name, "${a} ${b}")
            .trim()
            .to_owned();
        SeriesInfo {
            path: path.to_owned(),
            name: Some(name),
            year: None,
            provider_ids,
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}
