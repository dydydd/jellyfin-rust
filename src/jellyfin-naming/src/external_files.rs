use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use crate::common::NamingOptions;

/// External stream type whose file extensions should be accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DlnaProfileType {
    Audio,
    Subtitle,
    Lyric,
}

/// Language metadata needed by the external path parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageInfo {
    pub name: String,
    pub three_letter_iso_language_name: Option<String>,
}

impl LanguageInfo {
    pub fn new(
        name: impl Into<String>,
        three_letter_iso_language_name: Option<impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            three_letter_iso_language_name: three_letter_iso_language_name.map(Into::into),
        }
    }

    fn into_parser_language(self) -> Option<String> {
        if self.name.contains('-') {
            Some(self.name)
        } else {
            self.three_letter_iso_language_name
        }
    }
}

/// Lookup boundary corresponding to Jellyfin's `ILocalizationManager`.
pub trait LocalizationManager {
    fn find_language_info(&self, language: &str) -> Option<LanguageInfo>;
}

/// Information extracted from an external audio, subtitle, or lyric path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalPathParserResult {
    pub path: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_hearing_impaired: bool,
}

impl ExternalPathParserResult {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            language: None,
            title: None,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        }
    }
}

/// Parses metadata tokens from external media paths.
pub struct ExternalPathParser<'a, L: LocalizationManager + ?Sized> {
    naming_options: Arc<NamingOptions>,
    localization_manager: &'a L,
    profile_type: DlnaProfileType,
}

impl<'a, L: LocalizationManager + ?Sized> ExternalPathParser<'a, L> {
    pub fn new(
        naming_options: impl Into<Arc<NamingOptions>>,
        localization_manager: &'a L,
        profile_type: DlnaProfileType,
    ) -> Self {
        Self {
            naming_options: naming_options.into(),
            localization_manager,
            profile_type,
        }
    }

    /// Returns `None` when `path` is empty or its extension does not belong to
    /// this parser's profile type.
    pub fn parse_file(
        &self,
        path: &str,
        extra_string: Option<&str>,
    ) -> Option<ExternalPathParserResult> {
        if path.is_empty() || !self.has_matching_extension(path) {
            return None;
        }

        let mut result = ExternalPathParserResult::new(path);
        let Some(extra_string) = extra_string.filter(|extra| !extra.is_empty()) else {
            return Some(result);
        };
        let mut extra_string = extra_string.to_owned();

        for separator in &self.naming_options.media_flag_delimiters {
            let snapshot = std::mem::take(&mut extra_string);
            let mut filtered = Cow::Borrowed(snapshot.as_str());
            let mut language_end = snapshot.len();
            let mut title_string = String::new();

            while language_end > 0 {
                let language_string = &snapshot[..language_end];
                let Some(last_separator) = language_string.rfind(*separator) else {
                    break;
                };
                let current_slice = &language_string[last_separator..];
                let without_separator = &language_string[last_separator + separator.len_utf8()..];

                if contains_any_case_insensitive(
                    without_separator,
                    &self.naming_options.media_default_flags,
                ) {
                    result.is_default = true;
                    filtered = Cow::Owned(replace_case_insensitive(
                        filtered.as_ref(),
                        current_slice,
                        "",
                    ));
                    language_end = last_separator;
                    continue;
                }

                if contains_any_case_insensitive(
                    without_separator,
                    &self.naming_options.media_forced_flags,
                ) {
                    result.is_forced = true;
                    filtered = Cow::Owned(replace_case_insensitive(
                        filtered.as_ref(),
                        current_slice,
                        "",
                    ));
                    language_end = last_separator;
                    continue;
                }

                let culture = self
                    .localization_manager
                    .find_language_info(without_separator);
                let hindi_hearing_impaired = result.language.as_deref() == Some("hin");
                if let Some(culture) =
                    culture.filter(|_| result.language.is_none() || hindi_hearing_impaired)
                {
                    if hindi_hearing_impaired {
                        // `hi` is both Hindi's ISO code and a hearing-impaired flag.
                        result.is_hearing_impaired = true;
                    }
                    result.language = culture.into_parser_language();
                    filtered = Cow::Owned(replace_case_insensitive(
                        filtered.as_ref(),
                        current_slice,
                        "",
                    ));
                } else if self
                    .naming_options
                    .media_hearing_impaired_flags
                    .iter()
                    .any(|flag| without_separator.eq_ignore_ascii_case(flag))
                {
                    result.is_hearing_impaired = true;
                    filtered = Cow::Owned(replace_case_insensitive(
                        filtered.as_ref(),
                        current_slice,
                        "",
                    ));
                } else {
                    title_string.insert_str(0, current_slice);
                }

                language_end = last_separator;
            }

            result.title = title_string.strip_prefix(*separator).map(ToOwned::to_owned);
            extra_string = match filtered {
                Cow::Borrowed(_) => snapshot,
                Cow::Owned(filtered) => filtered,
            };
        }

        Some(result)
    }

