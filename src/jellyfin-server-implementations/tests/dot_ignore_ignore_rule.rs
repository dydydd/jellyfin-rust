use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use jellyfin_server_implementations::{DotIgnoreFileSystemEntry, DotIgnoreIgnoreRule};
use uuid::Uuid;

const RULE_1: &[&str] = &["SPs"];
const RULE_2: &[&str] = &["SPs", "!thebestshot.mkv"];
const RULE_3: &[&str] = &[
    "*.txt",
    r"{\colortbl;\red255\green255\blue255;}",
    "videos/",
    r"\invalid\escape\sequence",
    "*.mkv",
];
const RULE_4: &[&str] = &[
    r"{\colortbl;\red255\green255\blue255;}",
    r"\invalid\escape\sequence",
];

#[test]
fn check_ignore_rules_returns_expected_result() {
    let cases = [
        (RULE_1, "f:/cd/sps/ffffff.mkv", false, true),
        (RULE_1, "cd/sps/ffffff.mkv", false, true),
        (RULE_1, "/cd/sps/ffffff.mkv", false, true),
        (RULE_2, "f:/cd/sps/ffffff.mkv", false, true),
        (RULE_2, "cd/sps/ffffff.mkv", false, true),
        (RULE_2, "/cd/sps/ffffff.mkv", false, true),
        (RULE_2, "f:/cd/sps/thebestshot.mkv", false, false),
        (RULE_2, "cd/sps/thebestshot.mkv", false, false),
        (RULE_2, "/cd/sps/thebestshot.mkv", false, false),
        (RULE_3, "test.txt", false, true),
        (RULE_3, "videos/movie.mp4", false, true),
        (RULE_3, "movie.mkv", false, true),
        (RULE_3, "test.mp3", false, false),
        (RULE_4, "any-file.txt", false, true),
        (RULE_4, "any/path/to/file.mkv", false, true),
    ];

    for (rules, path, is_directory, expected) in cases {
        assert_eq!(
            DotIgnoreIgnoreRule::check_ignore_rules(path, rules, is_directory, false),
            expected,
            "path={path:?}"
        );
    }
}

#[test]
fn windows_paths_are_normalized_when_requested() {
    let cases = [
        (RULE_1, r"C:\cd\sps\ffffff.mkv", false, true),
        (RULE_1, r"D:\media\sps\movie.mkv", false, true),
        (RULE_1, r"\\server\share\sps\file.mkv", false, true),
        (RULE_2, r"C:\cd\sps\ffffff.mkv", false, true),
        (RULE_2, r"C:\cd\sps\thebestshot.mkv", false, false),
        (RULE_3, r"C:\videos\movie.mp4", false, true),
        (RULE_3, r"D:\documents\test.txt", false, true),
        (RULE_3, r"E:\music\song.mp3", false, false),
    ];

    for (rules, path, is_directory, expected) in cases {
        assert_eq!(
            DotIgnoreIgnoreRule::check_ignore_rules(path, rules, is_directory, true),
            expected,
            "path={path:?}"
        );
    }
}

#[test]
fn windows_paths_do_not_match_without_normalization() {
    for path in [r"C:\cd\sps\ffffff.mkv", r"D:\media\sps\movie.mkv"] {
        assert!(!DotIgnoreIgnoreRule::check_ignore_rules(
            path, RULE_1, false, false
        ));
    }
}

#[test]
fn comments_are_valid_no_op_rules_and_escaped_comments_are_patterns() {
    assert!(!DotIgnoreIgnoreRule::check_ignore_rules(
        "anything.mkv",
        &["# comment only"],
        false,
        false,
    ));
    assert!(DotIgnoreIgnoreRule::check_ignore_rules(
        "#secret",
        &[r"\#secret"],
        false,
        false,
    ));
    assert!(!DotIgnoreIgnoreRule::check_ignore_rules(
        "secret",
        &[r"\#secret"],
        false,
        false,
    ));
}

#[test]
fn repeated_calls_use_cached_rules() {
    let directory = TestDirectory::new();
    let subdirectory = directory.path().join("subdir");
    fs::create_dir(&subdirectory).expect("subdirectory");
    fs::write(directory.path().join(".ignore"), "*.tmp").expect("ignore file");
    let rule = DotIgnoreIgnoreRule::new();

    for (name, expected) in [
        ("test.tmp", true),
        ("test.tmp", true),
        ("other.tmp", true),
        ("other.txt", false),
    ] {
        assert_eq!(
            should_ignore(&rule, subdirectory.join(name), false),
            expected
        );
    }
}

#[test]
fn modified_ignore_file_reparses_cached_rules() {
    let directory = TestDirectory::new();
    let ignore_file = directory.path().join(".ignore");
    fs::write(&ignore_file, "*.tmp").expect("ignore file");
    let rule = DotIgnoreIgnoreRule::new();

    assert!(should_ignore(
        &rule,
        directory.path().join("test.tmp"),
        false
    ));
    thread::sleep(Duration::from_millis(50));
    fs::write(&ignore_file, "*.txt").expect("modified ignore file");

    assert!(!should_ignore(
        &rule,
        directory.path().join("test.tmp"),
        false
    ));
    assert!(should_ignore(
        &rule,
        directory.path().join("test.txt"),
        false
    ));
}

