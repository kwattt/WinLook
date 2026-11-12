//! GitHub Releases-based application updater subsystem for WinLook.

pub mod api;
pub mod download;
pub mod repo;
pub mod version;

pub use api::{ApiError, GitHubApiClient, GitHubRelease, ReleaseAsset, select_best_asset};
pub use download::{
    DownloadError, DownloadProgress, download_asset, execute_installer, get_updates_temp_dir,
};
pub use repo::{
    DEFAULT_GITHUB_REPO, DEFAULT_REPO_NAME, DEFAULT_REPO_OWNER, detect_repository,
    parse_github_repo_url,
};
pub use version::{
    VersionError, compare_versions, current_app_version, is_update_available,
    normalize_version_string,
};

/// High-level status of an update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available(GitHubRelease),
    RateLimited(u64), // reset unix timestamp
    Failed(String),
}

/// The result returned from `check_for_updates()`.
#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub status: UpdateStatus,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub target_asset: Option<ReleaseAsset>,
}

/// Orchestrator client for checking updates from GitHub Releases.
#[derive(Debug, Clone)]
pub struct GitHubUpdaterClient {
    api_client: GitHubApiClient,
    current_version: String,
}

impl GitHubUpdaterClient {
    /// Creates a new updater client targeting `owner/repo`.
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self {
            api_client: GitHubApiClient::new(owner, repo),
            current_version: current_app_version().to_string(),
        }
    }

    /// Creates an updater client with a custom base URL (e.g. for mock testing).
    pub fn with_base_url(
        base_url: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            api_client: GitHubApiClient::with_base_url(base_url, owner, repo),
            current_version: current_app_version().to_string(),
        }
    }

    /// Creates an updater client using the repository detected from configuration or Git metadata.
    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        let (owner, repo) = detect_repository(config.github_repo.as_deref());
        Self::new(owner, repo)
    }

    /// Queries GitHub Releases for the latest release and evaluates whether an update is available.
    pub async fn check_for_updates(&self) -> UpdateCheckResult {
        let current_ver = self.current_version.clone();
        match self.api_client.fetch_latest_release().await {
            Ok(release) => {
                match is_update_available(&current_ver, &release.tag_name) {
                    Ok(true) => {
                        let target_asset = select_best_asset(&release.assets).cloned();
                        UpdateCheckResult {
                            status: UpdateStatus::Available(release.clone()),
                            current_version: current_ver,
                            latest_version: Some(release.tag_name),
                            target_asset,
                        }
                    }
                    Ok(false) => UpdateCheckResult {
                        status: UpdateStatus::UpToDate,
                        current_version: current_ver,
                        latest_version: Some(release.tag_name),
                        target_asset: None,
                    },
                    Err(err) => UpdateCheckResult {
                        status: UpdateStatus::Failed(format!("Version parsing error: {}", err)),
                        current_version: current_ver,
                        latest_version: Some(release.tag_name),
                        target_asset: None,
                    },
                }
            }
            Err(ApiError::RateLimited { reset_timestamp }) => UpdateCheckResult {
                status: UpdateStatus::RateLimited(reset_timestamp),
                current_version: current_ver,
                latest_version: None,
                target_asset: None,
            },
            Err(err) => UpdateCheckResult {
                status: UpdateStatus::Failed(err.to_string()),
                current_version: current_ver,
                latest_version: None,
                target_asset: None,
            },
        }
    }
}