    fn has_matching_extension(&self, path: &str) -> bool {
        let extension = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"));
        let Some(extension) = extension else {
            return false;
        };

        let extensions = match self.profile_type {
            DlnaProfileType::Audio => &self.naming_options.audio_file_extensions,
            DlnaProfileType::Subtitle => &self.naming_options.subtitle_file_extensions,
            DlnaProfileType::Lyric => &self.naming_options.lyric_file_extensions,
        };
        extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
    }
}

fn contains_any_case_insensitive(haystack: &str, needles: &[String]) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| haystack.contains(&needle.to_ascii_lowercase()))
}

fn replace_case_insensitive(source: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return source.to_owned();
    }

    let source_lower = source.to_ascii_lowercase();
    let from_lower = from.to_ascii_lowercase();
    let mut result = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_start) = source_lower[cursor..].find(&from_lower) {
        let start = cursor + relative_start;
        result.push_str(&source[cursor..start]);
        result.push_str(to);
        cursor = start + from.len();
    }
    result.push_str(&source[cursor..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestLocalizationManager;

    impl LocalizationManager for TestLocalizationManager {
        fn find_language_info(&self, language: &str) -> Option<LanguageInfo> {
            let lower = language.to_ascii_lowercase();
            if lower.starts_with("en") {
                Some(LanguageInfo::new("English", Some("eng")))
            } else if lower.starts_with("fr") {
                Some(LanguageInfo::new("French", Some("fre")))
            } else if lower.starts_with("hi") {
                Some(LanguageInfo::new("Hindi", Some("hin")))
            } else {
                None
            }
        }
    }

    fn parser(
        profile_type: DlnaProfileType,
    ) -> ExternalPathParser<'static, TestLocalizationManager> {
        static LOCALIZATION_MANAGER: TestLocalizationManager = TestLocalizationManager;
        ExternalPathParser::new(
            NamingOptions::default(),
            &LOCALIZATION_MANAGER,
            profile_type,
        )
    }

    #[test]
    fn audio_extensions_not_matched_return_none() {
        let parser = parser(DlnaProfileType::Audio);
        for path in [
            "",
            "MyVideo.ass",
            "MyVideo.mks",
            "MyVideo.sami",
            "MyVideo.srt",
            "MyVideo.m4v",
        ] {
            assert_eq!(parser.parse_file(path, Some("")), None, "path: {path}");
        }
    }

    #[test]
    fn audio_extensions_matched_return_path() {
        let parser = parser(DlnaProfileType::Audio);
        for path in [
            "MyVideo.aa",
            "MyVideo.aac",
            "MyVideo.flac",
            "MyVideo.m4a",
            "MyVideo.mka",
            "MyVideo.mp3",
        ] {
            let actual = parser.parse_file(path, Some("")).unwrap();
            assert_eq!(actual.path, path, "path: {path}");
        }
    }

    #[test]
    fn subtitle_extensions_not_matched_return_none() {
        let parser = parser(DlnaProfileType::Subtitle);
        for path in [
            "",
            "MyVideo.aa",
            "MyVideo.aac",
            "MyVideo.flac",
            "MyVideo.mka",
            "MyVideo.m4v",
        ] {
            assert_eq!(parser.parse_file(path, Some("")), None, "path: {path}");
        }
    }

    #[test]
    fn subtitle_extensions_matched_return_path() {
        let parser = parser(DlnaProfileType::Subtitle);
        for path in [
            "MyVideo.ass",
            "MyVideo.mks",
            "MyVideo.sami",
            "MyVideo.srt",
            "MyVideo.vtt",
        ] {
            let actual = parser.parse_file(path, Some("")).unwrap();
            assert_eq!(actual.path, path, "path: {path}");
        }
    }

    #[derive(Clone, Copy)]
    struct TokenCase {
        tokens: &'static str,
        title: Option<&'static str>,
        language: Option<&'static str>,
        is_default: bool,
        is_forced: bool,
        is_hearing_impaired: bool,
    }

    const TOKEN_CASES: &[TokenCase] = &[
        TokenCase {
            tokens: "",
            title: None,
            language: None,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".default",
            title: None,
            language: None,
            is_default: true,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".forced",
            title: None,
            language: None,
            is_default: false,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".foreign",
            title: None,
            language: None,
            is_default: false,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".default.forced",
            title: None,
            language: None,
            is_default: true,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".forced.default",
            title: None,
            language: None,
            is_default: true,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".DEFAULT.FORCED",
            title: None,
            language: None,
            is_default: true,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".en",
            title: None,
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".EN",
            title: None,
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".hi",
            title: None,
            language: Some("hin"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".fr.en",
            title: Some("fr"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".en.fr",
            title: Some("en"),
            language: Some("fre"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".title.en.fr",
            title: Some("title.en"),
            language: Some("fre"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".Title Goes Here",
            title: Some("Title Goes Here"),
            language: None,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".Title.with.Separator",
            title: Some("Title.with.Separator"),
            language: None,
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".title.en.default.forced",
            title: Some("title"),
            language: Some("eng"),
            is_default: true,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".forced.default.en.title",
            title: Some("title"),
            language: Some("eng"),
            is_default: true,
            is_forced: true,
            is_hearing_impaired: false,
        },
        TokenCase {
            tokens: ".sdh.en.title",
            title: Some("title"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: true,
        },
        TokenCase {
            tokens: ".en.cc.title",
            title: Some("title"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: true,
        },
        TokenCase {
            tokens: ".hi.en.title",
            title: Some("title"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: true,
        },
        TokenCase {
            tokens: ".en.hi.title",
            title: Some("title"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: true,
        },
        TokenCase {
            tokens: ".Subs for Chinese Audio.eng",
            title: Some("Subs for Chinese Audio"),
            language: Some("eng"),
            is_default: false,
            is_forced: false,
            is_hearing_impaired: false,
        },
    ];

    #[test]
    fn extra_tokens_parse_to_values() {
        let parser = parser(DlnaProfileType::Subtitle);
        for case in TOKEN_CASES {
            let path = format!("My.Video{}.srt", case.tokens);
            let actual = parser.parse_file(&path, Some(case.tokens)).unwrap();
            assert_eq!(
                actual.title.as_deref(),
                case.title,
                "tokens: {}",
                case.tokens
            );
            assert_eq!(
                actual.language.as_deref(),
                case.language,
                "tokens: {}",
                case.tokens
            );
            assert_eq!(
                actual.is_default, case.is_default,
                "tokens: {}",
                case.tokens
            );
            assert_eq!(actual.is_forced, case.is_forced, "tokens: {}", case.tokens);
            assert_eq!(
                actual.is_hearing_impaired, case.is_hearing_impaired,
                "tokens: {}",
                case.tokens
            );
        }
    }

    #[test]
    fn accepts_lyric_and_case_insensitive_extensions() {
        let lyric_parser = parser(DlnaProfileType::Lyric);
        assert!(lyric_parser.parse_file("song.ELRC", None).is_some());
        assert!(lyric_parser.parse_file("song.SRT", None).is_none());

        let subtitle_parser = parser(DlnaProfileType::Subtitle);
        assert!(subtitle_parser.parse_file("movie.SRT", None).is_some());
    }

    #[test]
    fn uses_configured_extension_and_flags() {
        static LOCALIZATION_MANAGER: TestLocalizationManager = TestLocalizationManager;
        let options = NamingOptions {
            subtitle_file_extensions: vec![".captions".to_owned()],
            media_default_flags: vec!["primary".to_owned()],
            ..NamingOptions::default()
        };
        let parser =
            ExternalPathParser::new(options, &LOCALIZATION_MANAGER, DlnaProfileType::Subtitle);

        let actual = parser
            .parse_file("movie.primary.captions", Some(".primary"))
            .unwrap();
        assert!(actual.is_default);
        assert!(parser.parse_file("movie.srt", None).is_none());
    }

    #[test]
    fn preserves_region_specific_language_name() {
        struct RegionLocalizationManager;
        impl LocalizationManager for RegionLocalizationManager {
            fn find_language_info(&self, language: &str) -> Option<LanguageInfo> {
                (language == "pt-BR").then(|| LanguageInfo::new("pt-BR", Some("por")))
            }
        }

        let parser = ExternalPathParser::new(
            NamingOptions::default(),
            &RegionLocalizationManager,
            DlnaProfileType::Subtitle,
        );
        let actual = parser
            .parse_file("movie.pt-BR.srt", Some(".pt-BR"))
            .unwrap();
        assert_eq!(actual.language.as_deref(), Some("pt-BR"));
    }

    #[test]
    fn delimiter_only_extra_string_produces_empty_title() {
        let parser = parser(DlnaProfileType::Subtitle);
        let actual = parser.parse_file("movie..srt", Some(".")).unwrap();
        assert_eq!(actual.title.as_deref(), Some(""));
    }
}
