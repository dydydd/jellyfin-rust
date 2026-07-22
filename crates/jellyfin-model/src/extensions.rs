/// Uppercases the first UTF-16 code unit when it is a lowercase character.
///
/// This preserves the remainder byte-for-byte and avoids multi-character
/// uppercase expansions, matching Jellyfin's `char.ToUpperInvariant` logic.
#[must_use]
pub fn first_to_upper(input: &str) -> String {
    let Some(first) = input.chars().next() else {
        return String::new();
    };
    if first.len_utf16() != 1 || !first.is_lowercase() {
        return input.to_owned();
    }

    let mut uppercase = first.to_uppercase();
    let Some(replacement) = uppercase.next() else {
        return input.to_owned();
    };
    if uppercase.next().is_some() || replacement.len_utf16() != 1 {
        return input.to_owned();
    }

    let mut result = String::with_capacity(input.len().max(replacement.len_utf8()));
    result.push(replacement);
    result.push_str(&input[first.len_utf8()..]);
    result
}
