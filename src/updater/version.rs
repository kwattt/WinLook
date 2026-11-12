//! Tolerant Semantic Versioning (SemVer) parsing, normalization, and comparison.

use semver::Version;

/// Errors that can occur during version normalization and comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    EmptyString,
    InvalidFormat(String),
    SemVerParseError(String),
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyString => write!(f, "Version string is empty"),
            Self::InvalidFormat(s) => write!(f, "Invalid version format: {}", s),
            Self::SemVerParseError(s) => write!(f, "SemVer parse error: {}", s),
        }
    }
}

impl std::error::Error for VersionError {}

/// Returns the current application compile-time version from Cargo.
pub fn current_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Normalizes a human or git tag version string into a standard SemVer `Version`.
///
/// Features:
/// - Strips leading/trailing whitespace.
/// - Strips leading `'v'` or `'V'`.
/// - Normalizes missing minor/patch segments (e.g. `"1"` -> `"1.0.0"`, `"0.1"` -> `"0.1.0"`).
/// - Preserves prerelease identifiers (e.g. `"0.2-rc.1"` -> `"0.2.0-rc.1"`).
pub fn normalize_version_string(raw: &str) -> Result<Version, VersionError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(VersionError::EmptyString);
    }

    // Strip leading 'v' or 'V'
    let without_v = if trimmed.starts_with('v') || trimmed.starts_with('V') {
        &trimmed[1..]
    } else {
        trimmed
    };

    if without_v.is_empty() {
        return Err(VersionError::EmptyString);
    }

    // Must start with an ASCII digit
    if !without_v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Err(VersionError::InvalidFormat(raw.to_string()));
    }

    // Locate the first prerelease or build metadata delimiter ('-' or '+')
    let (main_part, suffix) = match without_v.find(['-', '+']) {
        Some(idx) => (&without_v[..idx], &without_v[idx..]),
        None => (without_v, ""),
    };

    // Split main numeric part by '.'
    let segments: Vec<&str> = main_part.split('.').collect();
    if segments.is_empty() {
        return Err(VersionError::EmptyString);
    }

    // Validate that each segment is an integer
    for seg in &segments {
        if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_digit()) {
            return Err(VersionError::InvalidFormat(raw.to_string()));
        }
    }

    let normalized_main = match segments.len() {
        1 => format!("{}.0.0", segments[0]),
        2 => format!("{}.{}.0", segments[0], segments[1]),
        3 => format!("{}.{}.{}", segments[0], segments[1], segments[2]),
        _ => return Err(VersionError::InvalidFormat(format!("Too many version segments: {}", raw))),
    };

    let full_normalized = format!("{}{}", normalized_main, suffix);
    Version::parse(&full_normalized)
        .map_err(|e| VersionError::SemVerParseError(e.to_string()))
}

/// Compares two version strings using SemVer 2.0 precedence.
pub fn compare_versions(current_str: &str, latest_str: &str) -> Result<std::cmp::Ordering, VersionError> {
    let current_ver = normalize_version_string(current_str)?;
    let latest_ver = normalize_version_string(latest_str)?;
    let ord = (current_ver.major, current_ver.minor, current_ver.patch, &current_ver.pre)
        .cmp(&(latest_ver.major, latest_ver.minor, latest_ver.patch, &latest_ver.pre));
    Ok(ord)
}

