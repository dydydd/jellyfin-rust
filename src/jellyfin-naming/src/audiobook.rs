use std::{cmp::Ordering, sync::Arc};

use crate::{NamingOptions, StackFileInfo, StackResolver};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioBookFileInfo {
    pub path: String,
    pub container: String,
    pub part_number: Option<u32>,
    pub chapter_number: Option<u32>,
}

impl AudioBookFileInfo {
    #[must_use]
    pub fn new(path: impl Into<String>, container: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            container: container.into(),
            part_number: None,
            chapter_number: None,
        }
    }

    #[must_use]
    pub fn with_numbers(
        path: impl Into<String>,
        container: impl Into<String>,
        part_number: Option<u32>,
        chapter_number: Option<u32>,
    ) -> Self {
        Self {
            path: path.into(),
            container: container.into(),
            part_number,
            chapter_number,
        }
    }

    #[must_use]
    pub fn compare_to(&self, other: Option<&Self>) -> i32 {
        other.map_or(1, |other| match self.cmp(other) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        })
    }
}

impl Ord for AudioBookFileInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.chapter_number
            .cmp(&other.chapter_number)
            .then_with(|| self.part_number.cmp(&other.part_number))
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for AudioBookFileInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioBookFilePathParserResult {
    pub part_number: Option<u32>,
    pub chapter_number: Option<u32>,
}

pub struct AudioBookFilePathParser {
    options: NamingOptions,
}

impl AudioBookFilePathParser {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn parse(&self, path: &str) -> AudioBookFilePathParserResult {
        Self::parse_with(path, &self.options)
    }

    fn parse_with(path: &str, options: &NamingOptions) -> AudioBookFilePathParserResult {
        let stem = file_stem(file_name(path));
        let mut result = AudioBookFilePathParserResult::default();
        for expression in &options.audio_book_parts_regexes {
            let Some(captures) = expression.captures(stem).ok().flatten() else {
                continue;
            };
            if result.chapter_number.is_none() {
                result.chapter_number = captures
                    .name("chapter")
                    .and_then(|value| value.as_str().parse().ok());
            }
            if result.part_number.is_none()
                && let Some(part) = captures.name("part")
            {
                // C# excludes a trailing number preceded by "chapter " or
                // "ch " with `(?<!ch(?:apter) )`; fancy-regex's lookbehind
                // is unreliable here, so keep the equivalent fixed-width check.
                let prefix = stem[..part.start()].to_ascii_lowercase();
                if !(prefix.ends_with("chapter ") || prefix.ends_with("ch ")) {
                    result.part_number = part.as_str().parse().ok();
                }
            }
        }
        result
    }
}

pub struct AudioBookResolver {
    options: NamingOptions,
}

impl AudioBookResolver {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn resolve(&self, path: &str) -> Option<AudioBookFileInfo> {
        Self::resolve_with(path, &self.options)
    }

