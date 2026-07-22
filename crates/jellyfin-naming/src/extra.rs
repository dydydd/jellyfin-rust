use regex::RegexBuilder;

use crate::{
    common::NamingOptions,
    video::{ExtraRule, ExtraRuleType, ExtraType, MediaType},
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtraResult {
    pub extra_type: Option<ExtraType>,
    pub rule: Option<ExtraRule>,
}

pub struct ExtraResolver;

impl ExtraResolver {
    #[must_use]
    pub fn resolve(path: &str, options: &NamingOptions) -> ExtraResult {
        Self::resolve_with_library_root(path, options, None)
    }

    #[must_use]
    pub fn resolve_with_library_root(
        path: &str,
        options: &NamingOptions,
        library_root: Option<&str>,
    ) -> ExtraResult {
        let is_audio = has_extension(path, &options.audio_file_extensions);
        let is_video = has_extension(path, &options.video_file_extensions);
        let filename = file_name(path);
        let stem = file_stem(filename);
        let suffix_stem = stem.trim_end_matches(|character: char| character.is_ascii_digit());
        let directory = parent_path(path);
        let directory_name = file_name(directory);
        let is_library_root = library_root.is_some_and(|library_root| {
            normalized_path(directory).eq_ignore_ascii_case(normalized_path(library_root))
        });

        for rule in &options.video_extra_rules {
            let media_matches = match rule.media_type {
                MediaType::Audio => is_audio,
                MediaType::Video => is_video,
            };
            if !media_matches {
                continue;
            }
            let rule_matches = match rule.rule_type {
                ExtraRuleType::DirectoryName => {
                    !is_library_root && directory_name.eq_ignore_ascii_case(&rule.token)
                }
                ExtraRuleType::Filename => stem.eq_ignore_ascii_case(&rule.token),
                ExtraRuleType::Regex => RegexBuilder::new(&rule.token)
                    .case_insensitive(true)
                    .build()
                    .is_ok_and(|regex| regex.is_match(filename)),
                ExtraRuleType::Suffix => ends_with_ignore_ascii_case(suffix_stem, &rule.token),
            };
            if rule_matches {
                return ExtraResult {
                    extra_type: Some(rule.extra_type),
                    rule: Some(rule.clone()),
                };
            }
        }
        ExtraResult::default()
    }

    #[must_use]
    pub fn get_extra_info(path: &str, options: &NamingOptions) -> ExtraResult {
        Self::resolve(path, options)
    }

    #[must_use]
    pub fn get_extra_info_with_library_root(
        path: &str,
        options: &NamingOptions,
        library_root: &str,
    ) -> ExtraResult {
        Self::resolve_with_library_root(path, options, Some(library_root))
    }
}

pub type ExtraRuleResolver = ExtraResolver;

fn has_extension(path: &str, extensions: &[String]) -> bool {
    extension(path).is_some_and(|extension| {
        extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

fn ends_with_ignore_ascii_case(value: &str, suffix: &str) -> bool {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(suffix))
}

fn normalized_path(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent_path(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |index| &path[..index])
}

fn extension(path: &str) -> Option<&str> {
    let name = file_name(path);
    let index = name.rfind('.')?;
    (index + 1 < name.len()).then_some(&name[index..])
}

fn file_stem(path: &str) -> &str {
    path.rfind('.').map_or(path, |index| &path[..index])
}
