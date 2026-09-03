use std::{fs, path::PathBuf};

use jellyfin_controller::client_event::ClientEventLogger;
use jellyfin_extensions::PathHelper;
use uuid::Uuid;

macro_rules! traversal_case {
    ($name:ident, $client_name:expr, $client_version:expr) => {
        #[tokio::test]
        async fn $name() {
            assert_traversal_stays_inside($client_name, $client_version).await;
        }
    };
}

traversal_case!(unix_traversal_client_name, "../../../../etc/passwd", "1.0");
traversal_case!(
    windows_traversal_client_name,
    "..\\..\\windows\\system32",
    "1.0"
);
traversal_case!(
    traversal_client_version,
    "normal-client",
    "../../../etc/passwd"
);
traversal_case!(absolute_client_name, "/absolute/path", "1.0");

#[tokio::test]
async fn legal_client_fields_are_preserved_and_payload_is_written() {
    let directory = TestDirectory::new();
    let logger = ClientEventLogger::new(directory.path());
    let mut contents = &b"payload"[..];

    let file_name = logger
        .write_document("normal-client", "1.0", &mut contents)
        .await
        .unwrap();

    assert!(file_name.starts_with("upload_normal-client_1.0_"));
    assert_eq!(
        fs::read(directory.path().join(file_name)).unwrap(),
        b"payload"
    );
}

#[tokio::test]
async fn unusable_client_fields_use_official_fallback_names() {
    let directory = TestDirectory::new();
    let logger = ClientEventLogger::new(directory.path());
    let mut contents = &b"payload"[..];

    let file_name = logger.write_document(".", "", &mut contents).await.unwrap();

    assert!(file_name.starts_with("upload_unknown-client_unknown-version_"));
    assert!(directory.path().join(file_name).is_file());
}

async fn assert_traversal_stays_inside(client_name: &str, client_version: &str) {
    let directory = TestDirectory::new();
    let logger = ClientEventLogger::new(directory.path());
    let mut contents = &b"payload"[..];

    let file_name = logger
        .write_document(client_name, client_version, &mut contents)
        .await
        .unwrap();

    let resolved = fs::canonicalize(directory.path().join(&file_name)).unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    assert!(PathHelper::is_contained_in(&root, &resolved).unwrap());
    assert_eq!(resolved.parent(), Some(root.as_path()));
    assert!(resolved.is_file());
    assert_eq!(fs::read(resolved).unwrap(), b"payload");
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jellyfin-clientlog-test-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
