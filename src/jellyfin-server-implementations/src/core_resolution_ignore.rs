use jellyfin_naming::{ExtraRuleType, NamingOptions};

use crate::IgnorePatterns;

const THEME_SONG_FILE_NAME: &str = "theme";

/// File-system metadata required by [`CoreResolutionIgnoreRule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionFileSystemEntry {
    pub full_name: String,
    pub name: String,
    pub is_directory: bool,
}

impl ResolutionFileSystemEntry {
    #[must_use]
    pub fn new(full_name: impl Into<String>, is_directory: bool) -> Self {
        let full_name = full_name.into();
        let name = full_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned();
        Self {
            full_name,
            name,
            is_directory,
        }
    }
}

/// Parent item kinds that affect core resolution ignore rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionParentKind {
    BaseItem,
    Folder,
    AggregateFolder,
    UserRootFolder,
}

/// Explicit parent context for one resolution candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionParentContext {
    None,
    Item {
        kind: ResolutionParentKind,
        is_top_parent: bool,
    },
}

impl ResolutionParentContext {
    #[must_use]
    pub const fn item(kind: ResolutionParentKind) -> Self {
        Self::Item {
            kind,
            is_top_parent: false,
        }
    }

    #[must_use]
    pub const fn top_parent(kind: ResolutionParentKind) -> Self {
        Self::Item {
            kind,
            is_top_parent: true,
        }
    }
}

/// Applies Jellyfin's core file-system resolution ignore rules.
#[derive(Debug, Clone)]
pub struct CoreResolutionIgnoreRule {
    naming_options: NamingOptions,
    server_root_path: String,
}

impl CoreResolutionIgnoreRule {
    #[must_use]
    pub fn new(naming_options: NamingOptions, server_root_path: impl Into<String>) -> Self {
        Self {
            naming_options,
            server_root_path: server_root_path.into(),
        }
    }

    /// Returns whether the file-system entry should be skipped by resolvers.
    #[must_use]
    pub fn should_ignore(
        &self,
        file: &ResolutionFileSystemEntry,
        parent: ResolutionParentContext,
    ) -> bool {
        // The official rule intentionally uses case-sensitive substring
        // containment rather than normalized path ancestry.
        if file.full_name.contains(&self.server_root_path) {
            return false;
        }

        if IgnorePatterns::should_ignore(&file.full_name) {
            return true;
        }

        if file.is_directory
            && matches!(
                parent,
                ResolutionParentContext::Item {
                    kind: ResolutionParentKind::AggregateFolder,
                    ..
                } | ResolutionParentContext::Item {
                    is_top_parent: true,
                    ..
                }
            )
        {
            return false;
        }

        let ResolutionParentContext::Item { kind, .. } = parent else {
            return false;
        };

        if file.is_directory {
            return kind != ResolutionParentKind::UserRootFolder
                && self.naming_options.video_extra_rules.iter().any(|rule| {
                    rule.rule_type == ExtraRuleType::DirectoryName
                        && rule.token.eq_ignore_ascii_case(&file.name)
                });
        }

        self.is_theme_song(&file.name)
    }

    fn is_theme_song(&self, name: &str) -> bool {
        let (stem, extension) = split_file_name(name);
        let is_audio = extension.is_some_and(|extension| {
            self.naming_options
                .audio_file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        });

        is_audio && stem == THEME_SONG_FILE_NAME
    }
}

fn split_file_name(name: &str) -> (&str, Option<&str>) {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let Some(dot) = basename.rfind('.') else {
        return (basename, None);
    };

    (&basename[..dot], Some(&basename[dot..]))
}
