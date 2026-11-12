//! Multi-tier repository detection and GitHub repository URL parsing.

use std::path::PathBuf;

pub const DEFAULT_REPO_OWNER: &str = "kwattt";
pub const DEFAULT_REPO_NAME: &str = "WinLook";
#[allow(dead_code)]
pub const DEFAULT_GITHUB_REPO: &str = "kwattt/WinLook";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Parses a GitHub repository URL or identifier into `(owner, repo)`.
///
/// Supported formats:
/// - SSH: `git@github.com:owner/repo.git`, `ssh://git@github.com/owner/repo.git`
/// - HTTPS/HTTP/GIT: `https://github.com/owner/repo.git`, `http://github.com/owner/repo`
/// - Bare identifier: `owner/repo`
pub fn parse_github_repo_url(raw_url: &str) -> Option<(String, String)> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Strip trailing slashes first
    let s = trimmed.trim_end_matches(['/', '\\']);
    if s.is_empty() {
        return None;
    }

    let repo_path = if s.contains("://") || s.contains('@') {
        const PREFIXES: &[&str] = &[
            "ssh://git@github.com/",
            "git@github.com:",
            "https://github.com/",
            "http://github.com/",
            "git://github.com/",
            "https://www.github.com/",
            "http://www.github.com/",
        ];
        let mut matched = None;
        for prefix in PREFIXES {
            if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
                matched = Some(&s[prefix.len()..]);
                break;
            }
        }
        matched?
    } else {
        // Bare identifier: must not contain protocol or other domain indicators
        if s.contains(':') || s.contains('\\') {
            return None;
        }
        s
    };

    // Clean up repo_path (e.g. if there were multiple trailing slashes before, or .git///)
    let clean_path = repo_path.trim_end_matches(['/', '\\']);
    let segments: Vec<&str> = clean_path.split('/').filter(|seg| !seg.is_empty()).collect();

    // Must have exactly 2 components: owner and repository name
    if segments.len() != 2 {
        return None;
    }

    let owner = segments[0];
    let mut repo = segments[1];

    // Strip .git suffix if present
    if let Some(stripped) = repo.strip_suffix(".git") {
        repo = stripped;
    }

    // GitHub usernames / orgs cannot contain dots or start/end with hyphens; repos cannot be '.' or '..' or start/end with '.'
    if owner.is_empty()
        || repo.is_empty()
        || owner == "."
        || owner == ".."
        || repo == "."
        || repo == ".."
        || repo.starts_with('.')
        || repo.ends_with('.')
        || owner.contains('.')
        || owner.starts_with('-')
        || owner.ends_with('-')
    {
        return None;
    }

    // Validate characters
    let valid_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
    if !owner.chars().all(valid_char) || !repo.chars().all(valid_char) {
        return None;
    }

    Some((owner.to_string(), repo.to_string()))
}