    fn resolve_with(path: &str, options: &NamingOptions) -> Option<AudioBookFileInfo> {
        if path.is_empty() {
            return None;
        }
        let filename = file_name(path);
        let stem = file_stem(filename);
        if stem.is_empty() {
            return None;
        }
        let extension = extension(filename)?;
        if !options
            .audio_file_extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        {
            return None;
        }
        let parsed = AudioBookFilePathParser::parse_with(path, options);
        Some(AudioBookFileInfo::with_numbers(
            path,
            extension.trim_start_matches('.'),
            parsed.part_number,
            parsed.chapter_number,
        ))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudioBookNameParserResult {
    pub name: String,
    pub year: Option<u16>,
}

pub struct AudioBookNameParser {
    options: NamingOptions,
}

impl AudioBookNameParser {
    #[must_use]
    pub fn new(options: NamingOptions) -> Self {
        Self { options }
    }

    #[must_use]
    pub fn parse(&self, value: &str) -> AudioBookNameParserResult {
        Self::parse_with(value, &self.options)
    }

    fn parse_with(value: &str, options: &NamingOptions) -> AudioBookNameParserResult {
        let mut result = AudioBookNameParserResult::default();
        let mut found_name = false;
        for expression in &options.audio_book_name_regexes {
            let Some(captures) = expression.captures(value).ok().flatten() else {
                continue;
            };
            if !found_name && let Some(name) = captures.name("name") {
                result.name = name.as_str().to_owned();
                found_name = true;
            }
            if result.year.is_none() {
                result.year = captures
                    .name("year")
                    .and_then(|year| year.as_str().parse().ok());
            }
        }
        if !found_name || result.name.is_empty() {
            result.name = value.to_owned();
        }
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioBookInfo {
    pub name: String,
    pub year: Option<u16>,
    pub files: Vec<AudioBookFileInfo>,
    pub extras: Vec<AudioBookFileInfo>,
    pub alternate_versions: Vec<AudioBookFileInfo>,
}

pub struct AudioBookListResolver {
    options: Arc<NamingOptions>,
}

impl AudioBookListResolver {
    #[must_use]
    pub fn new(options: impl Into<Arc<NamingOptions>>) -> Self {
        Self {
            options: options.into(),
        }
    }

    #[must_use]
    pub fn resolve_paths<S: AsRef<str>>(&self, paths: &[S]) -> Vec<AudioBookInfo> {
        let files = paths
            .iter()
            .map(|path| StackFileInfo::new(path.as_ref(), false))
            .collect::<Vec<_>>();
        self.resolve(&files)
    }

    #[must_use]
    pub fn resolve(&self, files: &[StackFileInfo]) -> Vec<AudioBookInfo> {
        let resolver = &self.options;
        let resolved = files
            .iter()
            .filter_map(|file| AudioBookResolver::resolve_with(&file.path, resolver))
            .collect::<Vec<_>>();
        StackResolver::resolve_audio_books(resolved)
            .into_iter()
            .map(|stack| {
                let mut stack_files = stack
                    .files
                    .iter()
                    .filter_map(|path| AudioBookResolver::resolve_with(path, resolver))
                    .collect::<Vec<_>>();
                stack_files.sort();
                let parsed_name = AudioBookNameParser::parse_with(&stack.name, &self.options);
                let (files, extras, alternate_versions) =
                    organize_files(stack_files, &parsed_name.name);
                AudioBookInfo {
                    name: parsed_name.name,
                    year: parsed_name.year,
                    files,
                    extras,
                    alternate_versions,
                }
            })
            .collect()
    }
}

fn organize_files(
    files: Vec<AudioBookFileInfo>,
    book_name: &str,
) -> (
    Vec<AudioBookFileInfo>,
    Vec<AudioBookFileInfo>,
    Vec<AudioBookFileInfo>,
) {
    type Key = (Option<u32>, Option<u32>);
    let has_numbered_files = files
        .iter()
        .any(|file| file.chapter_number.is_some() || file.part_number.is_some());
    let mut groups: Vec<(Key, Vec<AudioBookFileInfo>)> = Vec::new();
    for file in files {
        let key = (file.chapter_number, file.part_number);
        if let Some((_, group)) = groups.iter_mut().find(|(candidate, _)| *candidate == key) {
            group.push(file);
        } else {
            groups.push((key, vec![file]));
        }
    }

    let mut main_files = Vec::new();
    let mut extras = Vec::new();
    let mut alternatives = Vec::new();
    let dotted_name = book_name.replace(' ', ".");
    for (key, mut group) in groups {
        if key == (None, None) && (group.len() > 1 || has_numbered_files) {
            let (mut candidates, mut group_extras): (Vec<_>, Vec<_>) =
                group.into_iter().partition(|file| {
                    let stem = file_stem(file_name(&file.path));
                    stem.eq_ignore_ascii_case("audiobook")
                        || contains_ignore_ascii_case(stem, book_name)
                        || contains_ignore_ascii_case(stem, &dotted_name)
                });
            group_extras.sort_by(compare_container_and_path);
            extras.extend(group_extras);
            if !candidates.is_empty() {
                candidates.sort_by(compare_container_and_path);
                let main_index = candidates
                    .iter()
                    .position(|file| {
                        file_stem(file_name(&file.path)).eq_ignore_ascii_case(book_name)
                    })
                    .or_else(|| {
                        candidates.iter().position(|file| {
                            file_stem(file_name(&file.path)).eq_ignore_ascii_case("audiobook")
                        })
                    })
                    .unwrap_or(0);
                main_files.push(candidates.remove(main_index));
                alternatives.extend(candidates);
            }
        } else if group.len() > 1 {
            group.sort_by(compare_container_and_path);
            main_files.push(group.remove(0));
            alternatives.extend(group);
        } else {
            main_files.extend(group);
        }
    }
    (main_files, extras, alternatives)
}

fn compare_container_and_path(left: &AudioBookFileInfo, right: &AudioBookFileInfo) -> Ordering {
    left.container
        .cmp(&right.container)
        .then_with(|| left.path.cmp(&right.path))
}

fn contains_ignore_ascii_case(value: &str, pattern: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(&pattern.to_ascii_lowercase())
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn extension(path: &str) -> Option<&str> {
    let index = path.rfind('.')?;
    (index + 1 < path.len()).then_some(&path[index..])
}

fn file_stem(path: &str) -> &str {
    path.rfind('.').map_or(path, |index| &path[..index])
}