#[test]
fn empty_ignore_file_ignores_everything() {
    let directory = TestDirectory::new();
    fs::write(directory.path().join(".ignore"), "").expect("ignore file");

    assert!(should_ignore(
        &DotIgnoreIgnoreRule::new(),
        directory.path().join("anyfile.mkv"),
        false
    ));
}

#[test]
fn whitespace_only_ignore_file_ignores_everything() {
    let directory = TestDirectory::new();
    fs::write(directory.path().join(".ignore"), "   \n\t\n   ").expect("ignore file");

    assert!(should_ignore(
        &DotIgnoreIgnoreRule::new(),
        directory.path().join("anyfile.mkv"),
        false
    ));
}

#[test]
fn no_ignore_file_does_not_ignore() {
    let directory = TestDirectory::new();

    assert!(!should_ignore(
        &DotIgnoreIgnoreRule::new(),
        directory.path().join("anyfile.mkv"),
        false
    ));
}

#[test]
fn concurrent_access_is_thread_safe() {
    let directory = TestDirectory::new();
    fs::write(directory.path().join(".ignore"), "*.tmp").expect("ignore file");
    let rule = Arc::new(DotIgnoreIgnoreRule::new());
    let mut threads = Vec::new();

    for index in 0..100 {
        let rule = Arc::clone(&rule);
        let directory = directory.path().to_path_buf();
        threads.push(thread::spawn(move || {
            assert!(should_ignore(
                &rule,
                directory.join(format!("test{index}.tmp")),
                false
            ));
            assert!(!should_ignore(
                &rule,
                directory.join(format!("test{index}.txt")),
                false
            ));
        }));
    }
    for handle in threads {
        handle.join().expect("ignore worker");
    }
}

#[test]
fn clear_directory_cache_forces_lookup_again() {
    let directory = TestDirectory::new();
    let file = directory.path().join("test.tmp");
    let rule = DotIgnoreIgnoreRule::new();

    assert!(!should_ignore(&rule, &file, false));
    fs::write(directory.path().join(".ignore"), "*.tmp").expect("ignore file");
    assert!(!should_ignore(&rule, &file, false));

    rule.clear_directory_cache();
    assert!(should_ignore(&rule, &file, false));
}

#[test]
fn deleted_ignore_file_is_handled_gracefully() {
    let directory = TestDirectory::new();
    let ignore_file = directory.path().join(".ignore");
    let file = directory.path().join("test.tmp");
    fs::write(&ignore_file, "*.tmp").expect("ignore file");
    let rule = DotIgnoreIgnoreRule::new();

    assert!(should_ignore(&rule, &file, false));
    fs::remove_file(ignore_file).expect("delete ignore file");
    assert!(!should_ignore(&rule, &file, false));
}

#[test]
fn parent_directory_ignore_file_applies_to_subdirectories() {
    let directory = TestDirectory::new();
    let subdirectory = directory.path().join("sub1/sub2");
    fs::create_dir_all(&subdirectory).expect("subdirectories");
    fs::write(directory.path().join(".ignore"), "*.tmp").expect("ignore file");
    let rule = DotIgnoreIgnoreRule::new();

    assert!(should_ignore(&rule, subdirectory.join("test.tmp"), false));
    assert!(should_ignore(
        &rule,
        directory.path().join("sub1/test.tmp"),
        false
    ));
}

#[test]
fn trailing_slash_pattern_only_matches_directories() {
    let directory = TestDirectory::new();
    let videos = directory.path().join("videos");
    fs::create_dir(&videos).expect("videos directory");
    fs::write(directory.path().join(".ignore"), "videos/").expect("ignore file");
    let rule = DotIgnoreIgnoreRule::new();

    assert!(should_ignore(&rule, &videos, true));
    assert!(!should_ignore(&rule, &videos, false));
}

#[test]
fn ignore_file_read_errors_are_propagated() {
    let directory = TestDirectory::new();
    fs::write(directory.path().join(".ignore"), [0xff, 0xfe]).expect("invalid ignore file");

    let error = DotIgnoreIgnoreRule::new()
        .should_ignore(&DotIgnoreFileSystemEntry::new(
            directory.path().join("test.tmp"),
            false,
        ))
        .expect_err("invalid UTF-8 must be a read error");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

fn should_ignore(rule: &DotIgnoreIgnoreRule, path: impl Into<PathBuf>, is_directory: bool) -> bool {
    rule.should_ignore(&DotIgnoreFileSystemEntry::new(path, is_directory))
        .expect("ignore check")
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("jellyfin-dot-ignore-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).expect("remove test directory");
    }
}
