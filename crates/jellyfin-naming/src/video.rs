use std::fmt;

use crate::{ExtraResolver, NamingOptions, ProviderIdMap, provider_ids};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtraType {
    BehindTheScenes,
    Clip,
    DeletedScene,
    Featurette,
    Interview,
    Sample,
    Scene,
    Short,
    ThemeSong,
    ThemeVideo,
    Trailer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtraRuleType {
    DirectoryName,
    Filename,
    Regex,
    Suffix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaType {
    Audio,
    Video,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraRule {
    pub extra_type: ExtraType,
    pub rule_type: ExtraRuleType,
    pub token: String,
    pub media_type: MediaType,
}

impl ExtraRule {
    #[must_use]
    pub fn new(
        extra_type: ExtraType,
        rule_type: ExtraRuleType,
        token: impl Into<String>,
        media_type: MediaType,
    ) -> Self {
        Self {
            extra_type,
            rule_type,
            token: token.into(),
            media_type,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubTypeRule {
    pub token: String,
    pub stub_type: String,
}

impl StubTypeRule {
    #[must_use]
    pub fn new(token: impl Into<String>, stub_type: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            stub_type: stub_type.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Format3dRule {
    pub token: String,
    pub preceding_token: Option<String>,
}

impl Format3dRule {
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            preceding_token: None,
        }
    }

    #[must_use]
    pub fn with_preceding(token: impl Into<String>, preceding_token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            preceding_token: Some(preceding_token.into()),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Format3dResult {
    pub is_3d: bool,
    pub format_3d: Option<String>,
}

pub struct Format3dParser;

impl Format3dParser {
    #[must_use]
    pub fn parse(path: &str, options: &NamingOptions) -> Format3dResult {
        let is_delimiter = |character: char| {
            character == ' ' || options.video_flag_delimiters.contains(&character)
        };
        let tokens = path
            .split(is_delimiter)
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();

        for rule in &options.format_3d_rules {
            let matches = if let Some(preceding) = rule.preceding_token.as_deref() {
                let mut found_prefix = false;
                tokens.iter().any(|token| {
                    if !found_prefix {
                        found_prefix = token.eq_ignore_ascii_case(preceding);
                        false
                    } else {
                        token.eq_ignore_ascii_case(&rule.token)
                    }
                })
            } else {
                tokens
                    .iter()
                    .any(|token| token.eq_ignore_ascii_case(&rule.token))
            };
            if matches {
                return Format3dResult {
                    is_3d: true,
                    format_3d: Some(rule.token.clone()),
                };
            }
        }
        Format3dResult::default()
    }
}

pub struct StubResolver;

impl StubResolver {
    #[must_use]
    pub fn try_resolve_file(path: &str, options: &NamingOptions) -> Option<Option<String>> {
        if path.is_empty() {
            return None;
        }
        let file_extension = extension(path)?;
        if !options
            .stub_file_extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(file_extension))
        {
            return None;
        }

        let stem = file_stem(path);
        let token = extension(stem).unwrap_or_default().trim_start_matches('.');
        let stub_type = options
            .stub_types
            .iter()
            .find(|rule| rule.token.eq_ignore_ascii_case(token))
            .map(|rule| rule.stub_type.clone());
        Some(stub_type)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanDateTimeResult {
    pub name: String,
    pub year: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFileInfo {
    pub name: String,
    pub path: String,
    pub container: Option<String>,
    pub year: Option<u16>,
    pub format_3d: Option<String>,
    pub is_3d: bool,
    pub is_stub: bool,
    pub stub_type: Option<String>,
    pub extra_type: Option<ExtraType>,
    pub extra_rule: Option<ExtraRule>,
    pub is_directory: bool,
    pub provider_ids: ProviderIdMap,
}

impl VideoFileInfo {
    #[must_use]
    pub fn file_name_without_extension(&self) -> &str {
        if self.is_directory {
            file_name(&self.path)
        } else {
            file_stem(&self.path)
        }
    }
}

impl fmt::Display for VideoFileInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "VideoFileInfo(Name: '{}')", self.name)
    }
}

pub struct VideoResolver;

impl VideoResolver {
    #[must_use]
    pub fn try_clean_string(name: Option<&str>, options: &NamingOptions) -> Option<String> {
        let mut name = name.filter(|name| !name.is_empty())?.to_owned();
        let mut cleaned_any = false;
        for (index, regex) in options.clean_string_regexes.iter().enumerate() {
            let Some(captures) = regex.captures(&name).ok().flatten() else {
                continue;
            };
            let Some(cleaned) = captures.name("cleaned") else {
                continue;
            };
            let cleaned = cleaned.as_str().trim();
            if index == 3 && is_extension_only(cleaned) {
                continue;
            }
            name = cleaned.to_owned();
            cleaned_any = true;
        }
        cleaned_any.then_some(name)
    }

    #[must_use]
    pub fn clean_date_time(name: &str, options: &NamingOptions) -> CleanDateTimeResult {
        if !name.is_empty() {
            for regex in &options.clean_date_time_regexes {
                let Some(captures) = regex.captures(name).ok().flatten() else {
                    continue;
                };
                let (Some(cleaned), Some(year)) = (captures.get(1), captures.get(2)) else {
                    continue;
                };
                let Ok(year) = year.as_str().parse() else {
                    continue;
                };
                return CleanDateTimeResult {
                    name: cleaned.as_str().trim_end().to_owned(),
                    year: Some(year),
                };
            }
        }
        CleanDateTimeResult {
            name: name.to_owned(),
            year: None,
        }
    }

    #[must_use]
    pub fn resolve_file(path: Option<&str>, options: &NamingOptions) -> Option<VideoFileInfo> {
        Self::resolve_with_library_root(path, false, options, None)
    }

    #[must_use]
    pub fn resolve_file_with_library_root(
        path: Option<&str>,
        options: &NamingOptions,
        library_root: Option<&str>,
    ) -> Option<VideoFileInfo> {
        Self::resolve_with_library_root(path, false, options, library_root)
    }

    #[must_use]
    pub fn resolve_directory(path: Option<&str>, options: &NamingOptions) -> Option<VideoFileInfo> {
        Self::resolve_with_library_root(path, true, options, None)
    }

    #[must_use]
    pub fn resolve_directory_with_library_root(
        path: Option<&str>,
        options: &NamingOptions,
        library_root: Option<&str>,
    ) -> Option<VideoFileInfo> {
        Self::resolve_with_library_root(path, true, options, library_root)
    }

    #[must_use]
    pub fn resolve(
        path: Option<&str>,
        is_directory: bool,
        options: &NamingOptions,
    ) -> Option<VideoFileInfo> {
        Self::resolve_with_library_root(path, is_directory, options, None)
    }

    #[must_use]
    pub fn resolve_with_library_root(
        path: Option<&str>,
        is_directory: bool,
        options: &NamingOptions,
        library_root: Option<&str>,
    ) -> Option<VideoFileInfo> {
        let path = path.filter(|path| !path.is_empty())?;
        let (container, is_stub, stub_type) = if is_directory {
            (None, false, None)
        } else {
            let extension = extension(path)?;
            let supported = options
                .video_file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension));
            let stub_type = if supported {
                None
            } else {
                StubResolver::try_resolve_file(path, options)?
            };
            (
                Some(extension.trim_start_matches('.').to_owned()),
                !supported,
                stub_type,
            )
        };
        let format = Format3dParser::parse(path, options);
        let raw_name = file_stem(path);
        let provider_path = if is_directory {
            file_name(path)
        } else {
            raw_name
        };
        let provider_ids = provider_ids::from_path(
            provider_path,
            &[("Imdb", "imdbid"), ("Tmdb", "tmdbid"), ("Tvdb", "tvdbid")],
        );
        let date = Self::clean_date_time(raw_name, options);
        let name = Self::try_clean_string(Some(&date.name), options).unwrap_or(date.name);
        let extra = ExtraResolver::resolve_with_library_root(path, options, library_root);

        Some(VideoFileInfo {
            name,
            path: path.to_owned(),
            container,
            year: date.year,
            format_3d: format.format_3d,
            is_3d: format.is_3d,
            is_stub,
            stub_type,
            extra_type: extra.extra_type,
            extra_rule: extra.rule,
            is_directory,
            provider_ids,
        })
    }

    #[must_use]
    pub fn is_video_file(path: &str, options: &NamingOptions) -> bool {
        extension(path).is_some_and(|extension| {
            options
                .video_file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
    }

    #[must_use]
    pub fn is_stub_file(path: &str, options: &NamingOptions) -> bool {
        extension(path).is_some_and(|extension| {
            options
                .stub_file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
    }
}

fn is_extension_only(value: &str) -> bool {
    value.strip_prefix('.').is_some_and(|extension| {
        !extension.is_empty() && extension.chars().all(char::is_alphanumeric)
    })
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn extension(path: &str) -> Option<&str> {
    let name = file_name(path);
    let index = name.rfind('.')?;
    (index + 1 < name.len()).then_some(&name[index..])
}

fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    name.rfind('.').map_or(name, |index| &name[..index])
}
