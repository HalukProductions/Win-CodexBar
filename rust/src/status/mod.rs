//! Provider service status (statuspage.io pages and the Google Workspace
//! incidents feed), ported from upstream `ProviderStatus.swift` and
//! `ProviderStatusFetcher.swift`.

mod parse;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::core::ProviderId;

pub use parse::{ParseError, parse_google_workspace_status, parse_statuspage_status};

/// Upstream `ProviderStatusIndicator`. `Operational` is the statuspage.io
/// raw value `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusIndicator {
    Operational,
    Minor,
    Major,
    Critical,
    Maintenance,
    Unknown,
}

impl StatusIndicator {
    /// Unrecognised raw values map to `Unknown`, as upstream does.
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "none" => Self::Operational,
            "minor" => Self::Minor,
            "major" => Self::Major,
            "critical" => Self::Critical,
            "maintenance" => Self::Maintenance,
            _ => Self::Unknown,
        }
    }

    /// The upstream raw value, used on the bridge and in hook payloads.
    pub fn as_raw(self) -> &'static str {
        match self {
            Self::Operational => "none",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
            Self::Maintenance => "maintenance",
            Self::Unknown => "unknown",
        }
    }

    pub fn has_issue(self) -> bool {
        self != Self::Operational
    }

    /// Upstream `indicatorRank`: worst incident wins.
    pub fn rank(self) -> u8 {
        match self {
            Self::Operational => 0,
            Self::Maintenance | Self::Unknown => 1,
            Self::Minor => 2,
            Self::Major => 3,
            Self::Critical => 4,
        }
    }

    /// The CLI JSON `status.level` vocabulary, unchanged from before the
    /// poller so scripts keep parsing it: raw `major` and `critical` both
    /// read `major`, `maintenance` reads `unknown`. Raw `minor` used to fall
    /// through to `unknown` (a bug); it now reads `degraded`, the old
    /// vocabulary's word for a partial outage.
    pub fn cli_level(self) -> &'static str {
        match self {
            Self::Operational => "operational",
            Self::Minor => "degraded",
            Self::Major | Self::Critical => "major",
            Self::Maintenance | Self::Unknown => "unknown",
        }
    }

    /// Upstream CLI `cliLabel` (English only).
    pub fn cli_label(self) -> &'static str {
        match self {
            Self::Operational => "Operational",
            Self::Minor => "Partial outage",
            Self::Major => "Major outage",
            Self::Critical => "Critical issue",
            Self::Maintenance => "Maintenance",
            Self::Unknown => "Status unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub indicator: StatusIndicator,
    pub description: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl ProviderStatus {
    pub fn unknown() -> Self {
        Self {
            indicator: StatusIndicator::Unknown,
            description: None,
            updated_at: None,
        }
    }

    /// The description when the feed gave one, else the CLI label.
    pub fn summary(&self) -> &str {
        self.description
            .as_deref()
            .filter(|text| !text.is_empty())
            .unwrap_or(self.indicator.cli_label())
    }
}

/// Where a provider publishes its service status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusSource {
    /// A statuspage.io-compatible page; `api/v2/status.json` is appended.
    Statuspage(&'static str),
    /// A Google Workspace product id in the shared incidents feed.
    GoogleWorkspace(&'static str),
}

const GOOGLE_WORKSPACE_INCIDENTS_URL: &str =
    "https://www.google.com/appsstatus/dashboard/incidents.json";
const GEMINI_WORKSPACE_PRODUCT_ID: &str = "npdyhgECDJ6tB66MxXyo";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The providers upstream polls (`statusPageURL` / `statusWorkspaceProductID`
/// in each provider descriptor). Every other provider has no status source.
pub fn status_source(provider: ProviderId) -> Option<StatusSource> {
    use StatusSource::{GoogleWorkspace, Statuspage};
    Some(match provider {
        ProviderId::Claude | ProviderId::ClaudePlatform => Statuspage("https://status.claude.com"),
        ProviderId::Codex | ProviderId::OpenAIApi => Statuspage("https://status.openai.com"),
        ProviderId::Copilot => Statuspage("https://www.githubstatus.com"),
        ProviderId::Cursor => Statuspage("https://status.cursor.com"),
        ProviderId::Factory => Statuspage("https://status.factory.ai"),
        ProviderId::Augment => Statuspage("https://status.augmentcode.com"),
        ProviderId::HuggingFace => Statuspage("https://status.huggingface.co"),
        ProviderId::ZoomMate => Statuspage("https://www.zoomstatus.com"),
        ProviderId::Gemini | ProviderId::Antigravity => {
            GoogleWorkspace(GEMINI_WORKSPACE_PRODUCT_ID)
        }
        _ => return None,
    })
}

/// Fetch one source. Errors carry no URL or response body, so they are safe
/// to log.
pub async fn fetch_status(source: StatusSource) -> Result<ProviderStatus, String> {
    let client = crate::core::apply_app_proxy(reqwest::Client::builder())
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|_| "client build failed".to_string())?;
    let url = match source {
        StatusSource::Statuspage(base) => format!("{base}/api/v2/status.json"),
        StatusSource::GoogleWorkspace(_) => GOOGLE_WORKSPACE_INCIDENTS_URL.to_string(),
    };
    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "timeout".to_string()
            } else {
                "request failed".to_string()
            }
        })?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let body = response
        .bytes()
        .await
        .map_err(|_| "body read failed".to_string())?;
    parse_source(source, &body)
}

/// A decode error can quote feed text, so it is replaced by a fixed string.
fn parse_source(source: StatusSource, body: &[u8]) -> Result<ProviderStatus, String> {
    match source {
        StatusSource::Statuspage(_) => parse_statuspage_status(body),
        StatusSource::GoogleWorkspace(product) => parse_google_workspace_status(body, product),
    }
    .map_err(|_| "invalid status feed".to_string())
}

/// CLI entry point. A failed refresh never replaces the last successful
/// result (upstream 0.50.0 #2925), so long-running consumers (hooks watch)
/// keep showing the last known status.
pub async fn fetch_provider_status(provider: ProviderId) -> Option<ProviderStatus> {
    let source = status_source(provider)?;
    match fetch_status(source).await {
        Ok(status) => {
            if let Ok(mut cache) = last_success_cache().lock() {
                cache.insert(provider, status.clone());
            }
            Some(status)
        }
        Err(_) => last_success_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&provider).cloned()),
    }
}

fn last_success_cache() -> &'static std::sync::Mutex<HashMap<ProviderId, ProviderStatus>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<ProviderId, ProviderStatus>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}
