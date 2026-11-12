//! GitHub Releases REST API client, response models, and asset prioritization.

/// A release asset attached to a GitHub release.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    pub content_type: Option<String>,
}

/// A GitHub release payload returned by the GitHub Releases API.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    pub html_url: String,
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// Typed API errors for GitHub Releases requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    RateLimited { reset_timestamp: u64 },
    NotFound(String),
    HttpError { status: u16, message: String },
    NetworkError(String),
    JsonError(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited { reset_timestamp } => {
                write!(
                    f,
                    "GitHub API rate limit exceeded (resets at unix timestamp {})",
                    reset_timestamp
                )
            }
            Self::NotFound(msg) => write!(f, "Not found: {}", msg),
            Self::HttpError { status, message } => write!(f, "HTTP error {}: {}", status, message),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
            Self::JsonError(msg) => write!(f, "JSON parse error: {}", msg),
        }
    }
}

impl std::error::Error for ApiError {}

/// Selects the best Windows asset from a release based on prioritization rules:
/// 1. `WinLook-Setup-*.exe` (Inno Setup installer, prefers x64 if multiple architecture targets exist)
/// 2. Any `.exe` containing "setup" or "installer"
/// 3. Standalone `winlook.exe`
/// 4. Any other `.exe`
/// 5. `.zip` archive
///
/// Ignores non-Windows files (`.dmg`, `.AppImage`, `.deb`, `.tar.gz`, `.txt`, `.md`, `.sha256`).
pub fn select_best_asset<'a>(assets: &'a [ReleaseAsset]) -> Option<&'a ReleaseAsset> {
    let mut best: Option<(&'a ReleaseAsset, u32)> = None;

    for asset in assets {
        let name_lower = asset.name.to_lowercase();

        // Discard non-windows or non-archive assets
        if name_lower.ends_with(".tar.gz")
            || name_lower.ends_with(".dmg")
            || name_lower.ends_with(".appimage")
            || name_lower.ends_with(".deb")
            || name_lower.ends_with(".txt")
            || name_lower.ends_with(".md")
            || name_lower.ends_with(".sha256")
        {
            continue;
        }

        let is_exe = name_lower.ends_with(".exe");
        let is_zip = name_lower.ends_with(".zip");

        if !is_exe && !is_zip {
            continue;
        }

        let mut score: u32 = 0;

        if is_exe {
            if name_lower.starts_with("winlook-setup-") {
                score = 50;
                if name_lower.contains("x64") || name_lower.contains("x86_64") || name_lower.contains("amd64") {
                    score += 10;
                }
            } else if name_lower.contains("setup") || name_lower.contains("installer") {
                score = 40;
                if name_lower.contains("x64") || name_lower.contains("x86_64") || name_lower.contains("amd64") {
                    score += 5;
                }
            } else if name_lower == "winlook.exe" {
                score = 30;
            } else {
                score = 20;
            }
        } else if is_zip {
            score = 10;
        }

        if score > 0 {
            match best {
                Some((_, current_score)) if score > current_score => {
                    best = Some((asset, score));
                }
                None => {
                    best = Some((asset, score));
                }
                _ => {}
            }
        }
    }

    best.map(|(asset, _)| asset)
}

/// Asynchronous client for interacting with the GitHub Releases API.
#[derive(Debug, Clone)]
pub struct GitHubApiClient {
    client: reqwest::Client,
    base_url: String,
    owner: String,
    repo: String,
}

