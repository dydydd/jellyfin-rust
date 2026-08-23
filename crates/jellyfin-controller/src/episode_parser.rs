use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeParseResult {
    pub series_name: Option<String>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub ending_episode_number: Option<i32>,
}

pub fn parse_episode(path: &Path) -> EpisodeParseResult {
    let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let grandparent = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());

    let mut result = EpisodeParseResult {
        series_name: None,
        season_number: None,
        episode_number: None,
        ending_episode_number: None,
    };

    // Pattern 1: S01E01 or S01E01-E02 (most common)
    if let Some(caps) = extract_sxee(filename) {
        result.season_number = Some(caps.0);
        result.episode_number = Some(caps.1);
        result.ending_episode_number = caps.2;
    }
    // Pattern 2: 1x01 or 1x01-02
    else if let Some(caps) = extract_xsep(filename) {
        result.season_number = Some(caps.0);
        result.episode_number = Some(caps.1);
        result.ending_episode_number = caps.2;
    }
    // Pattern 3: Season 1/01.mkv or /101.mkv
    else if let Some(ep) = extract_standalone_episode(filename) {
        result.episode_number = Some(ep);
    }

    // Parse season number from parent directory
    if result.season_number.is_none() {
        result.season_number = parse_season_directory(parent);
    }

    // Parse series name from grandparent directory
    if result.series_name.is_none()
        && result.season_number.is_some()
        && let Some(gp) = grandparent
        && !is_season_directory(gp)
    {
        result.series_name = Some(clean_series_name(gp));
    }

    // Fallback: parse series name from parent if parent is not a season dir
    if result.series_name.is_none()
        && result.season_number.is_some()
        && !is_season_directory(parent)
    {
        result.series_name = Some(clean_series_name(parent));
    }

    // Try to extract series name from filename (patterns like "Series Name - 101.mkv")
    if result.series_name.is_none()
        && result.episode_number.is_some()
        && let Some(name) = extract_series_name_from_filename(filename)
    {
        result.series_name = Some(name);
    }

    result
}

pub fn parse_season_directory(path: &str) -> Option<i32> {
    // Pattern: "Season 1", "season 01", "S01", "Staffel 1", etc.
    let lower = path.to_lowercase();

    // "Season N" / "Sæson N" / "Staffel N" etc.
    for prefix in &[
        "season ",
        "sæson ",
        "staffel ",
        "saison ",
        "stagione ",
        "säsong ",
        "seizoen ",
        "temporada ",
        "sezon ",
        "сезон ",
        "シーズン ",
        "serie ",
        "série ",
        "series ",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Ok(n) = rest.trim().parse::<i32>()
            && (1..=250).contains(&n)
        {
            return Some(n);
        }
    }

    // "S01" (letter S followed by digits, not followed by E)
    if lower.starts_with('s') && lower.len() >= 2 {
        let rest = &lower[1..];
        if let Some(idx) = rest.find(|c: char| !c.is_ascii_digit()) {
            let digits = &rest[..idx];
            if !rest[idx..].starts_with('e')
                && !digits.is_empty()
                && let Ok(n) = digits.parse::<i32>()
                && (1..=250).contains(&n)
            {
                return Some(n);
            }
        } else if !rest.is_empty()
            && let Ok(n) = rest.parse::<i32>()
            && (1..=250).contains(&n)
        {
            return Some(n);
        }
    }

    // Bare number directory
    if let Ok(n) = lower.trim().parse::<i32>()
        && (1..=250).contains(&n)
    {
        return Some(n);
    }

    // "Specials" directory
    if lower == "specials" || lower == "extras" || lower == "special" {
        return Some(0);
    }

    None
}

fn is_season_directory(name: &str) -> bool {
    parse_season_directory(name).is_some()
}

/// Extract S01E01 or S01E01-E02 pattern
fn extract_sxee(filename: &str) -> Option<(i32, i32, Option<i32>)> {
    let lower = filename.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let len = chars.len();

    // Find "s" followed by digits
    let s_pos = chars.iter().position(|&c| c == 's')?;

    // Read season digits after 's'
    let season_start = s_pos + 1;
    let mut season_end = season_start;
    while season_end < len && chars[season_end].is_ascii_digit() {
        season_end += 1;
    }
    if season_end == season_start {
        return None;
    }
    let season: i32 = chars[season_start..season_end]
        .iter()
        .collect::<String>()
        .parse()
        .ok()?;
    if season > 250 {
        return None;
    }

    // Must have 'e' after season digits
    if season_end >= len || chars[season_end] != 'e' {
        return None;
    }

    // Read episode digits after 'e'
    let ep_start = season_end + 1;
    let mut ep_end = ep_start;
    while ep_end < len && chars[ep_end].is_ascii_digit() {
        ep_end += 1;
    }
    if ep_end == ep_start {
        return None;
    }
    let episode: i32 = chars[ep_start..ep_end]
        .iter()
        .collect::<String>()
        .parse()
        .ok()?;

    // Check for ending episode (E02 or -E02)
    let mut ending = None;
    let remaining = &chars[ep_end..];
    let rem_str: String = remaining.iter().collect();

    // Look for "-E02" or "E02" pattern
    if let Some(e_pos) = rem_str.find('e') {
        let after_e = &rem_str[e_pos + 1..];
        let digits: String = after_e.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && let Ok(n) = digits.parse::<i32>()
        {
            ending = Some(n);
        }
    } else if let Some(hyphen) = rem_str.find('-') {
        let after = &rem_str[hyphen + 1..];
        // Check for pattern like -02
        if after.len() <= 3
            && !after.is_empty()
            && let Ok(n) = after.parse::<i32>()
            && n > episode
            && n < 1000
        {
            ending = Some(n);
        }
    }

    Some((season, episode, ending))
}

