//! Gemini app source: the 5-hour and weekly limits shown at
//! gemini.google.com/usage, read with the signed-in browser account.
//!
//! These quotas belong to the Google account in the browser and are separate
//! from Gemini CLI quotas. Nothing from the CLI credential (email, plan) is
//! copied into this result.

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use serde_json::{Map, Value};

use crate::browser::cookies::CookieRequestScope;
use crate::browser::session_scan::{BrowserCookieStore, SystemCookieStore};
use crate::core::{FetchContext, ProviderError, RateWindow, UsageSnapshot};
use crate::providers::{
    BoundedBodyError, BrowserCookieSessions, browser_cookie_request_sessions_from,
    normalize_cookie_header, read_bounded_response,
};

/// Google session cookies live on `.google.com`; only the ones a browser
/// sends to gemini.google.com are used.
pub(crate) const COOKIE_SCOPE: CookieRequestScope = CookieRequestScope {
    scan_domain: "google.com",
    host: "gemini.google.com",
};

const BASE_URL: &str = "https://gemini.google.com";
const QUERY_PATH: &str = "/_/BardChatUi/graphql/schemas/GEMINI_WEB_GRAPHQL/executeQuery";
/// Persisted `UsagePageQuery` signature from the public UsagePageSurface bundle.
const QUERY_SIGNATURE: &str = "2/dWbCvkVB0X16r9NPnxifeOAAqdCqMf0xeWvau9yPy7g=";
const QUERY_VARIABLES: &str = r#"{"g1Request":{"entrypoints":["GEMINI_USAGE_SETTINGS_UPGRADE_BANNER"]},"workspaceRequest":{"recommendationsForSurfaceRequests":[]}}"#;
pub(crate) const USAGE_PAGE_URL: &str = "https://gemini.google.com/usage";
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

const FIVE_HOUR_POOL: &str = "GXU_FIVE_HOURLY";
const WEEKLY_POOL: &str = "GXU_WEEKLY";
const FIVE_HOUR_MINUTES: u32 = 300;
const WEEKLY_MINUTES: u32 = 10_080;

pub(crate) const FIVE_HOUR_LABEL: &str = "5-hour";
pub(crate) const WEEKLY_LABEL: &str = "Weekly";

pub(crate) const MISSING_SESSION: &str = "Sign in at gemini.google.com/usage in a supported browser, import its cookies in Settings, or paste its Cookie header.";
pub(crate) const COOKIES_UNAVAILABLE: &str = "Gemini app needs a browser session. Set the cookie source to Automatic, import cookies from a browser, or paste a Cookie header from gemini.google.com.";
pub(crate) const SESSION_EXPIRED: &str = "Sign in again at gemini.google.com/usage.";
pub(crate) const QUERY_CHANGED: &str =
    "Gemini app usage response changed. Update CodexBar or report this.";
pub(crate) const NO_WINDOWS: &str = "This Google account reports no Gemini app usage windows.";
const RATE_LIMITED: &str = "Gemini app usage is rate limited.";
const UNAVAILABLE: &str = "Gemini app usage is temporarily unavailable.";

/// What one browser session produced.
#[derive(Debug)]
pub(crate) enum SessionOutcome {
    Usage(Box<UsageSnapshot>),
    /// Signed out or rejected; the next browser session may be tried.
    Rejected,
}

pub struct GeminiWeb {
    client: Client,
    base_url: String,
}

impl GeminiWeb {
    pub fn new() -> Self {
        Self::with_base_url(
            BASE_URL,
            crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        )
    }

    pub(crate) fn with_base_url(base_url: impl Into<String>, client: Client) -> Self {
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    /// A pasted header is the only session tried. Otherwise each browser's
    /// session is tried in turn, moving on only after a rejected session.
    pub async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        if let Some(raw) = ctx.manual_cookie_header.as_deref() {
            let cookie = normalize_cookie_header(raw).ok_or_else(missing_session)?;
            return match self.fetch_session(&cookie).await? {
                SessionOutcome::Usage(usage) => Ok(*usage),
                SessionOutcome::Rejected => Err(session_expired()),
            };
        }
        if ctx.manual_cookie_missing {
            return Err(ProviderError::BrowserSignInRequired {
                message: COOKIES_UNAVAILABLE.into(),
                sign_in_url: USAGE_PAGE_URL.into(),
            });
        }
        let sessions = browser_cookie_request_sessions_from(SystemCookieStore, ctx, COOKIE_SCOPE);
        self.fetch_sessions(sessions).await
    }

    /// The first validated account ends the search, even when it reports no
    /// usage window; only a rejected session moves on.
    pub(crate) async fn fetch_sessions<S: BrowserCookieStore>(
        &self,
        mut sessions: BrowserCookieSessions<S>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let mut rejected = false;
        for header in sessions.by_ref() {
            let Some(cookie) = normalize_cookie_header(&header) else {
                continue;
            };
            match self.fetch_session(&cookie).await? {
                SessionOutcome::Usage(usage) => return Ok(*usage),
                SessionOutcome::Rejected => rejected = true,
            }
        }
        if rejected {
            return Err(session_expired());
        }
        Err(match sessions.exhaustion_error() {
            ProviderError::NoCookies => missing_session(),
            error => error,
        })
    }

