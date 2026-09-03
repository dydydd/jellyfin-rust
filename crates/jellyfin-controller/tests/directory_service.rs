use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use jellyfin_controller::providers::{
    DirectoryService, FileSystem, FileSystemMetadata, LocalFileSystem,
};
use uuid::Uuid;

const LOWER_CASE_PATH: &str = "/music/someartist";
const UPPER_CASE_PATH: &str = "/music/SOMEARTIST";

#[test]
fn paths_with_different_casing_cache_all_file_system_entries() {
    let file_system = TestFileSystem::default();
    let lower_entries = lower_case_metadata();
    let upper_entries = upper_case_metadata();
    file_system.set_entries(LOWER_CASE_PATH, lower_entries.clone());
    file_system.set_entries(UPPER_CASE_PATH, upper_entries.clone());
    let service = DirectoryService::new(file_system);

    let upper_result = service.get_file_system_entries(UPPER_CASE_PATH).unwrap();
    let lower_result = service.get_file_system_entries(LOWER_CASE_PATH).unwrap();

    assert_eq!(upper_result.as_ref(), upper_entries);
    assert_eq!(lower_result.as_ref(), lower_entries);
}

#[test]
fn paths_with_different_casing_return_the_correct_files() {
    let file_system = TestFileSystem::default();
    let lower_entries = lower_case_metadata();
    let upper_entries = upper_case_metadata();
    file_system.set_entries(LOWER_CASE_PATH, lower_entries.clone());
    file_system.set_entries(UPPER_CASE_PATH, upper_entries.clone());
    let service = DirectoryService::new(file_system);

    let upper_result = service.get_files(UPPER_CASE_PATH).unwrap();
    let lower_result = service.get_files(LOWER_CASE_PATH).unwrap();

    assert_eq!(upper_result, files(&upper_entries));
    assert_eq!(lower_result, files(&lower_entries));
}

#[test]
fn paths_with_different_casing_return_the_correct_directories() {
    let file_system = TestFileSystem::default();
    let lower_entries = lower_case_metadata();
    let upper_entries = upper_case_metadata();
    file_system.set_entries(LOWER_CASE_PATH, lower_entries.clone());
    file_system.set_entries(UPPER_CASE_PATH, upper_entries.clone());
    let service = DirectoryService::new(file_system);

    let upper_result = service.get_directories(UPPER_CASE_PATH).unwrap();
    let lower_result = service.get_directories(LOWER_CASE_PATH).unwrap();

    assert_eq!(upper_result, directories(&upper_entries));
    assert_eq!(lower_result, directories(&lower_entries));
}

#[test]
fn file_paths_with_different_casing_return_the_correct_file() {
    const LOWER_FILE: &str = "/music/someartist/song 1.mp3";
    const UPPER_FILE: &str = "/music/SOMEARTIST/SONG 1.mp3";
    let lower_metadata = existing_metadata(LOWER_FILE, false);
    let upper_metadata = FileSystemMetadata {
        full_name: UPPER_FILE.into(),
        ..FileSystemMetadata::default()
    };
    let file_system = TestFileSystem::default();
    file_system.set_info(LOWER_FILE, lower_metadata.clone());
    file_system.set_info(UPPER_FILE, upper_metadata);
    let service = DirectoryService::new(file_system);

    assert_eq!(service.get_directory(LOWER_FILE).unwrap(), None);
    assert_eq!(
        service.get_file(LOWER_FILE).unwrap().as_deref(),
        Some(&lower_metadata)
    );
    assert_eq!(service.get_directory(UPPER_FILE).unwrap(), None);
    assert_eq!(service.get_file(UPPER_FILE).unwrap(), None);
}

