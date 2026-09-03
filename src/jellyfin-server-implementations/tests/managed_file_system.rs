use std::{
    fs, io,
    path::{Path, PathBuf},
};

use jellyfin_server_implementations::ManagedFileSystem;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[test]
fn make_absolute_path_handles_unix_paths_independently_of_the_host() {
    for (folder_path, file_path, expected) in [
        (
            "/Volumes/Library/Sample/Music/Playlists/",
            "../Beethoven/Misc/Moonlight Sonata.mp3",
            "/Volumes/Library/Sample/Music/Beethoven/Misc/Moonlight Sonata.mp3",
        ),
        (
            "/Volumes/Library/Sample/Music/Playlists/",
            "../../Beethoven/Misc/Moonlight Sonata.mp3",
            "/Volumes/Library/Sample/Beethoven/Misc/Moonlight Sonata.mp3",
        ),
        (
            "/Volumes/Library/Sample/Music/Playlists/",
            "Beethoven/Misc/Moonlight Sonata.mp3",
            "/Volumes/Library/Sample/Music/Playlists/Beethoven/Misc/Moonlight Sonata.mp3",
        ),
        (
            "/Volumes/Library/Sample/Music/Playlists/",
            "/mnt/Beethoven/Misc/Moonlight Sonata.mp3",
            "/mnt/Beethoven/Misc/Moonlight Sonata.mp3",
        ),
    ] {
        assert_eq!(
            ManagedFileSystem::make_absolute_path(folder_path, file_path),
            expected
        );
    }
}

#[test]
fn make_absolute_path_handles_windows_paths_independently_of_the_host() {
    for (folder_path, file_path, expected) in [
        (
            r"C:\\Volumes\Library\Sample\Music\Playlists\",
            r"..\Beethoven\Misc\Moonlight Sonata.mp3",
            r"C:\Volumes\Library\Sample\Music\Beethoven\Misc\Moonlight Sonata.mp3",
        ),
        (
            r"C:\\Volumes\Library\Sample\Music\Playlists\",
            r"..\..\Beethoven\Misc\Moonlight Sonata.mp3",
            r"C:\Volumes\Library\Sample\Beethoven\Misc\Moonlight Sonata.mp3",
        ),
        (
            r"C:\\Volumes\Library\Sample\Music\Playlists\",
            r"Beethoven\Misc\Moonlight Sonata.mp3",
            r"C:\Volumes\Library\Sample\Music\Playlists\Beethoven\Misc\Moonlight Sonata.mp3",
        ),
        (
            r"C:\\Volumes\Library\Sample\Music\Playlists\",
            r"D:\\Beethoven\Misc\Moonlight Sonata.mp3",
            r"D:\\Beethoven\Misc\Moonlight Sonata.mp3",
        ),
    ] {
        assert_eq!(
            ManagedFileSystem::make_absolute_path(folder_path, file_path),
            expected
        );
    }
}

#[test]
fn get_valid_filename_matches_the_official_matrix() {
    for (filename, expected) in [
        ("ValidFileName", "ValidFileName"),
        ("AC/DC", "AC DC"),
        ("Invalid\0", "Invalid "),
        ("AC/DC\0KD/A", "AC DC KD A"),
    ] {
        assert_eq!(ManagedFileSystem::get_valid_filename(filename), expected);
    }
}

#[test]
fn get_valid_filename_replaces_the_complete_invalid_character_set() {
    let invalid_controls: String = ('\0'..='\u{1f}').collect();
    let filename = format!("{invalid_controls}\"<>|:*?\\/");
    assert_eq!(
        ManagedFileSystem::get_valid_filename(&filename),
        " ".repeat(filename.chars().count())
    );
}

