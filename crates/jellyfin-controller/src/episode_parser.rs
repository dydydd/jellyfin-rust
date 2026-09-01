use std::path::Path;

use jellyfin_naming::{EpisodePathParser, NamingOptions, SeasonPathParser, SeriesResolver};

#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeParseResult {
    pub series_name: Option<String>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub ending_episode_number: Option<i32>,
}

pub fn parse_episode(path: &Path) -> EpisodeParseResult {
    let mut result = EpisodeParseResult {
        series_name: None,
        season_number: None,
        episode_number: None,
        ending_episode_number: None,
    };

    let path_str = path.to_string_lossy();
    let options = NamingOptions::default();
    let parsed = EpisodePathParser::new(options.clone()).parse(&path_str, false);
    result.season_number = parsed.season_number;
    result.episode_number = parsed.episode_number;
    result.ending_episode_number = parsed.ending_episode_number;

    if result.season_number.is_none() {
        result.season_number = path
            .parent()
            .and_then(Path::to_str)
            .and_then(parse_season_directory);
    }

    let series_name = parsed
        .series_name
        .filter(|name| !name.trim().is_empty())
        .map(|name| clean_series_name(&name))
        .or_else(|| {
            series_name_from_directory(path, &options).map(|name| clean_series_name(&name))
        });

    if result.season_number.is_none() && result.episode_number.is_some() {
        result.season_number = Some(1);
    }

    result.series_name = series_name;
    result
}

pub fn parse_season_directory(path: &str) -> Option<i32> {
    SeasonPathParser::parse(path, None, true, true).season_number
}

fn is_season_directory(name: &str) -> bool {
    parse_season_directory(name).is_some()
}

fn series_name_from_directory(path: &Path, options: &NamingOptions) -> Option<String> {
    let parent = path.parent()?;
    let parent_name = parent.file_name()?.to_str()?;
    let series_path = if is_season_directory(parent_name) {
        parent.parent()?
    } else {
        parent
    };
    SeriesResolver::resolve(options, &series_path.to_string_lossy())
        .name
        .filter(|name| !name.trim().is_empty())
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
        assert_eq!(result.series_name, Some("Show".to_owned()));
    }

    #[test]
    fn parse_flat_series_episode() {
        let result = parse_episode(Path::new("/tv/Show Name/Show Name - S01E04.mkv"));
        assert_eq!(result.series_name.as_deref(), Some("Show Name"));
        assert_eq!(result.season_number, Some(1));
        assert_eq!(result.episode_number, Some(4));
    }

    #[test]
    fn parse_standalone_number_in_season_directory() {
        let result = parse_episode(Path::new("/tv/Show Name/Season 01/04.mkv"));
        assert_eq!(result.series_name.as_deref(), Some("Show Name"));
        assert_eq!(result.season_number, Some(1));
        assert_eq!(result.episode_number, Some(4));
    }

    #[test]
    fn clean_series_name_test() {
        assert_eq!(clean_series_name("My.Show"), "My Show");
        assert_eq!(clean_series_name("My_Show"), "My Show");
        assert_eq!(clean_series_name("Show Name"), "Show Name");
    }
}