#[test]
fn directory_paths_with_different_casing_return_the_correct_directory() {
    const LOWER_DIRECTORY: &str = "/music/someartist/Lyrics";
    const UPPER_DIRECTORY: &str = "/music/SOMEARTIST/LYRICS";
    let lower_metadata = existing_metadata(LOWER_DIRECTORY, true);
    let upper_metadata = FileSystemMetadata {
        full_name: UPPER_DIRECTORY.into(),
        is_directory: true,
        ..FileSystemMetadata::default()
    };
    let file_system = TestFileSystem::default();
    file_system.set_info(LOWER_DIRECTORY, lower_metadata.clone());
    file_system.set_info(UPPER_DIRECTORY, upper_metadata);
    let service = DirectoryService::new(file_system);

    assert_eq!(
        service.get_directory(LOWER_DIRECTORY).unwrap().as_deref(),
        Some(&lower_metadata)
    );
    assert_eq!(service.get_file(LOWER_DIRECTORY).unwrap(), None);
    assert_eq!(service.get_directory(UPPER_DIRECTORY).unwrap(), None);
    assert_eq!(service.get_file(UPPER_DIRECTORY).unwrap(), None);
}

#[test]
fn cached_file_path_returns_the_cached_file() {
    const PATH: &str = "/music/someartist/song 1.mp3";
    let cached_metadata = existing_metadata(PATH, false);
    let new_metadata = existing_metadata("/music/SOMEARTIST/song 1.mp3", false);
    let file_system = TestFileSystem::default();
    file_system.set_info(PATH, cached_metadata.clone());
    let service = DirectoryService::new(file_system.clone());

    let result = service.get_file(PATH).unwrap();
    file_system.set_info(PATH, new_metadata);
    let second_result = service.get_file(PATH).unwrap();

    assert_eq!(result.as_deref(), Some(&cached_metadata));
    assert_eq!(second_result.as_deref(), Some(&cached_metadata));
}

#[test]
fn cached_file_paths_without_clear_return_only_cached_paths() {
    let cached_paths = numbered_paths(1);
    let new_paths = numbered_paths(5);
    let file_system = TestFileSystem::default();
    file_system.set_file_paths(LOWER_CASE_PATH, cached_paths.clone());
    let service = DirectoryService::new(file_system.clone());

    let result = service.get_file_paths(LOWER_CASE_PATH).unwrap();
    file_system.set_file_paths(LOWER_CASE_PATH, new_paths);
    let second_result = service.get_file_paths(LOWER_CASE_PATH).unwrap();

    assert_eq!(result.as_ref(), cached_paths);
    assert_eq!(second_result.as_ref(), cached_paths);
}

#[test]
fn cached_file_paths_with_clear_return_new_paths() {
    let cached_paths = numbered_paths(1);
    let new_paths = numbered_paths(5);
    let file_system = TestFileSystem::default();
    file_system.set_file_paths(LOWER_CASE_PATH, cached_paths.clone());
    let service = DirectoryService::new(file_system.clone());

    let result = service.get_file_paths(LOWER_CASE_PATH).unwrap();
    file_system.set_file_paths(LOWER_CASE_PATH, new_paths.clone());
    let second_result = service
        .get_file_paths_with_options(LOWER_CASE_PATH, true)
        .unwrap();

    assert_eq!(result.as_ref(), cached_paths);
    assert_eq!(second_result.as_ref(), new_paths);
}

#[test]
fn local_file_system_exercises_the_production_backend() {
    let directory = TestDirectory::new();
    let album = directory.path().join("Album");
    let first_file = directory.path().join("01 Song.MP3");
    let second_file = directory.path().join("cover.jpg");
    fs::create_dir(&album).unwrap();
    fs::write(&first_file, b"audio").unwrap();
    fs::write(&second_file, b"image").unwrap();
    let service = DirectoryService::new(LocalFileSystem);

    let entries = service.get_file_system_entries(directory.path()).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(service.get_directories(directory.path()).unwrap().len(), 1);
    assert_eq!(service.get_files(directory.path()).unwrap().len(), 2);

    let paths = service.get_file_paths(directory.path()).unwrap();
    assert_eq!(paths.as_ref(), [first_file.clone(), second_file]);
    let metadata = service.get_file(&first_file).unwrap().unwrap();
    assert_eq!(metadata.extension.as_deref(), Some(".MP3".as_ref()));
    assert_eq!(metadata.length, 5);
    assert!(service.is_accessible(directory.path()).unwrap());
}

