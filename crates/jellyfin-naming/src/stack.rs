use regex::{Regex, RegexBuilder};

use crate::{NamingOptions, VideoResolver};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStackRuleResult {
    pub stack_name: String,
    pub part_type: String,
    pub part_number: String,
}

#[derive(Clone, Debug)]
pub struct FileStackRule {
    regex: Regex,
    pub is_numerical: bool,
}

impl FileStackRule {
    #[must_use]
    pub fn new(expression: &str, is_numerical: bool) -> Self {
        Self {
            regex: RegexBuilder::new(expression)
                .case_insensitive(true)
                .build()
                .expect("file stack expression must be valid"),
            is_numerical,
        }
    }

    #[must_use]
    pub fn parse(&self, input: &str) -> Option<FileStackRuleResult> {
        let captures = self.regex.captures(input)?;
        let mut stack_name = captures.name("filename")?.as_str().to_owned();
        if let Some(separator) = captures.name("separator") {
            let separator = separator.as_str();
            if separator.len() == 1 && matches!(separator.as_bytes()[0], b']' | b')' | b'}') {
                stack_name.push_str(separator);
            }
        }
        Some(FileStackRuleResult {
            stack_name,
            part_type: captures
                .name("parttype")
                .map_or_else(|| "unknown".to_owned(), |value| value.as_str().to_owned()),
            part_number: captures.name("number")?.as_str().to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackFileInfo {
    pub path: String,
    pub name: Option<String>,
    pub is_directory: bool,
}

impl StackFileInfo {
    #[must_use]
    pub fn new(path: impl Into<String>, is_directory: bool) -> Self {
        Self {
            path: path.into(),
            name: None,
            is_directory,
        }
    }

    #[must_use]
    pub fn with_name(path: impl Into<String>, name: impl Into<String>, is_directory: bool) -> Self {
        Self {
            path: path.into(),
            name: Some(name.into()),
            is_directory,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStack {
    pub name: String,
    pub files: Vec<String>,
    pub is_directory_stack: bool,
}

impl FileStack {
    #[must_use]
    pub fn contains_file(&self, path: &str, is_directory: bool) -> bool {
        !path.is_empty()
            && self.is_directory_stack == is_directory
            && self
                .files
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(path))
    }
}

pub struct StackResolver;

impl StackResolver {
    #[must_use]
    pub fn resolve_files<S: AsRef<str>>(files: &[S], options: &NamingOptions) -> Vec<FileStack> {
        let entries = files
            .iter()
            .map(|path| StackFileInfo::new(path.as_ref(), false))
            .collect::<Vec<_>>();
        Self::resolve(&entries, options)
    }

    #[must_use]
    pub fn resolve_directories<S: AsRef<str>>(
        files: &[S],
        options: &NamingOptions,
    ) -> Vec<FileStack> {
        let entries = files
            .iter()
            .map(|path| StackFileInfo::new(path.as_ref(), true))
            .collect::<Vec<_>>();
        Self::resolve(&entries, options)
    }

    #[must_use]
    pub fn resolve(files: &[StackFileInfo], options: &NamingOptions) -> Vec<FileStack> {
        let mut files = files
            .iter()
            .filter(|file| {
                file.is_directory
                    || VideoResolver::is_video_file(&file.path, options)
                    || VideoResolver::is_stub_file(&file.path, options)
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let mut candidates: Vec<StackCandidate> = Vec::new();
        for file in files {
            let name = file
                .name
                .as_deref()
                .unwrap_or_else(|| file_name(&file.path));
            for rule in &options.video_file_stacking_rules {
                let Some(parsed) = rule.parse(name) else {
                    continue;
                };
                let directory = parent_path(&file.path);
                let existing = candidates.iter().position(|candidate| {
                    candidate.name == parsed.stack_name
                        && candidate.directory == directory
                        && candidate.is_directory == file.is_directory
                });
                if let Some(index) = existing {
                    let candidate = &mut candidates[index];
                    if !candidate.part_type.eq_ignore_ascii_case(&parsed.part_type)
                        || candidate.contains_part(&parsed.part_number)
                    {
                        continue;
                    }
                    if candidate.is_numerical != rule.is_numerical {
                        break;
                    }
                    candidate
                        .parts
                        .push((parsed.part_number, file.path.clone()));
                } else {
                    candidates.push(StackCandidate {
                        name: parsed.stack_name,
                        directory: directory.to_owned(),
                        part_type: parsed.part_type,
                        is_numerical: rule.is_numerical,
                        is_directory: file.is_directory,
                        parts: vec![(parsed.part_number, file.path.clone())],
                    });
                }
                break;
            }
        }

        candidates
            .into_iter()
            .filter(|candidate| candidate.parts.len() > 1)
            .map(|candidate| FileStack {
                name: candidate.name,
                files: candidate.parts.into_iter().map(|(_, path)| path).collect(),
                is_directory_stack: candidate.is_directory,
            })
            .collect()
    }
}

struct StackCandidate {
    name: String,
    directory: String,
    part_type: String,
    is_numerical: bool,
    is_directory: bool,
    parts: Vec<(String, String)>,
}

impl StackCandidate {
    fn contains_part(&self, part_number: &str) -> bool {
        self.parts
            .iter()
            .any(|(candidate, _)| candidate.eq_ignore_ascii_case(part_number))
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent_path(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |index| &path[..index])
}