/// Extract 1x01 or 1x01-02 pattern
fn extract_xsep(filename: &str) -> Option<(i32, i32, Option<i32>)> {
    let lower = filename.to_lowercase();
    let x_pos = lower.find('x')?;

    if x_pos == 0 {
        return None;
    }

    let before_x: String = lower[..x_pos]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let season: i32 = before_x.parse().ok()?;
    if season > 250 || season == 0 {
        return None;
    }

    let after_x = &lower[x_pos + 1..];
    let ep_digits: String = after_x.chars().take_while(char::is_ascii_digit).collect();
    let episode: i32 = ep_digits.parse().ok()?;
    if episode > 500 {
        return None;
    }

    // Check for ending episode
    let remaining = &after_x[ep_digits.len()..];
    let mut ending = None;
    if let Some(after) = remaining.strip_prefix('-') {
        let end_digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !end_digits.is_empty()
            && let Ok(n) = end_digits.parse::<i32>()
            && n > episode
            && n < 1000
        {
            ending = Some(n);
        }
    }

    Some((season, episode, ending))
}

/// Extract standalone episode number from filename (e.g. "01.mkv", "Episode 1.mkv")
fn extract_standalone_episode(filename: &str) -> Option<i32> {
    let lower = filename.to_lowercase();

    // "Episode 1", "ep 1"
    for prefix in &["episode ", "ep "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty()
                && let Ok(n) = digits.parse::<i32>()
                && (1..=500).contains(&n)
            {
                return Some(n);
            }
        }
    }

    // Bare number (e.g., "01.mkv") - only for files in a season directory
    if !lower.contains(|c: char| c.is_alphabetic())
        || lower
            .split('.')
            .next()
            .unwrap_or("")
            .chars()
            .all(|c| c.is_ascii_digit())
    {
        let stem = filename;
        let digits: String = stem.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && let Ok(n) = digits.parse::<i32>()
            && (1..=500).contains(&n)
        {
            return Some(n);
        }
    }

    None
}

/// Extract series name from filename patterns like "Series.Name - 101.mkv"
fn extract_series_name_from_filename(filename: &str) -> Option<String> {
    let lower = filename.to_lowercase();

    // Look for " - " separator before the episode number
    if let Some(sep_pos) = lower.rfind(" - ") {
        let before_sep = &filename[..sep_pos];
        let after_sep = &lower[sep_pos + 3..];

        // After " - " should start with digits (episode number)
        if after_sep.starts_with(|c: char| c.is_ascii_digit()) {
            let name = before_sep.trim();
            if !name.is_empty() && name.len() > 1 {
                return Some(name.to_owned());
            }
        }
    }

    None
}

/// Clean series name: replace dots and underscores with spaces
fn clean_series_name(name: &str) -> String {
    let cleaned = name.replace(['.', '_', '-'], " ");
    // Collapse multiple spaces
    let mut result = String::with_capacity(cleaned.len());
    let mut prev_space = false;
    for c in cleaned.chars() {
        if c == ' ' {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_standard_sxee() {
        let result = parse_episode(Path::new("/tv/My.Show/Season 1/S01E01.mkv"));
        assert_eq!(result.season_number, Some(1));
        assert_eq!(result.episode_number, Some(1));
        assert_eq!(result.series_name, Some("My Show".to_owned()));
    }

    #[test]
    fn parse_multi_episode() {
        let result = parse_episode(Path::new("/tv/Show/Season 2/S02E03-E04.mkv"));
        assert_eq!(result.season_number, Some(2));
        assert_eq!(result.episode_number, Some(3));
        assert_eq!(result.ending_episode_number, Some(4));
    }

    #[test]
    fn parse_x_separator() {
        let result = parse_episode(Path::new("/tv/Show/Season 1/1x05.mkv"));
        assert_eq!(result.season_number, Some(1));
        assert_eq!(result.episode_number, Some(5));
    }

    #[test]
    fn detect_season_dirs() {
        assert_eq!(parse_season_directory("Season 1"), Some(1));
        assert_eq!(parse_season_directory("S02"), Some(2));
        assert_eq!(parse_season_directory("specials"), Some(0));
        assert_eq!(parse_season_directory("Staffel 3"), Some(3));
        assert_eq!(parse_season_directory("Extras"), Some(0));
    }

    #[test]
    fn parse_with_year_in_series() {
        let result = parse_episode(Path::new("/tv/Show (2022)/Season 1/S01E01.mkv"));
        assert_eq!(result.season_number, Some(1));
        assert_eq!(result.episode_number, Some(1));
        assert_eq!(result.series_name, Some("Show (2022)".to_owned()));
    }

    #[test]
    fn clean_series_name_test() {
        assert_eq!(clean_series_name("My.Show"), "My Show");
        assert_eq!(clean_series_name("My_Show"), "My Show");
        assert_eq!(clean_series_name("Show Name"), "Show Name");
    }
}