#[cfg(unix)]
#[test]
fn dangling_symlink_is_not_reported_as_an_existing_file() {
    let root = TemporaryDirectory::new_in(std::env::temp_dir()).expect("temporary directory");
    let link = root.path().join("dangling.link");
    std::os::unix::fs::symlink("thispathdoesntexist", &link).expect("create dangling symlink");
    assert!(fs::symlink_metadata(&link).is_ok());

    let info = ManagedFileSystem::get_file_info(&link).expect("read dangling symlink metadata");
    assert!(!info.exists);
    assert_eq!(info.length, None);
}

#[test]
fn move_directory_on_the_same_file_system_is_recursive() {
    let root = TemporaryDirectory::new_in(std::env::temp_dir()).expect("temporary directory");
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    create_source_tree(&source);

    ManagedFileSystem::move_directory(&source, &destination).expect("same-device move");
    assert_moved_tree(&source, &destination);
}

#[cfg(unix)]
#[test]
fn move_directory_across_file_systems_is_recursive_when_available() {
    let Some((source_root, destination_root)) = cross_device_roots() else {
        return;
    };

    let source = source_root.path().join("source");
    let destination = destination_root.path().join("destination");
    create_source_tree(&source);

    ManagedFileSystem::move_directory(&source, &destination).expect("cross-device move");
    assert_moved_tree(&source, &destination);
}

#[cfg(unix)]
#[test]
fn failed_cross_device_copy_keeps_source_and_does_not_publish_destination() {
    use std::os::unix::net::UnixListener;

    let Some((source_root, destination_root)) = cross_device_roots() else {
        return;
    };
    let source = source_root.path().join("source");
    let destination = destination_root.path().join("destination");
    create_source_tree(&source);
    let socket = source.join("unsupported.socket");
    let _listener = UnixListener::bind(&socket).expect("create source Unix socket");

    let error = ManagedFileSystem::move_directory(&source, &destination).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(source.join("tempfile0").is_file());
    assert!(socket.exists());
    assert!(!destination.exists());
    assert!(fs::read_dir(destination_root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jellyfin-move-")
    }));
}

#[test]
fn failed_move_does_not_remove_the_source_or_overwrite_the_destination() {
    let root = TemporaryDirectory::new_in(std::env::temp_dir()).expect("temporary directory");
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    create_source_tree(&source);
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("existing"), b"keep").unwrap();

    let error = ManagedFileSystem::move_directory(&source, &destination).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(source.join("tempfile0").is_file());
    assert_eq!(fs::read(destination.join("existing")).unwrap(), b"keep");
}

fn create_source_tree(source: &Path) {
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("tempfile0"), b"zero").unwrap();
    fs::write(source.join("tempfile1"), b"one").unwrap();
    fs::write(source.join("nested/child"), b"child").unwrap();
}

fn assert_moved_tree(source: &Path, destination: &Path) {
    assert!(!source.exists());
    assert_eq!(fs::read(destination.join("tempfile0")).unwrap(), b"zero");
    assert_eq!(fs::read(destination.join("tempfile1")).unwrap(), b"one");
    assert_eq!(
        fs::read(destination.join("nested/child")).unwrap(),
        b"child"
    );
}

#[cfg(unix)]
fn cross_device_roots() -> Option<(TemporaryDirectory, TemporaryDirectory)> {
    let shared_memory = Path::new("/dev/shm");
    if !fs::metadata(shared_memory).ok()?.is_dir() {
        return None;
    }

    let source =
        TemporaryDirectory::new_in(std::env::temp_dir()).expect("source temporary directory");
    let destination = match TemporaryDirectory::new_in(shared_memory) {
        Ok(directory) => directory,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            return None;
        }
        Err(error) => panic!("destination temporary directory: {error}"),
    };

    (fs::metadata(source.path()).unwrap().dev() != fs::metadata(destination.path()).unwrap().dev())
        .then_some((source, destination))
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new_in(parent: impl AsRef<Path>) -> io::Result<Self> {
        for _ in 0..8 {
            let path = parent
                .as_ref()
                .join(format!("jellyfin-rust-test-{}", Uuid::new_v4().simple()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