/// Returns `true` if `latest_str` represents a version strictly greater than `current_str`.
pub fn is_update_available(current_str: &str, latest_str: &str) -> Result<bool, VersionError> {
    Ok(compare_versions(current_str, latest_str)? == std::cmp::Ordering::Less)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 'v' / 'V' Prefix Stripping Tests ---
    #[test]
    fn test_normalize_version_v_prefix() {
        assert_eq!(normalize_version_string("v1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(normalize_version_string("V2.0.0").unwrap().to_string(), "2.0.0");
        assert_eq!(normalize_version_string("v0.1.0").unwrap().to_string(), "0.1.0");
        assert_eq!(normalize_version_string("1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(normalize_version_string("  v1.0.0  ").unwrap().to_string(), "1.0.0");
    }

    // --- Missing Minor / Patch Normalization Tests ---
    #[test]
    fn test_normalize_version_missing_segments() {
        assert_eq!(normalize_version_string("0.1").unwrap().to_string(), "0.1.0");
        assert_eq!(normalize_version_string("v0.2").unwrap().to_string(), "0.2.0");
        assert_eq!(normalize_version_string("1").unwrap().to_string(), "1.0.0");
        assert_eq!(normalize_version_string("V2").unwrap().to_string(), "2.0.0");
        assert_eq!(normalize_version_string("0.2-rc.1").unwrap().to_string(), "0.2.0-rc.1");
        assert_eq!(normalize_version_string("v1-alpha").unwrap().to_string(), "1.0.0-alpha");
    }

    // --- SemVer Ordering Tests (Major, Minor, Patch, Multi-Digit) ---
    #[test]
    fn test_is_update_available_major_bump() {
        assert!(is_update_available("0.1.0", "1.0.0").unwrap());
        assert!(is_update_available("0.9.5", "1.0.0").unwrap());
    }

    #[test]
    fn test_is_update_available_minor_bump() {
        assert!(is_update_available("0.1.0", "0.2.0").unwrap());
        assert!(is_update_available("0.1.5", "0.2.0").unwrap());
    }

    #[test]
    fn test_is_update_available_patch_bump() {
        assert!(is_update_available("0.1.0", "0.1.1").unwrap());
        assert!(is_update_available("0.1.9", "0.1.10").unwrap());
    }

    #[test]
    fn test_is_update_available_numerical_ordering_not_lexical() {
        // "10" must be greater than "9" (lexically "10" < "9", but numerically 10 > 9)
        assert!(is_update_available("0.9.0", "0.10.0").unwrap());
        assert!(is_update_available("0.1.9", "0.1.10").unwrap());
        assert!(!is_update_available("0.10.0", "0.9.0").unwrap());
    }

    // --- Prerelease Semantics Tests ---
    #[test]
    fn test_prerelease_semantics() {
        // Release is strictly newer than its own prerelease
        assert!(is_update_available("0.2.0-rc.1", "0.2.0").unwrap());
        assert!(!is_update_available("0.2.0", "0.2.0-rc.1").unwrap());

        // Higher version prerelease is newer than lower version stable
        assert!(is_update_available("0.1.0", "0.2.0-rc.1").unwrap());
        assert!(is_update_available("0.1.0", "v0.2.0-alpha").unwrap());

        // Prerelease progression ordering
        assert!(is_update_available("0.2.0-alpha", "0.2.0-beta").unwrap());
        assert!(is_update_available("0.2.0-beta.1", "0.2.0-beta.2").unwrap());
        assert!(is_update_available("0.2.0-beta.2", "0.2.0-rc.1").unwrap());
        assert!(!is_update_available("0.2.0-rc.1", "0.2.0-beta.2").unwrap());

        // Prerelease of older version is not newer than current stable
        assert!(!is_update_available("0.2.0", "0.1.0-alpha").unwrap());
    }

    // --- Same Version Tests ---
    #[test]
    fn test_is_update_available_same_version_returns_false() {
        assert!(!is_update_available("0.1.0", "0.1.0").unwrap());
        assert!(!is_update_available("0.1.0", "v0.1.0").unwrap());
        assert!(!is_update_available("0.1.0", "V0.1.0").unwrap());
        assert!(!is_update_available("0.1.0", "0.1").unwrap());
        assert!(!is_update_available("0.1.0", "v0.1").unwrap());
    }

    // --- Older Version Tests ---
    #[test]
    fn test_is_update_available_older_version_returns_false() {
        assert!(!is_update_available("0.1.0", "0.0.9").unwrap());
        assert!(!is_update_available("1.0.0", "0.9.9").unwrap());
        assert!(!is_update_available("0.2.0", "0.1.9").unwrap());
        assert!(!is_update_available("0.1.0", "v0.0.1").unwrap());
    }

    // --- Invalid Version String Error Handling ---
    #[test]
    fn test_invalid_version_strings_return_error() {
        assert!(matches!(normalize_version_string(""), Err(VersionError::EmptyString)));
        assert!(matches!(normalize_version_string("   "), Err(VersionError::EmptyString)));
        assert!(matches!(normalize_version_string("invalid"), Err(VersionError::InvalidFormat(_))));
        assert!(matches!(normalize_version_string("v.1.2"), Err(VersionError::InvalidFormat(_))));
        assert!(matches!(normalize_version_string("1.2.3.4.5"), Err(VersionError::InvalidFormat(_))));
        assert!(is_update_available("0.1.0", "not-a-version").is_err());
    }

    // --- Current App Version Baseline Test ---
    #[test]
    fn test_current_app_version_matches_cargo_pkg() {
        let current = current_app_version();
        assert_eq!(current, env!("CARGO_PKG_VERSION"));
        let parsed = normalize_version_string(current).expect("CARGO_PKG_VERSION must parse");
        assert_eq!(parsed.major, 0);
        assert_eq!(parsed.minor, 1);
        assert_eq!(parsed.patch, 0);
    }
}
