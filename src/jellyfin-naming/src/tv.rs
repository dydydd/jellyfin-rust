use fancy_regex::{Regex, RegexBuilder};

use crate::{
    NamingOptions, ProviderIdMap, provider_ids,
    video::{Format3dParser, StubResolver},
};

#[derive(Clone, Debug)]
pub struct EpisodeExpression {
    expression: String,
    regex: Regex,
    pub is_by_date: bool,
    pub is_optimistic: bool,
    pub is_named: bool,
    pub supports_absolute_episode_numbers: bool,
}

impl EpisodeExpression {
    fn named(expression: &str) -> Self {
        let mut value = Self::built_in(expression, false);
        value.is_named = true;
        value
    }

    fn positional(expression: &str) -> Self {
        Self::built_in(expression, false)
    }

    /// Creates a configurable episode expression and compiles its regex.
    #[allow(clippy::result_large_err)]
    pub fn try_new(
        expression: impl Into<String>,
        by_date: bool,
    ) -> Result<Self, fancy_regex::Error> {
        let expression = expression.into();
        let regex = compile_expression(&expression)?;
        Ok(Self {
            expression,
            regex,
            is_by_date: by_date,
            is_optimistic: false,
            is_named: false,
            supports_absolute_episode_numbers: true,
        })
    }

    #[must_use]
    pub fn expression(&self) -> &str {
        &self.expression
    }

    #[must_use]
    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    /// Recompiles and installs a user-supplied expression atomically.
    ///
    /// If compilation fails, both the expression and compiled regex remain
    /// unchanged.
    #[allow(clippy::result_large_err)]
    pub fn set_expression(
        &mut self,
        expression: impl Into<String>,
    ) -> Result<(), fancy_regex::Error> {
        let expression = expression.into();
        let regex = compile_expression(&expression)?;
        self.expression = expression;
        self.regex = regex;
        Ok(())
    }

    fn built_in(expression: &str, by_date: bool) -> Self {
        Self::try_new(expression, by_date).expect("built-in episode expression must be valid")
    }

    fn by_date(expression: &str) -> Self {
        let mut value = Self::built_in(expression, true);
        value.is_named = true;
        value
    }

    fn optimistic(mut self) -> Self {
        self.is_optimistic = true;
        self
    }

    fn without_absolute_numbers(mut self) -> Self {
        self.supports_absolute_episode_numbers = false;
        self
    }
}

#[allow(clippy::result_large_err)]
fn compile_expression(expression: &str) -> Result<Regex, fancy_regex::Error> {
    RegexBuilder::new(expression).case_insensitive(true).build()
}