impl GitHubApiClient {
    /// Creates a new client using the public GitHub API (`https://api.github.com`).
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self::with_base_url("https://api.github.com", owner, repo)
    }

    /// Creates a client with a custom base URL (useful for mock testing or GitHub Enterprise).
    pub fn with_base_url(
        base_url: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();

        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    /// Fetches the latest published release for the configured repository.
    pub async fn fetch_latest_release(&self) -> Result<GitHubRelease, ApiError> {
        let url = format!(
            "{}/repos/{}/{}/releases/latest",
            self.base_url, self.owner, self.repo
        );
        let user_agent = format!("WinLook-Updater/{}", env!("CARGO_PKG_VERSION"));

        let response = self
            .client
            .get(&url)
            .header("User-Agent", user_agent)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| ApiError::NetworkError(e.to_string()))?;

        let status = response.status();

        if status == reqwest::StatusCode::FORBIDDEN {
            let reset_ts = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            return Err(ApiError::RateLimited {
                reset_timestamp: reset_ts,
            });
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(ApiError::NotFound(format!(
                "No releases found for {}/{}",
                self.owner, self.repo
            )));
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::HttpError {
                status: status.as_u16(),
                message: body,
            });
        }

        response
            .json::<GitHubRelease>()
            .await
            .map_err(|e| ApiError::JsonError(e.to_string()))
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    fn make_asset(name: &str, size: u64) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            size,
            browser_download_url: format!(
                "https://github.com/kwattt/WinLook/releases/download/v0.2.0/{}",
                name
            ),
            content_type: Some("application/octet-stream".to_string()),
        }
    }

    #[test]
    fn test_select_best_asset_prioritizes_inno_setup_installer() {
        let assets = vec![
            make_asset("checksums.txt", 128),
            make_asset("winlook-v0.2.0-x64.zip", 10_000_000),
            make_asset("winlook.exe", 12_000_000),
            make_asset("WinLook-Setup-0.2.0-x64.exe", 15_000_000),
            make_asset("README.md", 1024),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "WinLook-Setup-0.2.0-x64.exe");
    }

    #[test]
    fn test_select_best_asset_fallback_to_standalone_exe() {
        let assets = vec![
            make_asset("checksums.txt", 128),
            make_asset("winlook-v0.2.0-x64.zip", 10_000_000),
            make_asset("winlook.exe", 12_000_000),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "winlook.exe");
    }

    #[test]
    fn test_select_best_asset_fallback_to_zip() {
        let assets = vec![
            make_asset("checksums.txt", 128),
            make_asset("WinLook-v0.2.0-windows-x64.zip", 10_000_000),
            make_asset("source.tar.gz", 5_000_000),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "WinLook-v0.2.0-windows-x64.zip");
    }

    #[test]
    fn test_select_best_asset_case_insensitivity() {
        let assets = vec![
            make_asset("WINLOOK-SETUP-0.2.0-X64.EXE", 15_000_000),
            make_asset("WINLOOK.EXE", 12_000_000),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "WINLOOK-SETUP-0.2.0-X64.EXE");
    }

    #[test]
    fn test_select_best_asset_ignores_non_windows_assets() {
        let assets = vec![
            make_asset("WinLook-darwin-arm64.dmg", 20_000_000),
            make_asset("WinLook-linux-x86_64.AppImage", 22_000_000),
            make_asset("winlook_0.2.0_amd64.deb", 18_000_000),
            make_asset("checksums.sha256", 256),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_none());
    }

    #[test]
    fn test_select_best_asset_empty_list() {
        let assets: Vec<ReleaseAsset> = Vec::new();
        assert!(select_best_asset(&assets).is_none());
    }

    #[test]
    fn test_select_best_asset_multiple_installers_prefers_x64() {
        let assets = vec![
            make_asset("WinLook-Setup-0.2.0-arm64.exe", 14_000_000),
            make_asset("WinLook-Setup-0.2.0-x64.exe", 15_000_000),
        ];

        let best = select_best_asset(&assets);
        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "WinLook-Setup-0.2.0-x64.exe");
    }
}

