use std::{
    fs,
    path::{Path, PathBuf},
};

use jellyfin_model::FolderStorageDto;
use sysinfo::{Disk, DiskRefreshKind, Disks};

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemStorageService;

impl SystemStorageService {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn folder(&self, path: impl AsRef<Path>) -> FolderStorageDto {
        folder_storage(path.as_ref())
    }
}

fn folder_storage(path: &Path) -> FolderStorageDto {
    let display_path = path_to_string(path);
    let resolved_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());
    let Some(disk) = best_matching_disk(&disks, &resolved_path) else {
        return unavailable_storage(display_path);
    };
    let Ok(free_space) = i64::try_from(disk.available_space()) else {
        return unavailable_storage(display_path);
    };
    let used_space = disk
        .total_space()
        .saturating_sub(disk.available_space())
        .try_into()
        .unwrap_or(i64::MAX);
    FolderStorageDto {
        path: display_path,
        free_space,
        used_space,
        storage_type: Some(format!("{:?}", disk.kind())),
        device_id: Some(path_to_string(disk.mount_point())),
    }
}

fn best_matching_disk<'a>(disks: &'a Disks, resolved_path: &Path) -> Option<&'a Disk> {
    disks
        .list()
        .iter()
        .filter(|disk| path_starts_with(resolved_path, disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())
}

fn path_starts_with(path: &Path, mount_point: &Path) -> bool {
    path.starts_with(mount_point)
        || equivalent_absolute(path).is_some_and(|path| path.starts_with(mount_point))
}

fn equivalent_absolute(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }
    std::env::current_dir().ok().map(|cwd| cwd.join(path))
}

fn unavailable_storage(path: String) -> FolderStorageDto {
    FolderStorageDto {
        path,
        free_space: -1,
        used_space: -1,
        storage_type: None,
        device_id: None,
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
