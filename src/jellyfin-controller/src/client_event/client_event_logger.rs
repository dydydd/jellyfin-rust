use std::{io, path::PathBuf};

use chrono::Utc;
use jellyfin_extensions::PathHelper;
use thiserror::Error;
use tokio::{
    fs::OpenOptions,
    io::{AsyncRead, AsyncWriteExt},
};
use tracing::{info, warn};
use uuid::Uuid;

/// Writes client-uploaded diagnostics into the configured server log folder.
#[derive(Debug, Clone)]
pub struct ClientEventLogger {
    log_directory: PathBuf,
}

impl ClientEventLogger {
    #[must_use]
    pub fn new(log_directory: impl Into<PathBuf>) -> Self {
        Self {
            log_directory: log_directory.into(),
        }
    }

    /// Writes one client log to a uniquely named file inside the log folder.
    ///
    /// # Errors
    ///
    /// Returns [`ClientEventLogError::UnsafePath`] if the generated path does
    /// not remain in the configured log directory, or an I/O error if path
    /// resolution, file creation, copying, or flushing fails.
    pub async fn write_document<R>(
        &self,
        client_name: &str,
        client_version: &str,
        file_contents: &mut R,
    ) -> Result<String, ClientEventLogError>
    where
        R: AsyncRead + Unpin + ?Sized,
    {
        let safe_client_name =
            PathHelper::get_safe_leaf_file_name(client_name).unwrap_or("unknown-client");
        let safe_client_version =
            PathHelper::get_safe_leaf_file_name(client_version).unwrap_or("unknown-version");
        let timestamp = Utc::now().format("%Y%m%d%H%M%S");
        let unique_id = Uuid::new_v4().simple();
        let file_name =
            format!("upload_{safe_client_name}_{safe_client_version}_{timestamp}_{unique_id}.log");
        let log_file_path = self.log_directory.join(&file_name);

        if !PathHelper::is_contained_in(&self.log_directory, &log_file_path)? {
            warn!(
                client_name,
                client_version,
                file_name,
                log_directory = %self.log_directory.display(),
                "rejected client event log path outside configured directory"
            );
            return Err(ClientEventLogError::UnsafePath);
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log_file_path)
            .await?;
        let bytes_written = tokio::io::copy(file_contents, &mut file).await?;
        file.flush().await?;

        info!(
            client_name,
            client_version, file_name, bytes_written, "stored client event log"
        );
        Ok(file_name)
    }
}

#[derive(Debug, Error)]
pub enum ClientEventLogError {
    #[error("client event log path escapes the configured log directory")]
    UnsafePath,
    #[error(transparent)]
    Io(#[from] io::Error),
}