fn lower_case_metadata() -> Vec<FileSystemMetadata> {
    vec![
        metadata(format!("{LOWER_CASE_PATH}/Artwork"), true),
        metadata(format!("{LOWER_CASE_PATH}/Some Other Folder"), true),
        metadata(format!("{LOWER_CASE_PATH}/Song 2.mp3"), false),
        metadata(format!("{LOWER_CASE_PATH}/Song 3.mp3"), false),
    ]
}

fn upper_case_metadata() -> Vec<FileSystemMetadata> {
    vec![
        metadata(format!("{UPPER_CASE_PATH}/Lyrics"), true),
        metadata(format!("{UPPER_CASE_PATH}/Song 1.mp3"), false),
    ]
}

fn metadata(path: impl Into<PathBuf>, is_directory: bool) -> FileSystemMetadata {
    FileSystemMetadata {
        full_name: path.into(),
        is_directory,
        ..FileSystemMetadata::default()
    }
}

fn existing_metadata(path: impl Into<PathBuf>, is_directory: bool) -> FileSystemMetadata {
    FileSystemMetadata {
        exists: true,
        ..metadata(path, is_directory)
    }
}

fn files(entries: &[FileSystemMetadata]) -> Vec<FileSystemMetadata> {
    entries
        .iter()
        .filter(|entry| !entry.is_directory)
        .cloned()
        .collect()
}

fn directories(entries: &[FileSystemMetadata]) -> Vec<FileSystemMetadata> {
    entries
        .iter()
        .filter(|entry| entry.is_directory)
        .cloned()
        .collect()
}

fn numbered_paths(first: u8) -> Vec<PathBuf> {
    (first..first + 4)
        .map(|number| PathBuf::from(format!("{LOWER_CASE_PATH}/song {number}.mp3")))
        .collect()
}

#[derive(Clone, Default)]
struct TestFileSystem {
    state: Arc<RwLock<TestFileSystemState>>,
}

#[derive(Default)]
struct TestFileSystemState {
    entries: HashMap<PathBuf, Vec<FileSystemMetadata>>,
    info: HashMap<PathBuf, FileSystemMetadata>,
    file_paths: HashMap<PathBuf, Vec<PathBuf>>,
    entry_paths: HashMap<PathBuf, Vec<PathBuf>>,
}

impl TestFileSystem {
    fn set_entries(&self, path: impl Into<PathBuf>, entries: Vec<FileSystemMetadata>) {
        self.state
            .write()
            .unwrap()
            .entries
            .insert(path.into(), entries);
    }

    fn set_info(&self, path: impl Into<PathBuf>, info: FileSystemMetadata) {
        self.state.write().unwrap().info.insert(path.into(), info);
    }

    fn set_file_paths(&self, path: impl Into<PathBuf>, file_paths: Vec<PathBuf>) {
        self.state
            .write()
            .unwrap()
            .file_paths
            .insert(path.into(), file_paths);
    }
}

impl FileSystem for TestFileSystem {
    fn get_file_system_entries(&self, path: &Path) -> io::Result<Vec<FileSystemMetadata>> {
        self.state
            .read()
            .unwrap()
            .entries
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn get_file_system_info(&self, path: &Path) -> io::Result<FileSystemMetadata> {
        Ok(self
            .state
            .read()
            .unwrap()
            .info
            .get(path)
            .cloned()
            .unwrap_or_else(|| FileSystemMetadata {
                full_name: path.to_path_buf(),
                ..FileSystemMetadata::default()
            }))
    }

    fn get_file_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.state
            .read()
            .unwrap()
            .file_paths
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn get_file_system_entry_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        Ok(self
            .state
            .read()
            .unwrap()
            .entry_paths
            .get(path)
            .cloned()
            .unwrap_or_default())
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-directory-service-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