pub(crate) fn default_episode_expressions() -> Vec<EpisodeExpression> {
    vec![
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![Ss]([0-9]+)[\[\] ._-]*[Ee]([0-9]+))[^\\/])*)?[Ss](?P<seasonnumber>[0-9]+)[\[\] ._-]*[Ee](?P<epnumber>[0-9]+)([^\\/]*)$",
        ),
        EpisodeExpression::positional(r".*?[\._ -]()[Ee][Pp]_?([0-9]+)([^\\/]*)$"),
        EpisodeExpression::positional(r"(?:^|[\\/._ -])()\.?[Ee]([0-9]+)\.([^\\/]*)$"),
        EpisodeExpression::by_date(
            r"(?P<year>[0-9]{4})[._ -](?P<month>[0-9]{2})[._ -](?P<day>[0-9]{2})",
        ),
        EpisodeExpression::by_date(
            r"(?P<day>[0-9]{2})[._ -](?P<month>[0-9]{2})[._ -](?P<year>[0-9]{4})",
        ),
        EpisodeExpression::named(
            r".*[\\/]((?P<seriesname>[^\\/]+?)\s)?[Ss](?:eason)?\s*(?P<seasonnumber>[0-9]+)\s+[Ee](?:pisode)?\s*(?P<epnumber>[0-9]+).*$",
        ),
        EpisodeExpression::named(
            r".*[\\/](?![Ee]pisode)(?![^\\/]*[Ss][0-9]+[\[\] ._-]*[Ee][0-9]+)(?P<seriesname>[\w\s]+?)\s(?P<epnumber>[0-9]{1,4})(-(?P<endingepnumber>[0-9]{2,4}))*[^\\/x]*$",
        ),
        EpisodeExpression::positional(
            r"[\\/\._ \[\(-]([0-9]+)x([0-9]+(?:(?:[a-i]|\.[1-9])(?![0-9]))?)([^\\/]*)$",
        ),
        EpisodeExpression::named(
            r".*[\\/]?.*?(\[.*?\])+.*?(?P<seriesname>[-\w\s]+?)[\s_]*-[\s_]*(?P<epnumber>[0-9]+).*$",
        ),
        EpisodeExpression::named(
            r".*[\\/](?P<seriesname>[^\\/]+?)[\s_]+-[\s_]+(?P<epnumber>[0-9]+)[\s_]*(?:\[.*?\]|\(.*?\))*[\s_]*(?:\.\w+)?$",
        ),
        EpisodeExpression::named(
            r"[\\/._ -](?P<seriesname>(?![0-9]{3})([^\\/_])*)[\\/._ -](?P<seasonnumber>[0-9]+)(?P<epnumber>[0-9][0-9](?:(?:[a-i]|\.[1-9])(?![0-9]))?)([._ -][^\\/]*)$",
        )
        .optimistic()
        .without_absolute_numbers(),
        EpisodeExpression::positional(r".*?[\/._ -]p(?:ar)?t[_. -]()([ivx]+|[0-9]+)([._ -][^\/]*)$"),
        EpisodeExpression::named(
            r"[Ee]pisode (?P<epnumber>[0-9]+)(-(?P<endingepnumber>[0-9]+))?[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)[sS]?(?P<seasonnumber>[0-9]+)[xX](?P<epnumber>[0-9]+)[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)[sS](?P<seasonnumber>[0-9]+)[x,X]?[eE](?P<epnumber>[0-9]+)[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![sS]?[0-9]{1,4}[xX][0-9]{1,3})[^\\/])*)?([sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]+))[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>[^\\/]*)[sS](?P<seasonnumber>[0-9]{1,4})[xX\.]?[eE](?P<epnumber>[0-9]+)[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*[\\/](?P<epnumber>[0-9]+)(-(?P<endingepnumber>[0-9]+))*\.\w+$",
        )
        .optimistic(),
        EpisodeExpression::positional(r"([0-9]+)-([0-9]+)"),
        EpisodeExpression::named(
            r".*(\\|/)(?P<epnumber>[0-9]{1,3})(-(?P<endingepnumber>[0-9]{2,3}))*\s?-\s?[^\\/]*$",
        )
        .optimistic(),
        EpisodeExpression::named(
            r".*(\\|/)(?P<epnumber>[0-9]{1,3})(-(?P<endingepnumber>[0-9]{2,3}))*\.[^\\/]+$",
        )
        .optimistic(),
        EpisodeExpression::named(
            r".*[\\/][^\\/]* - (?P<epnumber>[0-9]{1,3})(-(?P<endingepnumber>[0-9]{2,3}))*[^\\/]*$",
        )
        .optimistic(),
        EpisodeExpression::named(
            r"[Ss]eason[\._ ](?P<seasonnumber>[0-9]+)[\\/](?P<epnumber>[0-9]{1,3})([^\\/]*)$",
        )
        .optimistic(),
        EpisodeExpression::named(
            r"(.*(\\|/))*(?P<seriesname>.+)\/[Ss](eason)?[\. _\-]*(?P<seasonnumber>[0-9]+)",
        ),
        EpisodeExpression::named(
            r"(.*(\\|/))*(?P<seriesname>.+)[\. _\-]+[sS](eason)?[\. _\-]*(?P<seasonnumber>[0-9]+)",
        ),
        EpisodeExpression::named(
            r"(?:\[(?:[^\]]+)\]\s*)?(?P<seriesname>\[[^\]]+\]|[^\[\]]+)\s*\[(?P<epnumber>[0-9]+)\]",
        ),
    ]
}

