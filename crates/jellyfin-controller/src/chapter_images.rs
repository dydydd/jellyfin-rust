use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use jellyfin_data::{
    BaseItemError, BaseItemQuery, BaseItemRepository, ChapterRepository, ChapterStoreError,
};
use thiserror::Error;
use tokio::{fs, process::Command};
use uuid::Uuid;

const FIRST_CHAPTER_FALLBACK_SECONDS: i64 = 15;
const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Debug, Error)]
pub enum ChapterImageError {
    #[error(transparent)]
    Store(#[from] BaseItemError),
    #[error(transparent)]
    Chapter(#[from] ChapterStoreError),
    #[error("failed to extract chapter image: {stderr}")]
    Ffmpeg { stderr: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterImageTask {
    pub chapter_id: Uuid,
    pub source_path: String,
    pub output_path: String,
    pub position_seconds: u64,
}

/// Generates chapter thumbnails and persists their storage metadata.
#[derive(Clone)]
pub struct ChapterImageService {
    items: BaseItemRepository,
    chapters: ChapterRepository,
    ffmpeg_path: PathBuf,
    storage_directory: Arc<PathBuf>,
}

impl ChapterImageService {
    #[must_use]
    pub fn new(
        database: impl Into<jellyfin_data::SharedDatabase>,
        storage_directory: impl Into<PathBuf>,
        ffmpeg_path: impl Into<PathBuf>,
    ) -> Self {
        let database = database.into();
        Self {
            items: BaseItemRepository::new(Arc::clone(&database)),
            chapters: ChapterRepository::new(database),
            ffmpeg_path: ffmpeg_path.into(),
            storage_directory: Arc::new(storage_directory.into()),
        }
    }

    /// Replaces the directory used to store generated chapter images.
    pub fn set_storage_directory(&mut self, storage_directory: impl Into<PathBuf>) {
        self.storage_directory = Arc::new(storage_directory.into());
    }

    /// Replaces the `FFmpeg` binary used by extraction.
    pub fn set_ffmpeg_path(&mut self, ffmpeg_path: impl Into<PathBuf>) {
        self.ffmpeg_path = ffmpeg_path.into();
    }

    /// Extracts images for library videos with chapters.
    ///
    /// Individual items and chapters are best effort: failures are logged and
    /// remaining work continues.
    ///
    /// # Errors
    ///
    /// Returns a catalog error when the video query cannot be loaded.
    pub async fn refresh_all(&self) -> Result<(), ChapterImageError> {
        self.refresh_all_with_paths(&self.storage_directory, &self.ffmpeg_path)
            .await
    }

    /// Extracts chapter images using a per-run storage directory and `FFmpeg` binary.
    ///
    /// # Errors
    ///
    /// Returns a catalog error when the video query cannot be loaded.
    pub async fn refresh_all_with_paths(
        &self,
        storage_directory: &Path,
        ffmpeg_path: &Path,
    ) -> Result<(), ChapterImageError> {
        let query = BaseItemQuery {
            is_folder: Some(false),
            is_virtual_item: Some(false),
            media_types: vec!["Video".to_owned()],
            ..BaseItemQuery::default()
        };
        let page = self.items.query(&query).await?;
        for item in page.items {
            self.refresh_item_with_paths(
                &item.id,
                item.path.as_deref(),
                item.runtime_ticks,
                storage_directory,
                ffmpeg_path,
            )
            .await;
        }
        Ok(())
    }

    /// Extracts images for one item, tolerating missing files and bad chapters.
    pub async fn refresh_item(
        &self,
        item_id: &Uuid,
        source_path: Option<&str>,
        runtime_ticks: Option<i64>,
    ) {
        self.refresh_item_with_paths(
            item_id,
            source_path,
            runtime_ticks,
            &self.storage_directory,
            &self.ffmpeg_path,
        )
        .await;
    }

    async fn refresh_item_with_paths(
        &self,
        item_id: &Uuid,
        source_path: Option<&str>,
        runtime_ticks: Option<i64>,
        storage_directory: &Path,
        ffmpeg_path: &Path,
    ) {
        let Some(path) = source_path.filter(|path| !path.trim().is_empty()) else {
            return;
        };
        if !fs::try_exists(Path::new(path)).await.unwrap_or_default() {
            return;
        }
        let Ok(chapters) = self.chapters.list_for_item(*item_id).await else {
            return;
        };
        for chapter in chapters {
            if chapter.image_path.is_some() {
                continue;
            }
            let task = ChapterImageTask {
                chapter_id: chapter.id,
                source_path: path.to_owned(),
                output_path: Self::image_path(storage_directory, *item_id, Utc::now(), chapter.id)
                    .to_string_lossy()
                    .into_owned(),
                position_seconds: chapter_image_position_seconds(
                    chapter.start_position_ticks,
                    runtime_ticks,
                ),
            };
            if let Err(error) = self.generate_with_ffmpeg(task, ffmpeg_path).await {
                tracing::warn!(
                    item_id = %item_id,
                    chapter_id = %chapter.id,
                    %error,
                    "skipped chapter image"
                );
            }
        }
    }

    /// Extracts one chapter image and persists its storage metadata.
    ///
    /// # Errors
    ///
    /// Returns I/O, process, or persistence failures for one chapter.
    pub async fn generate(&self, task: ChapterImageTask) -> Result<(), ChapterImageError> {
        self.generate_with_ffmpeg(task, &self.ffmpeg_path).await
    }

    async fn generate_with_ffmpeg(
        &self,
        task: ChapterImageTask,
        ffmpeg_path: &Path,
    ) -> Result<(), ChapterImageError> {
        if fs::try_exists(Path::new(&task.output_path))
            .await
            .unwrap_or_default()
        {
            return Ok(());
        }
        if let Some(parent) = Path::new(&task.output_path).parent() {
            fs::create_dir_all(parent).await?;
        }
        let seconds = format_seconds(task.position_seconds);
        let output = Command::new(ffmpeg_path)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                &seconds,
                "-i",
                &task.source_path,
                "-frames:v",
                "1",
                "-q:v",
                "2",
            ])
            .arg(&task.output_path)
            .stdin(Stdio::null())
            .output()
            .await?;
        if !output.status.success() {
            let _ = fs::remove_file(&task.output_path).await;
            return Err(ChapterImageError::Ffmpeg {
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let modified: DateTime<Utc> = fs::metadata(&task.output_path).await?.modified()?.into();
        self.chapters
            .set_image_data(task.chapter_id, &task.output_path, modified)
            .await?;
        Ok(())
    }

    fn image_path(
        storage_directory: &Path,
        item_id: Uuid,
        date_modified: DateTime<Utc>,
        chapter_id: Uuid,
    ) -> PathBuf {
        let modified = date_modified
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .to_string();
        storage_directory
            .join(format!("{:02x}", item_id.as_bytes()[0]))
            .join(item_id.simple().to_string())
            .join(format!("{modified}_{}.jpg", chapter_id.simple()))
    }
}

/// Applies the official first-chapter lead-in while clamping to runtime.
#[must_use]
pub fn chapter_image_position_ticks(start_position_ticks: i64, runtime_ticks: Option<i64>) -> i64 {
    if start_position_ticks != 0 {
        return start_position_ticks.max(0);
    }
    let fallback = FIRST_CHAPTER_FALLBACK_SECONDS.saturating_mul(TICKS_PER_SECOND);
    runtime_ticks.map_or(fallback, |runtime| fallback.min(runtime.max(0)))
}

fn chapter_image_position_seconds(start_position_ticks: i64, runtime_ticks: Option<i64>) -> u64 {
    let ticks = chapter_image_position_ticks(start_position_ticks, runtime_ticks);
    u64::try_from(ticks / TICKS_PER_SECOND).unwrap_or(0)
}

fn format_seconds(seconds: u64) -> String {
    let whole = seconds / 1_000_000;
    let fraction = seconds % 1_000_000;
    format!("{whole}.{fraction:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_chapter_uses_runtime_limited_lead_in() {
        assert_eq!(
            chapter_image_position_ticks(0, Some(30 * TICKS_PER_SECOND)),
            15 * TICKS_PER_SECOND
        );
        assert_eq!(
            chapter_image_position_ticks(0, Some(9 * TICKS_PER_SECOND)),
            9 * TICKS_PER_SECOND
        );
        assert_eq!(
            chapter_image_position_ticks(40 * TICKS_PER_SECOND, Some(9 * TICKS_PER_SECOND)),
            40 * TICKS_PER_SECOND
        );
    }

    #[test]
    fn negative_chapter_positions_are_not_sent_to_ffmpeg() {
        assert_eq!(chapter_image_position_ticks(-1, None), 0);
    }
}