#[cfg(test)]
mod api_payload_and_mock_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    // --- GitHub Releases API JSON Fixtures ---
    const VALID_GITHUB_RELEASE_JSON: &str = r####"{
        "url": "https://api.github.com/repos/kwattt/WinLook/releases/180000000",
        "html_url": "https://github.com/kwattt/WinLook/releases/tag/v0.2.0",
        "id": 180000000,
        "tag_name": "v0.2.0",
        "name": "WinLook v0.2.0 - Multi-Monitor Support",
        "body": "## What's Changed\r\n* Multi-monitor capture across all screens\r\n* In-app updater support",
        "draft": false,
        "prerelease": false,
        "published_at": "2026-11-12T10:15:00Z",
        "assets": [
            {
                "name": "WinLook-Setup-0.2.0-x64.exe",
                "size": 15728640,
                "browser_download_url": "https://github.com/kwattt/WinLook/releases/download/v0.2.0/WinLook-Setup-0.2.0-x64.exe",
                "content_type": "application/x-msdownload"
            },
            {
                "name": "winlook.exe",
                "size": 12582912,
                "browser_download_url": "https://github.com/kwattt/WinLook/releases/download/v0.2.0/winlook.exe",
                "content_type": "application/x-msdownload"
            }
        ]
    }"####;

    const NULLABLE_FIELDS_RELEASE_JSON: &str = r#"{
        "tag_name": "v0.3.0",
        "name": null,
        "body": null,
        "draft": false,
        "prerelease": true,
        "html_url": "https://github.com/kwattt/WinLook/releases/tag/v0.3.0",
        "published_at": null,
        "assets": []
    }"#;

    const RATE_LIMIT_BODY: &str = r#"{
        "message": "API rate limit exceeded for 127.0.0.1. (But here's the good news: Authenticated requests get a higher rate limit.)",
        "documentation_url": "https://docs.github.com/rest/overview/resources-in-the-rest-api#rate-limiting"
    }"#;

    const NOT_FOUND_BODY: &str = r#"{
        "message": "Not Found",
        "documentation_url": "https://docs.github.com/rest/releases/releases#get-the-latest-release"
    }"#;

    // --- JSON Deserialization Tests (Pure In-Memory) ---
    #[test]
    fn test_parse_valid_github_release_json() {
        let release: GitHubRelease = serde_json::from_str(VALID_GITHUB_RELEASE_JSON)
            .expect("Failed to deserialize valid GitHub release payload");

        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(
            release.name.as_deref(),
            Some("WinLook v0.2.0 - Multi-Monitor Support")
        );
        assert_eq!(release.draft, false);
        assert_eq!(release.prerelease, false);
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].name, "WinLook-Setup-0.2.0-x64.exe");
        assert_eq!(release.assets[0].size, 15728640);
        assert_eq!(
            release.assets[0].browser_download_url,
            "https://github.com/kwattt/WinLook/releases/download/v0.2.0/WinLook-Setup-0.2.0-x64.exe"
        );
    }

    #[test]
    fn test_parse_release_json_with_nullable_optional_fields() {
        let release: GitHubRelease = serde_json::from_str(NULLABLE_FIELDS_RELEASE_JSON)
            .expect("Failed to deserialize release with null optional fields");

        assert_eq!(release.tag_name, "v0.3.0");
        assert_eq!(release.name, None);
        assert_eq!(release.body, None);
        assert_eq!(release.published_at, None);
        assert_eq!(release.prerelease, true);
        assert!(release.assets.is_empty());
    }

    #[test]
    fn test_parse_malformed_json_returns_error() {
        let err1 = serde_json::from_str::<GitHubRelease>(r#"{"tag_name": "v0.2.0", "as"#);
        assert!(err1.is_err());

        let err2 = serde_json::from_str::<GitHubRelease>("");
        assert!(err2.is_err());

        let html = "<!DOCTYPE html><html><body>502 Bad Gateway</body></html>";
        let err3 = serde_json::from_str::<GitHubRelease>(html);
        assert!(err3.is_err());

        let missing = r#"{"name": "test release", "draft": false}"#;
        let err4 = serde_json::from_str::<GitHubRelease>(missing);
        assert!(err4.is_err());
    }

    // --- Mock HTTP Server Test Harness ---
    fn spawn_mock_server(
        status_code: u16,
        extra_headers: Vec<(&'static str, String)>,
        body: &'static str,
    ) -> (String, mpsc::Receiver<String>) {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("Failed to bind ephemeral mock port");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let bytes_read = stream.read(&mut buf).unwrap_or(0);
                let raw_request = String::from_utf8_lossy(&buf[..bytes_read]).to_string();
                let _ = tx.send(raw_request);

                let status_line = match status_code {
                    200 => "HTTP/1.1 200 OK",
                    403 => "HTTP/1.1 403 Forbidden",
                    404 => "HTTP/1.1 404 Not Found",
                    500 => "HTTP/1.1 500 Internal Server Error",
                    code => panic!("Unsupported status code in mock: {}", code),
                };

                let mut headers = format!(
                    "{}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n",
                    status_line,
                    body.len()
                );
                for (k, v) in extra_headers {
                    headers.push_str(&format!("{}: {}\r\n", k, v));
                }
                headers.push_str("\r\n");

                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });

        (format!("http://127.0.0.1:{}", port), rx)
    }

    // --- Wire-Level Mock Server Client Tests ---
    #[tokio::test]
    async fn test_mock_client_fetch_latest_release_success_and_headers() {
        let (base_url, rx) = spawn_mock_server(200, vec![], VALID_GITHUB_RELEASE_JSON);

        let client = GitHubApiClient::with_base_url(&base_url, "kwattt", "WinLook");
        let release = client
            .fetch_latest_release()
            .await
            .expect("API client fetch failed");

        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(release.assets.len(), 2);

        let raw_req = rx.recv().expect("Mock server did not receive request");
        let req_lower = raw_req.to_lowercase();
        assert!(req_lower.contains("user-agent: winlook-updater/"));
        assert!(req_lower.contains("accept: application/vnd.github+json"));
        assert!(req_lower.contains("x-github-api-version: 2022-11-28"));
        assert!(raw_req.contains("GET /repos/kwattt/WinLook/releases/latest HTTP/1.1"));
    }

    #[tokio::test]
    async fn test_mock_client_rate_limit_detection_and_reset_header() {
        let reset_ts = 1762974000u64;
        let headers = vec![
            ("x-ratelimit-remaining", "0".to_string()),
            ("x-ratelimit-reset", reset_ts.to_string()),
        ];
        let (base_url, _rx) = spawn_mock_server(403, headers, RATE_LIMIT_BODY);

        let client = GitHubApiClient::with_base_url(&base_url, "kwattt", "WinLook");
        let result = client.fetch_latest_release().await;

        match result {
            Err(ApiError::RateLimited { reset_timestamp }) => {
                assert_eq!(reset_timestamp, reset_ts);
            }
            other => panic!("Expected ApiError::RateLimited, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mock_client_not_found_404_detection() {
        let (base_url, _rx) = spawn_mock_server(404, vec![], NOT_FOUND_BODY);

        let client = GitHubApiClient::with_base_url(&base_url, "kwattt", "WinLook");
        let result = client.fetch_latest_release().await;

        match result {
            Err(ApiError::NotFound(msg)) => {
                assert!(msg.contains("kwattt/WinLook"));
            }
            other => panic!("Expected ApiError::NotFound, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mock_client_server_error_500() {
        let (base_url, _rx) = spawn_mock_server(500, vec![], "Internal Server Error");

        let client = GitHubApiClient::with_base_url(&base_url, "kwattt", "WinLook");
        let result = client.fetch_latest_release().await;

        match result {
            Err(ApiError::HttpError { status, .. }) => {
                assert_eq!(status, 500);
            }
            other => panic!("Expected ApiError::HttpError, got: {:?}", other),
        }
    }
}
