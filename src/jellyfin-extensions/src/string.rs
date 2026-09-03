//! Unicode-aware string helpers.

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Removes diacritical marks while leaving non-Latin scripts intact.
///
/// Unicode canonical decomposition handles combining accents. A small set of
/// Latin letters that Unicode intentionally does not decompose is expanded
/// explicitly, matching Jellyfin's diacritics library for those characters.
#[must_use]
pub fn remove_diacritics(text: &str) -> String {
    let mut output = String::with_capacity(text.len());

    for character in text
        .nfd()
        .filter(|character| !is_combining_mark(*character))
    {
        match character {
            '\u{fffd}' => {}
            'Æ' => output.push_str("AE"),
            'æ' => output.push_str("ae"),
            'Ð' | 'Đ' => output.push('D'),
            'ð' | 'đ' => output.push('d'),
            'Ħ' => output.push('H'),
            'ħ' => output.push('h'),
            'ı' => output.push('i'),
            'ĸ' => output.push('k'),
            'Ł' => output.push('L'),
            'ł' => output.push('l'),
            'Ŋ' => output.push('N'),
            'ŋ' => output.push('n'),
            'Ø' => output.push('O'),
            'ø' => output.push('o'),
            'Œ' => output.push_str("OE"),
            'œ' => output.push_str("oe"),
            'Þ' => output.push_str("TH"),
            'þ' => output.push_str("th"),
            _ => output.push(character),
        }
    }

    output.nfc().collect()
}

/// Reports whether [`remove_diacritics`] would change `text`.
#[must_use]
pub fn has_diacritics(text: &str) -> bool {
    remove_diacritics(text) != text
}

/// Jellyfin-style helpers implemented for UTF-8 strings.
pub trait StringExtensions {
    /// Removes diacritical marks while leaving non-Latin scripts intact.
    fn remove_diacritics(&self) -> String;

    /// Reports whether removing diacritics changes this string.
    fn has_diacritics(&self) -> bool;

    /// Counts occurrences of `needle` as a Unicode scalar value.
    fn count_char(&self, needle: char) -> usize;

    /// Returns everything before the first occurrence of `needle`.
    fn left_part(&self, needle: char) -> &str;

    /// Returns everything after the last occurrence of `needle`.
    fn right_part(&self, needle: char) -> &str;

    /// Returns everything before the first null character.
    fn truncate_at_null(&self) -> &str;

    /// Normalizes text for loose comparison and search.
    fn clean_value(&self) -> String;
}

impl StringExtensions for str {
    fn remove_diacritics(&self) -> String {
        remove_diacritics(self)
    }

    fn has_diacritics(&self) -> bool {
        has_diacritics(self)
    }

    fn count_char(&self, needle: char) -> usize {
        self.chars()
            .filter(|character| *character == needle)
            .count()
    }

    fn left_part(&self, needle: char) -> &str {
        self.find(needle).map_or(self, |position| &self[..position])
    }

    fn right_part(&self, needle: char) -> &str {
        self.rfind(needle)
            .map_or(self, |position| &self[position + needle.len_utf8()..])
    }

    fn truncate_at_null(&self) -> &str {
        self.left_part('\0')
    }

    fn clean_value(&self) -> String {
        if self.trim().is_empty() {
            return self.to_owned();
        }

        let mut output = String::with_capacity(self.len());
        let mut pending_space = false;

        for character in remove_diacritics(self).chars().flat_map(char::to_lowercase) {
            if character.is_alphanumeric() {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                pending_space = false;
                output.push(character);
            } else {
                pending_space = true;
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::{StringExtensions, has_diacritics, remove_diacritics};

    #[test]
    fn removes_diacritics_from_official_examples() {
        let cases = [
            ("", ""),
            ("Indiana Jones", "Indiana Jones"),
            ("a�b", "ab"),
            ("åäö", "aao"),
            ("Jön", "Jon"),
            ("Jönssonligan", "Jonssonligan"),
            ("Kieślowski", "Kieslowski"),
            ("Cidadão Kane", "Cidadao Kane"),
            ("운명처럼 널 사랑해", "운명처럼 널 사랑해"),
            ("애타는 로맨스", "애타는 로맨스"),
            ("Le cœur a ses raisons", "Le coeur a ses raisons"),
            ("Béla Tarr", "Bela Tarr"),
        ];

        for (input, expected) in cases {
            assert_eq!(expected, remove_diacritics(input));
        }
    }

    #[test]
    fn detects_diacritics_in_official_examples() {
        let cases = [
            ("", false),
            ("Indiana Jones", false),
            ("a�b", true),
            ("åäö", true),
            ("Jön", true),
            ("Jönssonligan", true),
            ("Kieślowski", true),
            ("Cidadão Kane", true),
            ("운명처럼 널 사랑해", false),
            ("애타는 로맨스", false),
            ("Le cœur a ses raisons", true),
            ("Béla Tarr", true),
        ];

        for (input, expected) in cases {
            assert_eq!(expected, has_diacritics(input));
        }
    }

    #[test]
    fn counts_characters_like_official_span_extension() {
        assert_eq!(0, "".count_char('_'));
        assert_eq!(3, "___".count_char('_'));
        assert_eq!(1, "test\0".count_char('\0'));
        assert_eq!(
            2,
            "Imdb=tt0119567|Tmdb=330|TmdbCollection=328".count_char('|')
        );
    }

    #[test]
    fn returns_left_part_from_official_examples() {
        assert_eq!("", "".left_part('q'));
        assert_eq!("Banana", "Banana split".left_part(' '));
        assert_eq!("Banana split", "Banana split".left_part('q'));
        assert_eq!("Banana", "Banana split 2".left_part(' '));
    }

    #[test]
    fn returns_right_part_from_official_examples() {
        assert_eq!("", "".right_part('q'));
        assert_eq!("split", "Banana split".right_part(' '));
        assert_eq!("Banana split", "Banana split".right_part('q'));
        assert_eq!("", "Banana split.".right_part('.'));
        assert_eq!("2", "Banana split 2".right_part(' '));
    }

    #[test]
    fn parts_are_safe_for_multibyte_needles() {
        assert_eq!("left", "left🙂middle🙂right".left_part('🙂'));
        assert_eq!("right", "left🙂middle🙂right".right_part('🙂'));
    }

    #[test]
    fn truncates_at_null() {
        assert_eq!("Jellyfin", "Jellyfin\0ignored".truncate_at_null());
        assert_eq!("Jellyfin", "Jellyfin".truncate_at_null());
    }

    #[test]
    fn cleans_values_for_comparison() {
        assert_eq!("bela tarr 2024", "  Béla--TARR (2024) ".clean_value());
        assert_eq!("   ", "   ".clean_value());
    }
}
