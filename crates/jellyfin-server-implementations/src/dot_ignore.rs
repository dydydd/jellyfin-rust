use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::SystemTime,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

const IGNORE_FILE_NAME: &str = ".ignore";

/// File-system metadata required by the dot-ignore rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DotIgnoreFileSystemEntry {
    pub full_name: PathBuf,
    pub is_directory: bool,
}

impl DotIgnoreFileSystemEntry {
    #[must_use]
    pub fn new(full_name: impl Into<PathBuf>, is_directory: bool) -> Self {
        Self {
            full_name: full_name.into(),
            is_directory,
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedIgnoreCacheEntry {
    rules: Gitignore,
    modified: SystemTime,
    length: u64,
    is_empty: bool,
}

/// Resolves hierarchical .ignore files with thread-safe lookup and rule caches.
#[derive(Debug)]
pub struct DotIgnoreIgnoreRule {
    directory_cache: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
    rules_cache: Mutex<HashMap<PathBuf, ParsedIgnoreCacheEntry>>,
    directory_cache_capacity: usize,
    rules_cache_capacity: usize,
}

impl Default for DotIgnoreIgnoreRule {
    fn default() -> Self {
        Self::new()
    }
}

impl DotIgnoreIgnoreRule {
    #[must_use]
    pub fn new() -> Self {
        let processors = std::thread::available_parallelism().map_or(1, usize::from);
        let directory_cache_capacity = processors.saturating_mul(100).max(100);
        Self {
            directory_cache: Mutex::new(HashMap::new()),
            rules_cache: Mutex::new(HashMap::new()),
            directory_cache_capacity,
            rules_cache_capacity: (directory_cache_capacity / 4).max(32),
        }
    }

    /// Checks one entry against the nearest .ignore in its directory ancestry.
    ///
    /// A missing .ignore, including one deleted after its path was cached,
    /// returns false. Empty, whitespace-only, or entirely invalid rule files
    /// ignore every entry.
    ///
    /// # Errors
    ///
    /// Returns the underlying metadata or file-read error. In particular,
    /// unreadable and non-UTF-8 .ignore files are not treated as absent.
    pub fn should_ignore(&self, file: &DotIgnoreFileSystemEntry) -> io::Result<bool> {
        let search_directory = if file.is_directory {
            file.full_name.as_path()
        } else {
            file.full_name.parent().unwrap_or_else(|| Path::new(""))
        };
        if search_directory.as_os_str().is_empty() {
            return Ok(false);
        }

        let Some(ignore_file) = self.find_ignore_file_cached(search_directory)? else {
            return Ok(false);
        };
        let Some(parsed) = self.get_parsed_rules(&ignore_file)? else {
            lock_unpoisoned(&self.directory_cache).remove(search_directory);
            return Ok(false);
        };
        if parsed.is_empty {
            return Ok(true);
        }

        Ok(is_ignored(
            &parsed.rules,
            &path_to_check(&file.full_name, cfg!(windows)),
            file.is_directory,
        ))
    }

    /// Clears cached directory-to-ignore-file lookups.
    ///
    /// Parsed rules remain cached and are still validated by modification time
    /// and file length before reuse.
    pub fn clear_directory_cache(&self) {
        lock_unpoisoned(&self.directory_cache).clear();
    }

    /// Checks a path against an in-memory set of .ignore rules.
    #[must_use]
    pub fn check_ignore_rules<S: AsRef<str>>(
        path: &str,
        rules: &[S],
        is_directory: bool,
        normalize_path: bool,
    ) -> bool {
        let parsed = compile_rules(Path::new("/"), rules);
        parsed.is_empty
            || is_ignored(
                &parsed.rules,
                &path_to_check(Path::new(path), normalize_path),
                is_directory,
            )
    }

    fn find_ignore_file_cached(&self, directory: &Path) -> io::Result<Option<PathBuf>> {
        if let Some(cached) = lock_unpoisoned(&self.directory_cache)
            .get(directory)
            .cloned()
        {
            return Ok(cached.map(|path| path.join(IGNORE_FILE_NAME)));
        }

        let mut checked_directories = vec![directory.to_path_buf()];
        let mut current = Some(directory);
        while let Some(current_directory) = current {
            let parent_cached = (current_directory != directory)
                .then(|| {
                    lock_unpoisoned(&self.directory_cache)
                        .get(current_directory)
                        .cloned()
                })
                .flatten();
            if let Some(cached) = parent_cached {
                self.cache_directories(&checked_directories, cached.as_deref());
                return Ok(cached.map(|path| path.join(IGNORE_FILE_NAME)));
            }

            let ignore_file = current_directory.join(IGNORE_FILE_NAME);
            match fs::metadata(&ignore_file) {
                Ok(metadata) if metadata.is_file() => {
                    self.cache_directories(&checked_directories, Some(current_directory));
                    return Ok(Some(ignore_file));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }

            current = current_directory.parent();
            if let Some(parent) = current
                && parent != current_directory
            {
                checked_directories.push(parent.to_path_buf());
            }
        }

        self.cache_directories(&checked_directories, None);
        Ok(None)
    }

    fn cache_directories(&self, directories: &[PathBuf], ignore_directory: Option<&Path>) {
        let mut cache = lock_unpoisoned(&self.directory_cache);
        for directory in directories {
            insert_bounded(
                &mut cache,
                self.directory_cache_capacity,
                directory.clone(),
                ignore_directory.map(Path::to_path_buf),
            );
        }
    }

    fn get_parsed_rules(&self, ignore_file: &Path) -> io::Result<Option<ParsedIgnoreCacheEntry>> {
        let metadata = match fs::metadata(ignore_file) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                lock_unpoisoned(&self.rules_cache).remove(ignore_file);
                return Ok(None);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                lock_unpoisoned(&self.rules_cache).remove(ignore_file);
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let modified = metadata.modified()?;
        let length = metadata.len();

        if let Some(cached) = lock_unpoisoned(&self.rules_cache).get(ignore_file)
            && cached.modified == modified
            && cached.length == length
        {
            return Ok(Some(cached.clone()));
        }

        let content = fs::read_to_string(ignore_file)?;
        let parsed = if content.trim().is_empty() {
            ParsedIgnoreCacheEntry {
                rules: Gitignore::empty(),
                modified,
                length,
                is_empty: true,
            }
        } else {
            let compiled = compile_rules(
                ignore_file.parent().unwrap_or_else(|| Path::new("/")),
                &content
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>(),
            );
            ParsedIgnoreCacheEntry {
                rules: compiled.rules,
                modified,
                length,
                is_empty: compiled.is_empty,
            }
        };

        insert_bounded(
            &mut lock_unpoisoned(&self.rules_cache),
            self.rules_cache_capacity,
            ignore_file.to_path_buf(),
            parsed.clone(),
        );
        Ok(Some(parsed))
    }
}

struct CompiledRules {
    rules: Gitignore,
    is_empty: bool,
}

fn compile_rules<S: AsRef<str>>(root: &Path, rules: &[S]) -> CompiledRules {
    let mut builder = GitignoreBuilder::new(root);
    let _ = builder.case_insensitive(true);
    let mut valid_rules_added = 0;

    for rule in rules {
        let rule = rule.as_ref();
        if !has_compatible_escapes(rule) {
            continue;
        }
        if builder.add_line(None, rule).is_ok() {
            valid_rules_added += 1;
        }
    }

    let rules = builder.build().unwrap_or_else(|_| Gitignore::empty());
    CompiledRules {
        is_empty: valid_rules_added == 0,
        rules,
    }
}

fn has_compatible_escapes(rule: &str) -> bool {
    let characters = rule.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '\\' {
            index += 1;
            continue;
        }

        let Some(&escaped) = characters.get(index + 1) else {
            return false;
        };
        if !escaped.is_ascii_alphabetic() {
            index += 2;
            continue;
        }

        match escaped {
            'a' | 'b' | 't' | 'r' | 'v' | 'f' | 'n' | 'e' | 'd' | 'D' | 's' | 'S' | 'w' | 'W'
            | 'p' | 'P' | 'A' | 'Z' | 'z' | 'G' | 'B' | 'k' => {
                index += 2;
            }
            'c' if characters
                .get(index + 2)
                .is_some_and(char::is_ascii_alphabetic) =>
            {
                index += 3;
            }
            'x' if has_hex_digits(&characters, index + 2, 2) => {
                index += 4;
            }
            'u' if has_hex_digits(&characters, index + 2, 4) => {
                index += 6;
            }
            _ => return false,
        }
    }
    true
}

fn has_hex_digits(characters: &[char], start: usize, length: usize) -> bool {
    characters
        .get(start..start + length)
        .is_some_and(|digits| digits.iter().all(char::is_ascii_hexdigit))
}

fn is_ignored(rules: &Gitignore, path: &Path, is_directory: bool) -> bool {
    let mut current = Some(path);
    let mut current_is_directory = is_directory;
    while let Some(candidate) = current {
        let matched = rules.matched(candidate, current_is_directory);
        if !matched.is_none() {
            return matched.is_ignore();
        }
        if candidate == rules.path() {
            break;
        }
        current = candidate.parent();
        current_is_directory = true;
    }
    false
}

fn path_to_check(path: &Path, normalize_path: bool) -> PathBuf {
    if normalize_path {
        PathBuf::from(path.to_string_lossy().replace('\\', "/"))
    } else {
        path.to_path_buf()
    }
}

fn insert_bounded<K, V>(cache: &mut HashMap<K, V>, capacity: usize, key: K, value: V)
where
    K: std::hash::Hash + Eq,
{
    if cache.len() >= capacity && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, value);
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
