//! GitHub endpoint roots for the Copilot device login and usage API.
//!
//! Production always uses `github.com` for the device flow and
//! `api.github.com` for identity and usage. `CODEXBAR_GITHUB_BASE_URL` points
//! both roots at one origin so a proof run or test can use a fake server. Only
//! `https://` and loopback `http://` origins are accepted, so a token is never
//! sent to a remote plain-HTTP host. Anything else is ignored with a warning.

use reqwest::Url;

/// Environment override for both GitHub roots (proof runs and fake servers).
pub(crate) const GITHUB_BASE_URL_ENV: &str = "CODEXBAR_GITHUB_BASE_URL";

const GITHUB_WEB_ROOT: &str = "https://github.com";
const GITHUB_API_ROOT: &str = "https://api.github.com";

/// The GitHub roots a Copilot login or usage request talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubEndpoints {
    /// Root for `/login/device/code` and `/login/oauth/access_token`.
    pub(crate) web_root: String,
    /// Root for `/user` and `/copilot_internal/user`.
    pub(crate) api_root: String,
    /// True when an override replaced the production roots.
    pub(crate) overridden: bool,
}

impl GitHubEndpoints {
    /// Production endpoints on github.com and api.github.com.
    pub(crate) fn github() -> Self {
        Self {
            web_root: GITHUB_WEB_ROOT.to_string(),
            api_root: GITHUB_API_ROOT.to_string(),
            overridden: false,
        }
    }

    /// Both roots on one origin, such as `http://127.0.0.1:8080`.
    pub(crate) fn with_origin(origin: &str) -> Option<Self> {
        let origin = validated_origin(origin)?;
        Some(Self {
            web_root: origin.clone(),
            api_root: origin,
            overridden: true,
        })
    }

    /// Reads `CODEXBAR_GITHUB_BASE_URL`; falls back to GitHub when it is unset
    /// or rejected.
    pub(crate) fn from_env() -> Self {
        Self::from_override(std::env::var(GITHUB_BASE_URL_ENV).ok().as_deref())
    }

    fn from_override(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Self::github();
        };
        match Self::with_origin(raw) {
            Some(endpoints) => endpoints,
            None => {
                tracing::warn!(
                    "Ignoring {GITHUB_BASE_URL_ENV}: use an https:// URL or a loopback http:// URL with no path, query, or credentials"
                );
                Self::github()
            }
        }
    }
}

/// Accepts `https://host[:port]` or loopback `http://` (127.0.0.1, ::1,
/// localhost) with an empty or `/` path and no query, fragment, or userinfo.
/// Returns the origin without a trailing slash.
fn validated_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.path().is_empty() || url.path() == "/")
    {
        return None;
    }
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let allowed = match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => loopback,
        _ => false,
    };
    if !allowed {
        return None;
    }
    let host = url.host_str()?;
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Some(format!("{}://{host}{port}", url.scheme()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_endpoints_are_github() {
        let endpoints = GitHubEndpoints::from_override(None);
        assert_eq!(endpoints, GitHubEndpoints::github());
        assert!(!endpoints.overridden);
    }

    #[test]
    fn blank_override_uses_github() {
        assert_eq!(
            GitHubEndpoints::from_override(Some("   ")),
            GitHubEndpoints::github()
        );
    }

    #[test]
    fn accepts_https_and_loopback_http_origins() {
        assert_eq!(
            validated_origin("https://github.example.com/").as_deref(),
            Some("https://github.example.com")
        );
        assert_eq!(
            validated_origin("http://127.0.0.1:4010").as_deref(),
            Some("http://127.0.0.1:4010")
        );
        assert_eq!(
            validated_origin("http://localhost:9").as_deref(),
            Some("http://localhost:9")
        );
    }

    #[test]
    fn rejects_remote_plain_http_and_unsafe_forms() {
        for raw in [
            "http://github.example.com",
            "ftp://127.0.0.1",
            "https://user:pass@github.example.com",
            "https://github.example.com/api",
            "https://github.example.com?x=1",
            "https://github.example.com#frag",
            "not a url",
        ] {
            assert_eq!(validated_origin(raw), None, "{raw}");
        }
    }

    #[test]
    fn rejected_override_falls_back_to_github() {
        let endpoints = GitHubEndpoints::from_override(Some("http://attacker.example.com"));
        assert_eq!(endpoints, GitHubEndpoints::github());
    }

    #[test]
    fn accepted_override_replaces_both_roots() {
        let endpoints = GitHubEndpoints::from_override(Some("http://127.0.0.1:4010"));
        assert_eq!(endpoints.web_root, "http://127.0.0.1:4010");
        assert_eq!(endpoints.api_root, "http://127.0.0.1:4010");
        assert!(endpoints.overridden);
    }
}
