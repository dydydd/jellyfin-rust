use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use jellyfin_extensions::{FileHelper, create_empty};
use uuid::Uuid;

#[test]
fn create_empty_creates_a_missing_file_like_the_official_helper() {
    let fixture = Fixture::new();
    let path = fixture.path("new-file");
    assert!(!path.exists());

    FileHelper::create_empty(&path).expect("missing file must be created");

    assert!(path.is_file());
    assert_eq!(fs::metadata(path).expect("created metadata").len(), 0);
}

#[test]
fn create_empty_truncates_an_existing_nonempty_file() {
    let fixture = Fixture::new();
    let path = fixture.path("existing-file");
    fs::write(&path, b"existing jellyfin data").expect("fixture file");

    create_empty(&path).expect("existing file must be truncated");

    assert_eq!(fs::read(path).expect("truncated file"), b"");
}

#[test]
fn create_empty_returns_the_parent_directory_error() {
    let fixture = Fixture::new();
    let missing_parent = fixture.path("missing-parent");
    let path = missing_parent.join("file");

    let error = FileHelper::create_empty(&path).expect_err("missing parent must fail");

    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert!(!missing_parent.exists());
}

#[test]
fn create_empty_returns_an_error_for_a_directory_path() {
    let fixture = Fixture::new();

    let error = create_empty(&fixture.root).expect_err("directory path must fail");

    assert_ne!(error.kind(), ErrorKind::NotFound);
    assert!(fixture.root.is_dir());
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "jellyfin-file-helper-tests-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&root).expect("fixture directory");
        Self { root }
    }

    fn path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}
