/// Applies URL replacement rules from an ExtraHub configuration.
///
/// Mirrors Kotlin's `URLReplace.replaceURL()`.
///
/// Three replacement modes:
/// 1. Plain regex: `replace(search_regex, replace_str)` across the full URL.
/// 2. Host-only: if `replace_str` looks like a bare host URL (no path), only
///    the host portion of the original URL is replaced.
/// 3. Template: if `replace_str` contains `{DOWNLOAD_URL}`, the original URL
///    is embedded as a parameter (e.g. proxy wrappers).
pub fn apply_url_replace(url: &str, search: Option<&str>, replace: Option<&str>) -> String {
    let replace_str = match replace {
        Some(r) if !r.is_empty() => r,
        _ => return url.to_string(),
    };

    // Mode 3: template substitution — replace_str contains {DOWNLOAD_URL}
    if replace_str.contains("{DOWNLOAD_URL}") {
        return replace_str.replace("{DOWNLOAD_URL}", url);
    }

    // Mode 2: host-only replacement — replace_str is a bare host URL (no path component)
    if is_host_only(replace_str) {
        return replace_host(url, replace_str);
    }

    // Mode 1: plain regex replacement (or literal if no search)
    match search {
        Some(pattern) if !pattern.is_empty() => match regex::Regex::new(pattern) {
            Ok(re) => re.replace_all(url, replace_str).into_owned(),
            Err(_) => url.replace(pattern, replace_str),
        },
        // No search pattern: nothing to replace
        _ => url.to_string(),
    }
}

/// Returns true if `s` looks like a bare host URL with no meaningful path.
/// e.g. "https://mirror.example.com" or "https://mirror.example.com/"
fn is_host_only(s: &str) -> bool {
    match url::Url::parse(s) {
        Ok(u) => {
            let path = u.path();
            path.is_empty() || path == "/"
        }
        Err(_) => false,
    }
}

/// Replace only the host (scheme + host + port) of `original_url` with the
/// host from `host_url`, keeping the original path, query and fragment.
fn replace_host(original_url: &str, host_url: &str) -> String {
    let orig = match url::Url::parse(original_url) {
        Ok(u) => u,
        Err(_) => return original_url.to_string(),
    };
    let host = match url::Url::parse(host_url) {
        Ok(u) => u,
        Err(_) => return original_url.to_string(),
    };

    // Rebuild: scheme + host from `host`, everything else from `orig`
    let mut result = host.clone();
    result.set_path(orig.path());
    result.set_query(orig.query());
    result.set_fragment(orig.fragment());
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_replace_returns_original() {
        let url = "https://github.com/user/repo/releases/download/v1.0/app.apk";
        assert_eq!(apply_url_replace(url, None, None), url);
        assert_eq!(apply_url_replace(url, None, Some("")), url);
    }

    #[test]
    fn test_download_url_template() {
        let url = "https://github.com/user/repo/releases/download/v1.0/app.apk";
        let replace = "https://ghproxy.com/?url={DOWNLOAD_URL}";
        let result = apply_url_replace(url, None, Some(replace));
        assert_eq!(
            result,
            "https://ghproxy.com/?url=https://github.com/user/repo/releases/download/v1.0/app.apk"
        );
    }

    #[test]
    fn test_host_only_replacement() {
        let url = "https://github.com/user/repo/releases/download/v1.0/app.apk";
        let result = apply_url_replace(url, None, Some("https://mirror.ghproxy.com"));
        assert!(result.contains("mirror.ghproxy.com"));
        assert!(result.contains("/user/repo/releases/download/v1.0/app.apk"));
        assert!(!result.contains("github.com"));
    }

    #[test]
    fn test_host_only_with_trailing_slash() {
        let url = "https://github.com/owner/repo/archive/v2.zip";
        let result = apply_url_replace(url, None, Some("https://mirror.example.com/"));
        assert!(result.contains("mirror.example.com"));
        assert!(result.contains("/owner/repo/archive/v2.zip"));
    }

    #[test]
    fn test_regex_replacement() {
        let url = "https://github.com/user/repo/releases/download/v1.0/app.apk";
        let result = apply_url_replace(url, Some("github\\.com"), Some("github.com.cnpmjs.org"));
        assert!(result.contains("github.com.cnpmjs.org"));
        assert!(!result.contains("//github.com/"));
    }

    #[test]
    fn test_invalid_regex_falls_back_to_literal() {
        let url = "https://github.com/user/repo";
        // Invalid regex pattern — should fall back to literal string replace
        let result = apply_url_replace(url, Some("github.com"), Some("gitlab.com"));
        assert!(result.contains("gitlab.com"));
    }

    // -------------------------------------------------------------------------
    // Phase 8: chained GLOBAL + hub-specific rules (as applied in get_download)
    // -------------------------------------------------------------------------

    #[test]
    fn test_global_then_hub_specific_chain() {
        // Simulate: GLOBAL rule replaces github.com host, then hub rule wraps via template.
        let url = "https://github.com/owner/repo/releases/download/v1.0/app.apk";

        // Step 1 — GLOBAL rule: replace host with mirror
        let after_global = apply_url_replace(url, None, Some("https://mirror.example.com"));
        assert!(after_global.contains("mirror.example.com"));
        assert!(!after_global.contains("github.com"));

        // Step 2 — hub-specific rule: wrap with proxy template
        let after_hub = apply_url_replace(
            &after_global,
            None,
            Some("https://proxy.example.com/?url={DOWNLOAD_URL}"),
        );
        assert!(after_hub.starts_with("https://proxy.example.com/?url="));
        assert!(after_hub.contains("mirror.example.com"));
    }

    #[test]
    fn test_no_global_rule_hub_rule_applies() {
        let url = "https://github.com/owner/repo/archive/v2.zip";

        // GLOBAL has no replace rule — url unchanged
        let after_global = apply_url_replace(url, None, None);
        assert_eq!(after_global, url);

        // hub-specific regex rule
        let after_hub =
            apply_url_replace(&after_global, Some("github\\.com"), Some("gh.example.com"));
        assert!(after_hub.contains("gh.example.com"));
    }

    #[test]
    fn test_both_rules_none_url_unchanged() {
        let url = "https://github.com/owner/repo/releases/v1.apk";
        let after_global = apply_url_replace(url, None, None);
        let after_hub = apply_url_replace(&after_global, None, None);
        assert_eq!(after_hub, url);
    }
}
