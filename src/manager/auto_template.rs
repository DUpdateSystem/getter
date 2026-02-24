use std::collections::HashMap;

/// URL template regex: matches `%placeholder` tokens.
/// Mirrors Kotlin's `URL_ARG_REGEX = "(%.*?)\\w*"`.
const URL_ARG_PATTERN: &str = r"(%[^%/?\s]+)";

/// Given a URL and a list of templates, return the first template that fully
/// matches the URL, with all placeholder values extracted.
///
/// Template format: `https://github.com/%owner/%repo/releases`
/// Placeholders are `%key` tokens. The returned map has keys without the `%`.
///
/// Returns `None` if no template matches fully.
///
/// Mirrors Kotlin's `AutoTemplate.urlToAppId()`.
pub fn url_to_app_id(url: &str, templates: &[String]) -> Option<HashMap<String, String>> {
    if url.is_empty() || templates.is_empty() {
        return None;
    }
    for template in templates {
        if let Some(args) = match_template(url, template) {
            return Some(args);
        }
    }
    None
}

/// Attempt to match `url` against a single `template`.
///
/// The algorithm splits the template into alternating [literal, placeholder]
/// segments, then uses literals to cut the URL apart and extract placeholder
/// values.  Mirrors Kotlin's `AutoTemplate.matchArgs()` and `checkFull()`.
fn match_template(url: &str, template: &str) -> Option<HashMap<String, String>> {
    let re = regex::Regex::new(URL_ARG_PATTERN).ok()?;

    // Build ordered list of segments: either a literal string or a %placeholder
    let mut segments: Vec<Segment> = Vec::new();
    let mut last = 0;
    for m in re.find_iter(template) {
        if m.start() > last {
            segments.push(Segment::Literal(template[last..m.start()].to_string()));
        }
        segments.push(Segment::Placeholder(m.as_str().to_string()));
        last = m.end();
    }
    if last < template.len() {
        segments.push(Segment::Literal(template[last..].to_string()));
    }

    // Collect expected placeholder keys (in order)
    let expected_keys: Vec<String> = segments
        .iter()
        .filter_map(|s| {
            if let Segment::Placeholder(p) = s {
                Some(p.trim_start_matches('%').to_string())
            } else {
                None
            }
        })
        .collect();

    if expected_keys.is_empty() {
        return None;
    }

    // Walk through segments: use literals to split the URL and assign values to
    // adjacent placeholders.
    let mut args: HashMap<String, String> = HashMap::new();
    let mut remaining = url.to_string();

    for (i, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Literal(lit) => {
                if lit.is_empty() {
                    continue;
                }
                // Find the literal in `remaining`, split on it
                match remaining.split_once(lit.as_str()) {
                    Some((before, after)) => {
                        // `before` belongs to the preceding placeholder (if any)
                        if i > 0 {
                            if let Segment::Placeholder(key) = &segments[i - 1] {
                                let k = key.trim_start_matches('%').to_string();
                                if !before.is_empty() {
                                    args.insert(k, before.to_string());
                                }
                            }
                        }
                        remaining = after.to_string();
                    }
                    None => return None, // literal not found → no match
                }
            }
            Segment::Placeholder(_) => {
                // Value will be filled in when the next literal is processed,
                // or captured from trailing remaining string at end.
            }
        }
    }

    // If the last segment is a placeholder, the rest of `remaining` is its value
    if let Some(Segment::Placeholder(key)) = segments.last() {
        let k = key.trim_start_matches('%').to_string();
        // Strip trailing slash or query string for cleanliness
        let val = remaining.trim_end_matches('/').to_string();
        if !val.is_empty() {
            args.insert(k, val);
        }
    }

    // Verify all expected keys were matched
    for key in &expected_keys {
        if !args.contains_key(key) {
            return None;
        }
    }

    Some(args)
}

#[derive(Debug)]
enum Segment {
    Literal(String),
    Placeholder(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_github_url() {
        let url = "https://github.com/DUpdateSystem/UpgradeAll";
        let result = url_to_app_id(url, &templates(&["https://github.com/%owner/%repo"]));
        let map = result.unwrap();
        assert_eq!(map["owner"], "DUpdateSystem");
        assert_eq!(map["repo"], "UpgradeAll");
    }

    #[test]
    fn test_github_url_with_trailing_slash() {
        let url = "https://github.com/foo/bar/";
        let result = url_to_app_id(url, &templates(&["https://github.com/%owner/%repo/"]));
        let map = result.unwrap();
        assert_eq!(map["owner"], "foo");
        assert_eq!(map["repo"], "bar");
    }

    #[test]
    fn test_gitlab_url() {
        let url = "https://gitlab.com/AuroraOSS/AuroraStore";
        let result = url_to_app_id(url, &templates(&["https://gitlab.com/%owner/%repo"]));
        let map = result.unwrap();
        assert_eq!(map["owner"], "AuroraOSS");
        assert_eq!(map["repo"], "AuroraStore");
    }

    #[test]
    fn test_no_match_returns_none() {
        let url = "https://example.com/something/else";
        let result = url_to_app_id(url, &templates(&["https://github.com/%owner/%repo"]));
        assert!(result.is_none());
    }

    #[test]
    fn test_first_matching_template_wins() {
        let url = "https://github.com/user/proj";
        let result = url_to_app_id(
            url,
            &templates(&[
                "https://gitlab.com/%owner/%repo",
                "https://github.com/%owner/%repo",
            ]),
        );
        let map = result.unwrap();
        assert_eq!(map["owner"], "user");
        assert_eq!(map["repo"], "proj");
    }

    #[test]
    fn test_empty_url_returns_none() {
        let result = url_to_app_id("", &templates(&["https://github.com/%owner/%repo"]));
        assert!(result.is_none());
    }

    #[test]
    fn test_empty_templates_returns_none() {
        let result = url_to_app_id("https://github.com/a/b", &[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_single_placeholder() {
        let url = "https://f-droid.org/packages/com.example.app/";
        let result = url_to_app_id(
            url,
            &templates(&["https://f-droid.org/packages/%package_name/"]),
        );
        let map = result.unwrap();
        assert_eq!(map["package_name"], "com.example.app");
    }
}