/// Parses a `.git/config` file content to extract the `[remote "origin"]` URL.
pub fn parse_git_config_origin_url(content: &str) -> Option<String> {
    let mut in_origin_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1].trim();
            // Match `remote "origin"` (case-insensitive for section type, exact for origin name)
            in_origin_section = section.eq_ignore_ascii_case("remote \"origin\"");
            continue;
        }

        if in_origin_section {
            if let Some((key, val)) = trimmed.split_once('=') {
                if key.trim().eq_ignore_ascii_case("url") {
                    let url = val.trim();
                    if !url.is_empty() {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Locates `.git/config` by checking the current directory, executable directory,
/// and traversing up to 4 directory levels.
pub fn find_git_config_path() -> Option<PathBuf> {
    // 1. Current working directory and ancestors
    if let Ok(mut dir) = std::env::current_dir() {
        for _ in 0..4 {
            let candidate = dir.join(".git").join("config");
            if candidate.is_file() {
                return Some(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    // 2. Current executable directory and ancestors
    if let Ok(mut exe_dir) = std::env::current_exe() {
        if exe_dir.pop() {
            for _ in 0..4 {
                let candidate = exe_dir.join(".git").join("config");
                if candidate.is_file() {
                    return Some(candidate);
                }
                if !exe_dir.pop() {
                    break;
                }
            }
        }
    }

    None
}

/// Attempts to retrieve the remote origin URL using `git config --get remote.origin.url`.
pub fn get_git_cli_origin_url() -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["config", "--get", "remote.origin.url"]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Multi-tier GitHub repository detection:
/// 1. `WINLOOK_UPDATE_REPO` environment variable or `config_repo` override.
/// 2. `.git/config` pure Rust INI parser.
/// 3. `git config --get remote.origin.url` CLI fallback.
/// 4. Default constant fallback `"kwattt/WinLook"`.
pub fn detect_repository(config_repo: Option<&str>) -> (String, String) {
    // Tier 1: Environment variable or config override
    if let Ok(env_repo) = std::env::var("WINLOOK_UPDATE_REPO") {
        if let Some(pair) = parse_github_repo_url(&env_repo) {
            return pair;
        }
    }

    if let Some(cfg_repo) = config_repo {
        if let Some(pair) = parse_github_repo_url(cfg_repo) {
            return pair;
        }
    }

    // Tier 2: Pure-Rust .git/config parser
    if let Some(config_path) = find_git_config_path() {
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Some(url) = parse_git_config_origin_url(&contents) {
                if let Some(pair) = parse_github_repo_url(&url) {
                    return pair;
                }
            }
        }
    }

    // Tier 3: Git CLI fallback
    if let Some(cli_url) = get_git_cli_origin_url() {
        if let Some(pair) = parse_github_repo_url(&cli_url) {
            return pair;
        }
    }

    // Tier 4: Default fallback
    (DEFAULT_REPO_OWNER.to_string(), DEFAULT_REPO_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SSH Format Tests ---
    #[test]
    fn test_parse_github_url_ssh_with_git_suffix() {
        let parsed = parse_github_repo_url("git@github.com:kwattt/WinLook.git");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_github_url_ssh_without_git_suffix() {
        let parsed = parse_github_repo_url("git@github.com:kwattt/WinLook");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_github_url_ssh_protocol_scheme() {
        let parsed = parse_github_repo_url("ssh://git@github.com/kwattt/WinLook.git");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    // --- HTTPS / HTTP Format Tests ---
    #[test]
    fn test_parse_github_url_https_with_git_suffix() {
        let parsed = parse_github_repo_url("https://github.com/kwattt/WinLook.git");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_github_url_https_without_git_suffix() {
        let parsed = parse_github_repo_url("https://github.com/kwattt/WinLook");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_github_url_https_with_trailing_slashes() {
        let parsed = parse_github_repo_url("https://github.com/kwattt/WinLook/");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));

        let parsed_git = parse_github_repo_url("https://github.com/kwattt/WinLook.git///");
        assert_eq!(parsed_git, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_github_url_http() {
        let parsed = parse_github_repo_url("http://github.com/kwattt/WinLook");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    // --- Bare Identifier & Whitespace Tests ---
    #[test]
    fn test_parse_github_url_bare_identifier() {
        let parsed = parse_github_repo_url("kwattt/WinLook");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));

        let parsed_hyphen = parse_github_repo_url("my-org/my_app-2");
        assert_eq!(parsed_hyphen, Some(("my-org".to_string(), "my_app-2".to_string())));
    }

    #[test]
    fn test_parse_github_url_surrounding_whitespace() {
        let parsed = parse_github_repo_url("   https://github.com/kwattt/WinLook.git \t\r\n ");
        assert_eq!(parsed, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    // --- Negative / Malformed URL Tests ---
    #[test]
    fn test_parse_github_url_rejects_non_github_domains() {
        assert_eq!(parse_github_repo_url("https://gitlab.com/kwattt/WinLook.git"), None);
        assert_eq!(parse_github_repo_url("git@bitbucket.org:kwattt/WinLook.git"), None);
        assert_eq!(parse_github_repo_url("https://evil-github.com/kwattt/WinLook"), None);
        assert_eq!(parse_github_repo_url("https://notgithub.com/kwattt/WinLook"), None);
    }

    #[test]
    fn test_parse_github_url_rejects_malformed_inputs() {
        assert_eq!(parse_github_repo_url(""), None);
        assert_eq!(parse_github_repo_url("   "), None);
        assert_eq!(parse_github_repo_url("github.com"), None);
        assert_eq!(parse_github_repo_url("https://github.com"), None);
        assert_eq!(parse_github_repo_url("https://github.com/"), None);
        assert_eq!(parse_github_repo_url("https://github.com/kwattt"), None);
        assert_eq!(parse_github_repo_url("https://github.com/kwattt/"), None);
        assert_eq!(parse_github_repo_url("https://github.com/kwattt/WinLook/releases"), None);
        assert_eq!(parse_github_repo_url("just_a_random_word"), None);
        assert_eq!(parse_github_repo_url("kwattt/"), None);
        assert_eq!(parse_github_repo_url("/WinLook"), None);
        assert_eq!(parse_github_repo_url("git@github.com:"), None);
        assert_eq!(parse_github_repo_url("git@github.com:kwattt/"), None);
    }

    // --- Git Config INI Parsing Tests ---
    #[test]
    fn test_parse_git_config_origin_url_ssh() {
        let git_config = r#"
[core]
	repositoryformatversion = 0
	filemode = false
	bare = false
[remote "origin"]
	url = git@github.com:kwattt/WinLook.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
	remote = origin
	merge = refs/heads/main
"#;
        let url = parse_git_config_origin_url(git_config);
        assert_eq!(url.as_deref(), Some("git@github.com:kwattt/WinLook.git"));
        let repo = url.and_then(|u| parse_github_repo_url(&u));
        assert_eq!(repo, Some(("kwattt".to_string(), "WinLook".to_string())));
    }

    #[test]
    fn test_parse_git_config_origin_url_https() {
        let git_config = r#"
[remote "upstream"]
	url = https://github.com/upstream/other.git
[remote "origin"]
	url = https://github.com/kwattt/WinLook.git
"#;
        let url = parse_git_config_origin_url(git_config);
        assert_eq!(url.as_deref(), Some("https://github.com/kwattt/WinLook.git"));
    }

    #[test]
    fn test_parse_git_config_missing_origin() {
        let git_config = r#"
[core]
	bare = false
[remote "upstream"]
	url = https://github.com/upstream/other.git
"#;
        let url = parse_git_config_origin_url(git_config);
        assert_eq!(url, None);
    }

    #[test]
    fn test_parse_git_config_empty_or_corrupted() {
        assert_eq!(parse_git_config_origin_url(""), None);
        assert_eq!(parse_git_config_origin_url("[remote origin]\nurl=broken"), None);
    }

    // --- Multi-Tier Detection Tests ---
    #[test]
    fn test_detect_repository_tier1_config_override() {
        let (owner, repo) = detect_repository(Some("custom-owner/custom-repo"));
        assert_eq!(owner, "custom-owner");
        assert_eq!(repo, "custom-repo");
    }

    #[test]
    fn test_detect_repository_tier4_fallback_constant() {
        let (owner, repo) = detect_repository(Some("invalid_string_with_no_slash"));
        assert!(!owner.is_empty());
        assert!(!repo.is_empty());
    }
}
