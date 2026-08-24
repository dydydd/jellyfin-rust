use std::io;

use jellyfin_naming::{
    ExtraResolver, ExtraRule, ExtraRuleType, ExtraType, MediaType, NamingOptions, StackFileInfo,
    StackResolver, VideoFileInfo, VideoResolver,
};

/// Library item kinds that can own local extras.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraOwnerKind {
    Movie,
    Series,
    Video,
}

/// Owner metadata needed to associate local extras with one library item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraOwner {
    pub path: String,
    pub name: String,
    pub kind: ExtraOwnerKind,
    pub is_folder: bool,
    pub is_disc: bool,
}

impl ExtraOwner {
    #[must_use]
    pub fn new(path: impl Into<String>, name: impl Into<String>, kind: ExtraOwnerKind) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            kind,
            is_folder: false,
            is_disc: false,
        }
    }

    #[must_use]
    pub const fn with_folder(mut self) -> Self {
        self.is_folder = true;
        self
    }

    #[must_use]
    pub const fn with_disc(mut self) -> Self {
        self.is_disc = true;
        self
    }
}

/// File-system metadata consumed while finding extras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraFileSystemEntry {
    pub full_name: String,
    pub name: String,
    pub is_directory: bool,
}

impl ExtraFileSystemEntry {
    #[must_use]
    pub fn new(full_name: impl Into<String>, is_directory: bool) -> Self {
        let full_name = full_name.into();
        let name = file_name(&full_name).to_owned();
        Self {
            full_name,
            name,
            is_directory,
        }
    }

    #[must_use]
    pub fn with_name(
        full_name: impl Into<String>,
        name: impl Into<String>,
        is_directory: bool,
    ) -> Self {
        Self {
            full_name: full_name.into(),
            name: name.into(),
            is_directory,
        }
    }
}

/// Directory access used for recognized extras folders.
pub trait ExtraDirectoryReader {
    /// Lists the files immediately inside `path`.
    ///
    /// # Errors
    ///
    /// Returns the underlying file-system error when the directory cannot be read.
    fn get_files(&self, path: &str) -> io::Result<Vec<ExtraFileSystemEntry>>;
}

impl<F> ExtraDirectoryReader for F
where
    F: Fn(&str) -> io::Result<Vec<ExtraFileSystemEntry>>,
{
    fn get_files(&self, path: &str) -> io::Result<Vec<ExtraFileSystemEntry>> {
        self(path)
    }
}

/// Concrete library item type produced for an extra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtraMediaKind {
    Audio,
    Trailer,
    Video,
}

/// A resolved local extra belonging to one library item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLibraryExtra {
    pub path: String,
    pub name: String,
    pub production_year: Option<u16>,
    pub media_kind: ExtraMediaKind,
    pub extra_type: ExtraType,
    pub is_in_mixed_folder: bool,
}

/// Aggregates suffix-based and extras-directory media for a library owner.
#[derive(Debug, Clone)]
pub struct LibraryExtrasResolver {
    naming_options: NamingOptions,
    library_root: Option<String>,
}

impl LibraryExtrasResolver {
    #[must_use]
    pub fn new(naming_options: NamingOptions) -> Self {
        Self {
            naming_options,
            library_root: None,
        }
    }

    #[must_use]
    pub fn with_library_root(
        naming_options: NamingOptions,
        library_root: impl Into<String>,
    ) -> Self {
        Self {
            naming_options,
            library_root: Some(library_root.into()),
        }
    }

    /// Finds extras among the owner's known children and recognized extras folders.
    ///
    /// # Errors
    ///
    /// Returns an error when [`ExtraDirectoryReader`] cannot read a recognized extras folder.
    pub fn find_extras<R: ExtraDirectoryReader>(
        &self,
        owner: &ExtraOwner,
        file_system_children: &[ExtraFileSystemEntry],
        directory_reader: &R,
    ) -> io::Result<Vec<ResolvedLibraryExtra>> {
        let owner_is_directory = owner.is_folder || owner.is_disc;
        let Some(owner_video_info) = VideoResolver::resolve_with_library_root(
            Some(&owner.path),
            owner_is_directory,
            &self.naming_options,
            self.library_root.as_deref(),
        ) else {
            return Ok(Vec::new());
        };
        let owner_stack_parts = self.owner_stack_parts(owner, file_system_children);
        let mut extras = Vec::new();

        for current in file_system_children {
            if current.is_directory && self.is_extras_directory(&current.name) {
                let files = directory_reader.get_files(&current.full_name)?;
                let is_in_mixed_folder = files.len() > 1;
                for file in files.iter().filter(|file| !file.is_directory) {
                    if let Some(extra) =
                        self.resolve_for_owner(file, &owner_video_info, is_in_mixed_folder)
                    {
                        extras.push(extra);
                    }
                }
            } else if !current.is_directory
                && !contains_path(&owner_stack_parts, &current.full_name)
                && let Some(extra) = self.resolve_for_owner(current, &owner_video_info, false)
            {
                extras.push(extra);
            }
        }

        Ok(extras)
    }

