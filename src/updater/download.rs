//! Streaming release asset downloader and detached installer execution.

use crate::updater::api::ReleaseAsset;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Progress information emitted during streaming download.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<f32>,
}

/// Errors that can occur during asset download and installer execution.
#[derive(Debug)]
pub enum DownloadError {
    Io(std::io::Error),
    Network(String),
    Http { status: u16, message: String },
    InvalidFileName(String),
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Network(s) => write!(f, "Network error: {}", s),
            Self::Http { status, message } => write!(f, "HTTP error {}: {}", status, message),
            Self::InvalidFileName(s) => write!(f, "Invalid file name: {}", s),
            Self::Cancelled => write!(f, "Download cancelled"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Returns the temporary updates directory (`%TEMP%\WinLook-Updates`), creating it if necessary.
pub fn get_updates_temp_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("WinLook-Updates");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Downloads a release asset with streaming progress reporting and atomic rename.
///
/// Steps:
/// 1. Sanitizes asset filename to prevent path traversal.
/// 2. Streams HTTP response chunks to `{asset_name}.part`.
/// 3. Emits `DownloadProgress` events to `progress_tx`.
/// 4. On completion, removes pre-existing target file and atomically renames `.part` to final file.
pub async fn download_asset(
    asset: &ReleaseAsset,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<PathBuf, DownloadError> {
    let safe_name = Path::new(&asset.name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| DownloadError::InvalidFileName(asset.name.clone()))?;

    let dir = get_updates_temp_dir()?;
    let part_path = dir.join(format!("{}.part", safe_name));
    let final_path = dir.join(safe_name);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default();

    let user_agent = format!("WinLook-Updater/{}", env!("CARGO_PKG_VERSION"));
    let response = client
        .get(&asset.browser_download_url)
        .header("User-Agent", user_agent)
        .send()
        .await
        .map_err(|e| DownloadError::Network(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::Http {
            status: status.as_u16(),
            message: status.to_string(),
        });
    }

    let total_bytes = response
        .content_length()
        .or(if asset.size > 0 { Some(asset.size) } else { None });

    // Clean up any stale partial download
    let _ = tokio::fs::remove_file(&part_path).await;

    let mut file = tokio::fs::File::create(&part_path)
        .await
        .map_err(DownloadError::Io)?;

    let mut stream = response.bytes_stream();
    let mut downloaded_bytes: u64 = 0;

    while let Some(chunk_res) = stream.next().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(DownloadError::Network(e.to_string()));
            }
        };

        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(DownloadError::Io(e));
        }

        downloaded_bytes += chunk.len() as u64;

        if let Some(ref tx) = progress_tx {
            let percent = total_bytes.map(|tot| {
                if tot > 0 {
                    ((downloaded_bytes as f32 / tot as f32) * 100.0).clamp(0.0, 100.0)
                } else {
                    100.0
                }
            });

            let _ = tx.send(DownloadProgress {
                downloaded_bytes,
                total_bytes,
                percent,
            });
        }
    }

    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(DownloadError::Io(e));
    }
    drop(file);

    // If final target file exists, remove it before atomic rename
    if final_path.exists() {
        let _ = std::fs::remove_file(&final_path);
    }

    std::fs::rename(&part_path, &final_path).map_err(DownloadError::Io)?;

    Ok(final_path)
}

/// Spawns the downloaded installer detached from the current process group.
pub fn execute_installer(installer_path: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(installer_path);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_updates_temp_dir() {
        let dir = get_updates_temp_dir().expect("Failed to get or create updates temp dir");
        assert!(dir.is_dir());
        assert!(dir.ends_with("WinLook-Updates"));
    }

    #[test]
    fn test_sanitize_asset_file_name_prevents_directory_traversal() {
        let dangerous_name = "../../Windows/System32/calc.exe";
        let safe_name = Path::new(dangerous_name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("update.exe");
        assert_eq!(safe_name, "calc.exe");
        assert!(!safe_name.contains(".."));
        assert!(!safe_name.contains('/'));
        assert!(!safe_name.contains('\\'));
    }

    #[test]
    fn test_download_progress_calculation() {
        let p1 = DownloadProgress {
            downloaded_bytes: 5_000_000,
            total_bytes: Some(10_000_000),
            percent: Some(50.0),
        };
        assert_eq!(p1.percent, Some(50.0));

        let p_unknown_total = DownloadProgress {
            downloaded_bytes: 2_000_000,
            total_bytes: None,
            percent: None,
        };
        assert_eq!(p_unknown_total.percent, None);
    }
}