pub(crate) fn default_multiple_episode_expressions() -> Vec<EpisodeExpression> {
    vec![
        EpisodeExpression::named(
            r".*(\\|/)[sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3})((-| - )[0-9]{1,4}[eExX](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)[sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3})((-| - )[0-9]{1,4}[xX][eE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)[sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3})((-| - )?[xXeE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)[sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3})(-[xE]?[eE]?(?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![sS]?[0-9]{1,4}[xX][0-9]{1,3})[^\\/])*)?([sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3}))((-| - )[0-9]{1,4}[xXeE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![sS]?[0-9]{1,4}[xX][0-9]{1,3})[^\\/])*)?([sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3}))((-| - )[0-9]{1,4}[xX][eE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![sS]?[0-9]{1,4}[xX][0-9]{1,3})[^\\/])*)?([sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3}))((-| - )?[xXeE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>((?![sS]?[0-9]{1,4}[xX][0-9]{1,3})[^\\/])*)?([sS]?(?P<seasonnumber>[0-9]{1,4})[xX](?P<epnumber>[0-9]{1,3}))(-[xX]?[eE]?(?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>[^\\/]*)[sS](?P<seasonnumber>[0-9]{1,4})[xX\.]?[eE](?P<epnumber>[0-9]{1,3})((-| - )?[xXeE](?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
        EpisodeExpression::named(
            r".*(\\|/)(?P<seriesname>[^\\/]*)[sS](?P<seasonnumber>[0-9]{1,4})[xX\.]?[eE](?P<epnumber>[0-9]{1,3})(-[xX]?[eE]?(?P<endingepnumber>[0-9]{1,3}))+[^\\/]*$",
        ),
    ]
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EpisodePathParserResult {
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub ending_episode_number: Option<i32>,
    pub series_name: Option<String>,
    pub success: bool,
    pub is_by_date: bool,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
}

pub struct EpisodePathParser {
    options: NamingOptions,
}

impl EpisodePathParser {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn parse(&self, path: &str, is_directory: bool) -> EpisodePathParserResult {
        self.parse_with_options(path, is_directory, None, None, None, true)
    }

    #[must_use]
    pub fn parse_with_options(
        &self,
        path: &str,
        is_directory: bool,
        is_named: Option<bool>,
        is_optimistic: Option<bool>,
        supports_absolute_numbers: Option<bool>,
        fill_extended_info: bool,
    ) -> EpisodePathParserResult {
        Self::parse_borrowed(
            path,
            &self.options,
            is_directory,
            is_named,
            is_optimistic,
            supports_absolute_numbers,
            fill_extended_info,
        )
    }

    pub fn parse_borrowed(
        path: &str,
        options: &NamingOptions,
        is_directory: bool,
        is_named: Option<bool>,
        is_optimistic: Option<bool>,
        supports_absolute_numbers: Option<bool>,
        fill_extended_info: bool,
    ) -> EpisodePathParserResult {
        let owned_path;
        let path = if is_directory {
            owned_path = format!("{path}.mp4");
            owned_path.as_str()
        } else {
            path
        };

        let mut result = options
            .episode_expressions
            .iter()
            .filter(|expression| {
                supports_absolute_numbers
                    .is_none_or(|value| expression.supports_absolute_episode_numbers == value)
                    && is_named.is_none_or(|value| expression.is_named == value)
                    && is_optimistic.is_none_or(|value| expression.is_optimistic == value)
            })
            .find_map(|expression| {
                parse_expression(path, expression).filter(|result| result.success)
            })
            .unwrap_or_default();

        if result.success && fill_extended_info {
            Self::fill_additional_with(path, options, &mut result);
            result.series_name = result
                .series_name
                .take()
                .map(|name| clean_series_name(&name));
            fill_series_from_path(path, &mut result);
        }
        result
    }

    fn fill_additional_with(
        path: &str,
        options: &NamingOptions,
        result: &mut EpisodePathParserResult,
    ) {
        for expression in options
            .episode_expressions
            .iter()
            .chain(&options.multiple_episode_expressions)
            .filter(|expression| expression.is_named)
        {
            let Some(additional) = parse_expression(path, expression).filter(|value| value.success)
            else {
                continue;
            };
            if result.series_name.as_deref().is_none_or(str::is_empty)
                && additional
                    .series_name
                    .as_deref()
                    .is_some_and(|name| !name.is_empty())
            {
                result.series_name = additional.series_name;
            }
            if result.ending_episode_number.is_none() && result.episode_number.is_some() {
                result.ending_episode_number = additional.ending_episode_number;
            }
            if result
                .series_name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
                && (result.episode_number.is_none() || result.ending_episode_number.is_some())
            {
                break;
            }
        }
    }
}

fn parse_expression(path: &str, expression: &EpisodeExpression) -> Option<EpisodePathParserResult> {
    let normalized_path = expression.is_by_date.then(|| path.replace('_', "-"));
    let (matched_path, captures) =
        if let Some(captures) = expression.regex().captures(path).ok().flatten() {
            (path, captures)
        } else {
            let normalized_path = normalized_path.as_deref()?;
            (
                normalized_path,
                expression
                    .regex()
                    .captures(normalized_path)
                    .ok()
                    .flatten()?,
            )
        };
    let mut result = EpisodePathParserResult {
        season_number: capture_number(&captures, "seasonnumber"),
        episode_number: capture_number(&captures, "epnumber"),
        series_name: expression.is_named.then(|| {
            captures
                .name("seriesname")
                .map_or_else(String::new, |value| value.as_str().to_owned())
        }),
        is_by_date: expression.is_by_date,
        ..EpisodePathParserResult::default()
    };

    if expression.is_by_date {
        result.year = capture_number(&captures, "year").or_else(|| capture_at(&captures, 2));
        result.month = capture_number(&captures, "month").or_else(|| capture_at(&captures, 3));
        result.day = capture_number(&captures, "day").or_else(|| capture_at(&captures, 4));
        result.success = true;
        if !valid_date(result.year, result.month, result.day) {
            result.year = None;
            result.month = None;
            result.day = None;
        }
        return Some(result);
    }

    if !expression.is_named {
        result.season_number = captures
            .get(1)
            .and_then(|value| value.as_str().parse().ok());
        result.episode_number = captures
            .get(2)
            .and_then(|value| value.as_str().parse().ok());
    }

    if let Some(ending) = captures.name("endingepnumber") {
        let next = matched_path.as_bytes().get(ending.end()).copied();
        if !next.is_some_and(|value| value.is_ascii_digit() || b"iIpP".contains(&value)) {
            result.ending_episode_number = ending.as_str().parse().ok();
        }
    } else if captures.name("endingepnumber_suffix").is_some() {
        result.ending_episode_number = result.episode_number;
    }

    if let Some(season) = result.season_number
        && ((200..1928).contains(&season) || season > 2500)
    {
        return Some(result);
    }
    result.success = result.episode_number.is_some();
    Some(result)
}

fn capture_number(captures: &fancy_regex::Captures<'_>, name: &str) -> Option<i32> {
    captures.name(name)?.as_str().parse().ok()
}

fn capture_at(captures: &fancy_regex::Captures<'_>, index: usize) -> Option<i32> {
    captures.get(index)?.as_str().parse().ok()
}

fn valid_date(year: Option<i32>, month: Option<i32>, day: Option<i32>) -> bool {
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year <= 1 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn clean_series_name(name: &str) -> String {
    name.trim()
        .trim_matches(['_', '.', '-'])
        .trim()
        .trim_start_matches('[')
        .to_owned()
}

fn fill_series_from_path(path: &str, result: &mut EpisodePathParserResult) {
    if result.is_by_date
        && let Some(name) = daily_series_name_from_path(path)
    {
        result.series_name = Some(name);
    }
    if let Some(name) = result.series_name.as_mut()
        && name.ends_with("_KTLADT")
    {
        name.truncate(name.len() - "_KTLADT".len());
    }

    let parts = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if result.series_name.as_deref().is_none_or(str::is_empty)
        && parts.len() >= 3
        && is_season_folder(parts[parts.len() - 2])
    {
        result.series_name = Some(strip_year(parts[parts.len() - 3]));
    }

    if parts.len() >= 4 {
        let parent = parts[parts.len() - 2];
        let grandparent = parts[parts.len() - 3];
        if parent.len() > grandparent.len()
            && parent
                .get(..grandparent.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(grandparent))
        {
            result.series_name = Some(grandparent.to_owned());
        }
    }
}

fn daily_series_name_from_path(path: &str) -> Option<String> {
    let stem = file_stem(path);
    for expression in [
        r"^(?P<name>.*?)[._ \-]*(?:\(|\[)?(?P<year>[0-9]{4})[._\-](?P<month>[0-9]{2})[._\-](?P<day>[0-9]{2})",
        r"^(?P<name>.*?)[._ \-]*(?:\(|\[)?(?P<day>[0-9]{2})[._\-](?P<month>[0-9]{2})[._\-](?P<year>[0-9]{4})",
    ] {
        let Ok(regex) = RegexBuilder::new(expression).case_insensitive(true).build() else {
            continue;
        };
        let Some(captures) = regex.captures(stem).ok().flatten() else {
            continue;
        };
        let Some(name) = captures.name("name") else {
            continue;
        };
        let name = name
            .as_str()
            .trim()
            .trim_matches(['_', '.', '-', ' ', '(', '['])
            .trim();
        let name = if name
            .get(name.len().saturating_sub("_KTLADT".len())..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case("_KTLADT"))
        {
            &name[..name.len() - "_KTLADT".len()]
        } else {
            name
        };
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}

fn is_season_folder(value: &str) -> bool {
    value
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("season"))
}

fn strip_year(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.ends_with(')')
        && let Some(index) = trimmed.rfind(" (")
        && trimmed[index + 2..trimmed.len() - 1]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return trimmed[..index].to_owned();
    }
    trimmed.to_owned()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EpisodeInfo {
    pub path: String,
    pub container: Option<String>,
    pub series_name: Option<String>,
    pub format_3d: Option<String>,
    pub is_3d: bool,
    pub is_stub: bool,
    pub stub_type: Option<String>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub ending_episode_number: Option<i32>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub is_by_date: bool,
    pub provider_ids: ProviderIdMap,
}

pub struct EpisodeResolver {
    options: NamingOptions,
}

impl EpisodeResolver {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn resolve(&self, path: &str, is_directory: bool) -> Option<EpisodeInfo> {
        self.resolve_with_options(path, is_directory, None, None, None, true)
    }

    #[must_use]
    pub fn resolve_with_options(
        &self,
        path: &str,
        is_directory: bool,
        is_named: Option<bool>,
        is_optimistic: Option<bool>,
        supports_absolute_numbers: Option<bool>,
        fill_extended_info: bool,
    ) -> Option<EpisodeInfo> {
        let mut is_stub = false;
        let mut stub_type = None;
        let container = if is_directory {
            None
        } else {
            let extension = extension(path)?;
            if !self
                .options
                .video_file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            {
                stub_type = StubResolver::try_resolve_file(path, &self.options)?;
                is_stub = true;
            }
            Some(extension.trim_start_matches('.').to_owned())
        };
        let format = Format3dParser::parse(path, &self.options);
        let provider_path = if is_directory {
            file_name(path)
        } else {
            file_stem(path)
        };
        let provider_ids = provider_ids::from_path(
            provider_path,
            &[
                ("Imdb", "imdbid"),
                ("Tvdb", "tvdbid"),
                ("TvMaze", "tvmazeid"),
                ("Tmdb", "tmdbid"),
            ],
        );
        let parsed = EpisodePathParser::parse_borrowed(
            path,
            &self.options,
            is_directory,
            is_named,
            is_optimistic,
            supports_absolute_numbers,
            fill_extended_info,
        );
        if !parsed.success && !is_stub {
            return None;
        }
        Some(EpisodeInfo {
            path: path.to_owned(),
            container,
            series_name: parsed.series_name.or_else(|| Some(String::new())),
            format_3d: format.format_3d,
            is_3d: format.is_3d,
            is_stub,
            stub_type,
            season_number: parsed.season_number,
            episode_number: parsed.episode_number,
            ending_episode_number: parsed.ending_episode_number,
            year: parsed.year,
            month: parsed.month,
            day: parsed.day,
            is_by_date: parsed.is_by_date,
            provider_ids,
        })
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    name.rfind('.').map_or(name, |index| &name[..index])
}

fn extension(path: &str) -> Option<&str> {
    let name = file_name(path);
    let index = name.rfind('.')?;
    (index + 1 < name.len()).then_some(&name[index..])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeriesStatus {
    Continuing,
    Ended,
    Unreleased,
}

pub struct TvParserHelpers;

impl TvParserHelpers {
    #[must_use]
    pub fn try_parse_series_status(status: Option<&str>) -> Option<SeriesStatus> {
        let status = status?.trim();
        if status.eq_ignore_ascii_case("ended")
            || status.eq_ignore_ascii_case("cancelled")
            || status.eq_ignore_ascii_case("canceled")
        {
            Some(SeriesStatus::Ended)
        } else if status.eq_ignore_ascii_case("continuing")
            || status.eq_ignore_ascii_case("pilot")
            || status.eq_ignore_ascii_case("returning")
            || status.eq_ignore_ascii_case("returning series")
        {
            Some(SeriesStatus::Continuing)
        } else if status.eq_ignore_ascii_case("unreleased") {
            Some(SeriesStatus::Unreleased)
        } else {
            None
        }
    }
}