    fn resolve_for_owner(
        &self,
        file: &ExtraFileSystemEntry,
        owner: &VideoFileInfo,
        is_in_mixed_folder: bool,
    ) -> Option<ResolvedLibraryExtra> {
        let (extra_type, rule) = self.extra_type_for_owner(&file.full_name, owner)?;
        let (name, production_year, media_kind) = match rule.media_type {
            MediaType::Audio => {
                let parsed = VideoResolver::clean_date_time(
                    file_stem(file_name(&file.full_name)),
                    &self.naming_options,
                );
                let name =
                    VideoResolver::try_clean_string(Some(&parsed.name), &self.naming_options)
                        .unwrap_or(parsed.name);
                (name, parsed.year, ExtraMediaKind::Audio)
            }
            MediaType::Video => {
                let video = VideoResolver::resolve_file_with_library_root(
                    Some(&file.full_name),
                    &self.naming_options,
                    self.library_root.as_deref(),
                )?;
                let kind = if extra_type == ExtraType::Trailer {
                    ExtraMediaKind::Trailer
                } else {
                    ExtraMediaKind::Video
                };
                (video.name, video.year, kind)
            }
        };

        Some(ResolvedLibraryExtra {
            path: file.full_name.clone(),
            name,
            production_year,
            media_kind,
            extra_type,
            is_in_mixed_folder,
        })
    }

    fn extra_type_for_owner(
        &self,
        path: &str,
        owner: &VideoFileInfo,
    ) -> Option<(ExtraType, ExtraRule)> {
        let extra = ExtraResolver::resolve_with_library_root(
            path,
            &self.naming_options,
            self.library_root.as_deref(),
        );
        let (extra_type, rule) = (extra.extra_type?, extra.rule?);
        let parsed =
            VideoResolver::clean_date_time(file_stem(file_name(path)), &self.naming_options);
        let owner_file_name =
            trim_filename_delimiters(owner.file_name_without_extension(), &self.naming_options);
        let owner_name = trim_filename_delimiters(&owner.name, &self.naming_options);
        let extra_name = trim_filename_delimiters(&parsed.name, &self.naming_options);

        let names_match = starts_with_ignore_ascii_case(extra_name, owner_file_name)
            || (starts_with_ignore_ascii_case(extra_name, owner_name) && parsed.year == owner.year);
        if !names_match {
            let current_parent = if rule.rule_type == ExtraRuleType::DirectoryName {
                parent_path(parent_path(path))
            } else {
                parent_path(path)
            };
            let owner_parent = if owner.is_directory {
                owner.path.as_str()
            } else {
                parent_path(&owner.path)
            };
            if current_parent.is_empty()
                || owner_parent.is_empty()
                || !normalized_path(current_parent)
                    .eq_ignore_ascii_case(normalized_path(owner_parent))
            {
                return None;
            }
        }

        Some((extra_type, rule))
    }

    fn is_extras_directory(&self, name: &str) -> bool {
        self.naming_options.video_extra_rules.iter().any(|rule| {
            rule.rule_type == ExtraRuleType::DirectoryName && rule.token.eq_ignore_ascii_case(name)
        })
    }

    fn owner_stack_parts(
        &self,
        owner: &ExtraOwner,
        file_system_children: &[ExtraFileSystemEntry],
    ) -> Vec<String> {
        if owner.is_folder || owner.is_disc {
            return vec![owner.path.clone()];
        }

        let mut files = Vec::with_capacity(file_system_children.len() + 1);
        files.push(StackFileInfo::new(&owner.path, false));
        files.extend(
            file_system_children
                .iter()
                .filter(|entry| !entry.is_directory)
                .map(|entry| StackFileInfo::new(&entry.full_name, false)),
        );
        StackResolver::resolve(&files, &self.naming_options)
            .into_iter()
            .find(|stack| stack.contains_file(&owner.path, false))
            .map_or_else(|| vec![owner.path.clone()], |stack| stack.files)
    }
}

fn trim_filename_delimiters<'a>(name: &'a str, options: &NamingOptions) -> &'a str {
    name.trim_end()
        .trim_end_matches(|character| options.video_flag_delimiters.contains(&character))
        .trim_end()
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    !prefix.is_empty()
        && value
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn contains_path(paths: &[String], path: &str) -> bool {
    paths
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(path))
}

fn normalized_path(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

fn parent_path(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rfind(['/', '\\'])
        .map_or("", |index| &path[..index])
}

fn file_stem(path: &str) -> &str {
    path.rfind('.').map_or(path, |index| &path[..index])
}