    async fn fetch_session(&self, cookie: &str) -> Result<SessionOutcome, ProviderError> {
        let response = self
            .client
            .get(format!("{}{QUERY_PATH}", self.base_url))
            .query(&[
                ("query_signature", QUERY_SIGNATURE),
                ("variables", QUERY_VARIABLES),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::REFERER, USAGE_PAGE_URL)
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Ok(SessionOutcome::Rejected);
            }
            StatusCode::TOO_MANY_REQUESTS => return Err(ProviderError::Other(RATE_LIMITED.into())),
            status if status.is_server_error() => {
                return Err(ProviderError::Other(UNAVAILABLE.into()));
            }
            status => {
                return Err(ProviderError::Other(format!(
                    "Gemini app returned HTTP {}.",
                    status.as_u16()
                )));
            }
        }
        let body = read_bounded_response(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Read(error) => ProviderError::Network(error),
                BoundedBodyError::TooLarge => ProviderError::Parse(QUERY_CHANGED.into()),
            })?;
        parse_usage(&body)
    }
}

impl Default for GeminiWeb {
    fn default() -> Self {
        Self::new()
    }
}

fn missing_session() -> ProviderError {
    ProviderError::BrowserSignInRequired {
        message: MISSING_SESSION.into(),
        sign_in_url: USAGE_PAGE_URL.into(),
    }
}

fn session_expired() -> ProviderError {
    ProviderError::BrowserSignInRequired {
        message: SESSION_EXPIRED.into(),
        sign_in_url: USAGE_PAGE_URL.into(),
    }
}

fn query_changed() -> ProviderError {
    ProviderError::Parse(QUERY_CHANGED.into())
}

/// Parse a 200 response. The response text is never echoed into errors.
pub(crate) fn parse_usage(body: &[u8]) -> Result<SessionOutcome, ProviderError> {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    // A sign-in page instead of JSON means this browser is signed out.
    if text.starts_with('<') {
        return Ok(SessionOutcome::Rejected);
    }
    let json = text.strip_prefix(")]}'").unwrap_or(text).trim_start();
    let results: Value = serde_json::from_str(json).map_err(|_| query_changed())?;
    let results = results.as_array().ok_or_else(query_changed)?;
    let data: Vec<&Map<String, Value>> = results
        .iter()
        .filter_map(|result| result.get("data")?.as_object())
        .collect();
    let result = data
        .iter()
        .find(|data| data.contains_key("viewer"))
        .ok_or_else(query_changed)?;
    let viewer = &result["viewer"];
    if viewer.is_null() {
        return Ok(SessionOutcome::Rejected);
    }
    let usage = viewer
        .as_object()
        .and_then(|viewer| viewer.get("rpcs")?.get("getUsageInfo")?.as_object())
        .ok_or_else(query_changed)?;
    let pools = usage
        .get("pools")
        .and_then(Value::as_array)
        .ok_or_else(query_changed)?;

    let five_hour = pool_window(pools, FIVE_HOUR_POOL, FIVE_HOUR_MINUTES);
    let weekly = pool_window(pools, WEEKLY_POOL, WEEKLY_MINUTES);
    if five_hour.is_none() && weekly.is_none() {
        return Err(ProviderError::Other(NO_WINDOWS.into()));
    }

    // A missing 5-hour value stays unavailable rather than becoming 0%.
    let mut snapshot = UsageSnapshot::new(five_hour.unwrap_or_else(RateWindow::no_active_session))
        .with_primary_label(FIVE_HOUR_LABEL);
    if let Some(weekly) = weekly {
        snapshot = snapshot
            .with_secondary(weekly)
            .with_secondary_label(WEEKLY_LABEL);
    }
    if usage.get("subscriptionTier").and_then(Value::as_str) == Some("TIER1") {
        snapshot = snapshot.with_login_method("Pro");
    }
    Ok(SessionOutcome::Usage(Box::new(snapshot)))
}

fn pool_window(pools: &[Value], pool_type: &str, minutes: u32) -> Option<RateWindow> {
    let pool = pools
        .iter()
        .filter_map(Value::as_object)
        .find(|pool| pool.get("poolType").and_then(Value::as_str) == Some(pool_type))?;
    let fraction = pool.get("percentageUsed")?.as_f64()?;
    if !fraction.is_finite() || fraction < 0.0 {
        return None;
    }
    let resets_at = pool
        .get("gxu")
        .and_then(|gxu| gxu.get("windowRefreshesAt"))
        .and_then(Value::as_str)
        .and_then(parse_reset);
    Some(RateWindow::with_details(
        (fraction * 100.0).min(100.0),
        Some(minutes),
        resets_at,
        None,
    ))
}

fn parse_reset(raw: &str) -> Option<DateTime<Utc>> {
    let bytes = raw.as_bytes();
    let dated = bytes.len() > 10
        && bytes[..10]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
        && bytes[10] == b'T';
    if !dated {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

#[cfg(test)]
mod tests;
