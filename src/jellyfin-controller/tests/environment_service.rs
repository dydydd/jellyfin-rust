use std::{fs, path::PathBuf};

use jellyfin_controller::{EnvironmentError, EnvironmentService};
use jellyfin_model::FileSystemEntryType;
use uuid::Uuid;

#[test]
fn directory_contents_are_uncached_filtered_sorted_and_unicode_safe() {
    let directory = TestDirectory::new();
    let service = EnvironmentService::new();
    fs::create_dir(directory.path().join("Zulu Folder")).unwrap();
    fs::create_dir(directory.path().join("媒体")).unwrap();
    fs::write(directory.path().join("zeta.txt"), b"z").unwrap();
    fs::write(directory.path().join("ä song.mkv"), b"video").unwrap();
    let path = directory.path_string();

    assert!(
        service
            .directory_contents(&path, false, false)
            .unwrap()
            .is_empty()
    );
    let files = service.directory_contents(&path, true, false).unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .all(|entry| entry.entry_type == FileSystemEntryType::File)
    );
    assert!(files.iter().any(|entry| entry.name == "ä song.mkv"));

    let directories = service.directory_contents(&path, false, true).unwrap();
    assert_eq!(directories.len(), 2);
    assert!(
        directories
            .iter()
            .all(|entry| entry.entry_type == FileSystemEntryType::Directory)
    );
    assert!(directories.iter().any(|entry| entry.name == "媒体"));

    let first = service.directory_contents(&path, true, true).unwrap();
    assert!(first.windows(2).all(|pair| pair[0].path <= pair[1].path));
    fs::write(directory.path().join("new.txt"), b"new").unwrap();
    let second = service.directory_contents(&path, true, true).unwrap();
    assert_eq!(second.len(), first.len() + 1);
    assert!(second.iter().any(|entry| entry.name == "new.txt"));
}

#[test]
fn directory_contents_preserve_unc_and_missing_directory_behavior() {
    let service = EnvironmentService::new();
    assert!(
        service
            .directory_contents(r"\\server", true, true)
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .directory_contents(r"\\", true, true)
            .unwrap()
            .is_empty()
    );

    let missing = std::env::temp_dir().join(format!(
        "jellyfin-environment-missing-{}",
        Uuid::new_v4().simple()
    ));
    assert!(matches!(
        service.directory_contents(&missing.to_string_lossy(), true, true),
        Err(EnvironmentError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn validate_path_matches_official_three_way_branching_and_cleans_probe() {
    let directory = TestDirectory::new();
    let service = EnvironmentService::new();
    let file = directory.path().join("media.mkv");
    let missing = directory.path().join("missing");
    fs::write(&file, b"media").unwrap();
    let directory_path = directory.path_string();
    let file_path = file.to_string_lossy();
    let missing_path = missing.to_string_lossy();

    assert!(
        service
            .validate_path(Some(&file_path), Some(true), true)
            .is_ok()
    );
    assert!(
        service
            .validate_path(Some(&directory_path), Some(false), true)
            .is_ok()
    );
    assert!(matches!(
        service.validate_path(Some(&directory_path), Some(true), false),
        Err(EnvironmentError::NotFound)
    ));
    assert!(matches!(
        service.validate_path(Some(&file_path), Some(false), false),
        Err(EnvironmentError::NotFound)
    ));
    assert!(service.validate_path(Some(&file_path), None, false).is_ok());
    assert!(
        service
            .validate_path(Some(&directory_path), None, false)
            .is_ok()
    );
    assert!(matches!(
        service.validate_path(Some(&missing_path), None, false),
        Err(EnvironmentError::NotFound)
    ));
    assert!(matches!(
        service.validate_path(None, None, true),
        Err(EnvironmentError::NotFound)
    ));

    let before = child_names(directory.path());
    service
        .validate_path(Some(&directory_path), None, true)
        .unwrap();
    assert_eq!(child_names(directory.path()), before);
    assert!(matches!(
        service.validate_path(Some(&file_path), None, true),
        Err(EnvironmentError::Io(_))
    ));
}

#[test]
fn parent_paths_and_real_drives_match_environment_contract() {
    let service = EnvironmentService::new();
    if cfg!(windows) {
        assert_eq!(
            service.parent_path(r"C:\Media\Movies"),
            Some(r"C:\Media".to_owned())
        );
    } else {
        assert_eq!(
            service.parent_path("/media/movies"),
            Some("/media".to_owned())
        );
        assert_eq!(
            service.parent_path("relative/child"),
            Some("relative".to_owned())
        );
        assert_eq!(
            service.parent_path(r"\\server\share\child"),
            Some(r"\\server\share".to_owned())
        );
    }
    assert_eq!(service.parent_path("single"), None);
    assert_eq!(service.parent_path(r"\\server"), None);

    let drives = service.drives();
    assert!(
        !drives.is_empty(),
        "the running system must expose a real drive"
    );
    assert!(drives.iter().all(|drive| {
        drive.entry_type == FileSystemEntryType::Directory && PathBuf::from(&drive.path).is_dir()
    }));
}

fn child_names(path: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-environment-service-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn path_string(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
