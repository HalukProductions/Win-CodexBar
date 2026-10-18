//! Future AGI billing-period usage against free allowances.
//!
//! Ported from upstream CodexBar v0.77.0 (`Resources/Plugins/futureagi.ts`,
//! `Providers/FutureAGI/FutureAGIProviderDescriptor.swift`). Reads
//! `GET https://api.futureagi.com/usage/v2/usage-overview/` with the API key
//! as `X-Api-Key` and the companion secret as `X-Secret-Key`. Both are
//! required. Response bodies are never echoed in errors and neither secret is
//! logged.

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use regex_lite::Regex;
use reqwest::{Client, StatusCode, header::ACCEPT};
use serde_json::{Map, Value};

use crate::core::{
    FetchContext, Provider, ProviderDisplayDetail, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::{BoundedBodyError, format, read_bounded_response};

const USAGE_URL: &str = "https://api.futureagi.com/usage/v2/usage-overview/";
const CREDENTIAL_TARGET: &str = "codexbar-futureagi";
pub const API_KEY_ENV: &str = "FI_API_KEY";
pub const SECRET_KEY_ENV: &str = "FI_SECRET_KEY";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const USAGE_SECTION: &str = "Used / free allowance";
const BILLING_SECTION: &str = "Billing period";
const PRIMARY_KEY: &str = "ai_credits";
const SECONDARY_KEY: &str = "gateway_requests";
const MAX_DIMENSIONS: usize = 16;
/// Upstream `ProviderDetailSection.maximumStringLength`.
const MAX_LABEL_CHARS: usize = 120;
/// JavaScript `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

const MISSING_CREDENTIALS: &str = "Future AGI requires both an API key and a secret key. Enter API_KEY:SECRET_KEY in Settings, or set FI_API_KEY and FI_SECRET_KEY.";
const UNRECOGNIZED: &str = "Future AGI returned an unrecognized usage overview.";

static BILLING_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))?$")
        .expect("static billing date regex")
});

/// The key pair sent on every request.
#[derive(Debug, PartialEq, Eq)]
struct Credentials {
    api_key: String,
    secret_key: String,
}

pub struct FutureAgiProvider {
    client: Client,
    usage_url: String,
}

impl FutureAgiProvider {
    pub fn new() -> Self {
        Self::with_usage_url(USAGE_URL)
    }

    fn with_usage_url(usage_url: &str) -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
            usage_url: usage_url.to_owned(),
        }
    }

    async fn fetch_usage_impl(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let credentials = resolve_credentials(ctx.api_key.as_deref())?;
        let response = self
            .client
            .get(&self.usage_url)
            .header("X-Api-Key", &credentials.api_key)
            .header("X-Secret-Key", &credentials.secret_key)
            .header(ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(ProviderError::Other(format!(
                "Future AGI returned HTTP {}.",
                status.as_u16()
            )));
        }
        let body = read_bounded_response(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Read(error) => ProviderError::Network(error),
                BoundedBodyError::TooLarge => unrecognized(),
            })?;
        parse_overview(&body)
    }
}

impl Default for FutureAgiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for FutureAgiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::FutureAgi
    }

    fn metadata(&self) -> &ProviderMetadata {
        &FUTUREAGI_METADATA
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => self.fetch_usage_impl(ctx).await,
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn resolve_credentials(explicit: Option<&str>) -> Result<Credentials, ProviderError> {
    let stored = explicit.and_then(cleaned_key).or_else(|| {
        keyring::Entry::new(CREDENTIAL_TARGET, "api_key")
            .ok()
            .and_then(|entry| entry.get_password().ok())
            .as_deref()
            .and_then(cleaned_key)
    });
    credentials_from(stored.as_deref(), |name| std::env::var(name).ok())
        .ok_or_else(|| ProviderError::NotInstalled(MISSING_CREDENTIALS.into()))
}

/// The Settings field holds `API_KEY:SECRET_KEY`, or only the API key with the
/// secret in `FI_SECRET_KEY`. Each half from Settings overrides its
/// environment variable, as upstream's config projection does.
fn credentials_from(
    stored: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<Credentials> {
    let (stored_api, stored_secret) = match stored {
        Some(value) => match value.split_once(':') {
            Some((api, secret)) => (cleaned_key(api), cleaned_key(secret)),
            None => (cleaned_key(value), None),
        },
        None => (None, None),
    };
    let api_key = stored_api.or_else(|| env(API_KEY_ENV).as_deref().and_then(cleaned_key))?;
    let secret_key =
        stored_secret.or_else(|| env(SECRET_KEY_ENV).as_deref().and_then(cleaned_key))?;
    Some(Credentials {
        api_key,
        secret_key,
    })
}

/// Upstream's `SettingsValue.cleaned`: trimmed, one pair of wrapping quotes
/// removed, trimmed again, and blank values treated as absent.
fn cleaned_key(raw: &str) -> Option<String> {
    let value = raw.trim();
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    let value = if quoted {
        value.get(1..value.len() - 1).unwrap_or_default()
    } else {
        value
    };
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn unrecognized() -> ProviderError {
    ProviderError::Parse(UNRECOGNIZED.into())
}

/// A finite, non-negative JSON number no larger than `MAX_SAFE_INTEGER`.
/// Numeric strings are rejected so malformed usage never becomes zero.
fn amount(value: Option<&Value>) -> Result<f64, ProviderError> {
    value
        .filter(|value| value.is_number())
        .and_then(Value::as_f64)
        .filter(|amount| amount.is_finite() && (0.0..=MAX_SAFE_INTEGER).contains(amount))
        .ok_or_else(unrecognized)
}

/// A `YYYY-MM-DD` date (UTC midnight, as JavaScript parses it) or an ISO 8601
/// timestamp with an explicit offset.
fn billing_date(value: Option<&Value>) -> Result<DateTime<Utc>, ProviderError> {
    let text = value
        .and_then(Value::as_str)
        .filter(|text| BILLING_DATE.is_match(text))
        .ok_or_else(unrecognized)?;
    if text.len() == 10 {
        return NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|midnight| midnight.and_utc())
            .ok_or_else(unrecognized);
    }
    DateTime::parse_from_rfc3339(text)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|_| unrecognized())
}

/// Upstream `ctx.isDetailLabel`: a string that is non-empty after trimming,
/// at most 120 characters, and free of control characters.
fn detail_label(value: Option<&Value>) -> Result<String, ProviderError> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| {
            !label.is_empty()
                && label.chars().count() <= MAX_LABEL_CHARS
                && !label.chars().any(char::is_control)
        })
        .map(str::to_string)
        .ok_or_else(unrecognized)
}

/// Upstream `ctx.pct`: clamped to 0...100.
fn percent(used: f64, allowance: f64) -> f64 {
    (used / allowance * 100.0).clamp(0.0, 100.0)
}

fn parse_overview(body: &[u8]) -> Result<ProviderFetchResult, ProviderError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| unrecognized())?;
    if root.get("status") != Some(&Value::Bool(true)) {
        return Err(unrecognized());
    }
    let data: &Map<String, Value> = root
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(unrecognized)?;
    let dimensions = data
        .get("dimensions")
        .and_then(Value::as_array)
        .ok_or_else(unrecognized)?;

    let start = billing_date(data.get("billing_period_start"))?;
    let end = billing_date(data.get("billing_period_end"))?;
    if end <= start || dimensions.is_empty() || dimensions.len() > MAX_DIMENSIONS {
        return Err(unrecognized());
    }
    // Upstream rounds the JavaScript millisecond span to whole minutes.
    let period_ms = (end - start).num_milliseconds();
    let window_minutes = u32::try_from((period_ms + 30_000) / 60_000).ok();

    let mut keys = HashSet::new();
    let mut primary = None;
    let mut secondary = None;
    let mut rows = Vec::new();
    for (index, meter) in dimensions.iter().enumerate() {
        let meter = meter.as_object().ok_or_else(unrecognized)?;
        let key = meter
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(unrecognized)?;
        if !keys.insert(key) {
            return Err(unrecognized());
        }
        let name = detail_label(meter.get("display_name"))?;
        let unit = detail_label(meter.get("display_unit"))?;
        let used = amount(meter.get("current_usage"))?;
        let allowance = amount(meter.get("free_allowance"))?;
        if allowance > 0.0 {
            let window =
                RateWindow::with_details(percent(used, allowance), window_minutes, Some(end), None);
            match key {
                PRIMARY_KEY => primary = Some(window),
                SECONDARY_KEY => secondary = Some(window),
                _ => {}
            }
        }
        rows.extend(
            ProviderDisplayDetail::new(
                format!("dimension-{index}"),
                name,
                format!(
                    "{} / {} {unit}",
                    format::number(used, 3),
                    format::number(allowance, 3)
                ),
            )
            .and_then(|row| row.with_section_title(USAGE_SECTION)),
        );
    }
    let estimated_cost = amount(data.get("total_estimated_cost"))?;
    rows.extend(
        ProviderDisplayDetail::new(
            "estimated-usage-cost",
            "Estimated usage cost",
            format::usd(estimated_cost),
        )
        .and_then(|row| row.with_section_title(BILLING_SECTION)),
    );

    // Upstream has no primary window without a positive AI credits allowance;
    // the placeholder keeps the card to the detail rows.
    let primary =
        primary.unwrap_or_else(|| RateWindow::informational(USAGE_SECTION).hidden_on_card());
    let mut usage = UsageSnapshot::new(primary);
    if let Some(window) = secondary {
        usage = usage.with_secondary(window);
    }
    if let Some(plan) = data
        .get("plan_display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
    {
        usage = usage.with_login_method(plan);
    }
    Ok(ProviderFetchResult::new(usage, "api").with_display_details(rows))
}

const FUTUREAGI_METADATA: ProviderMetadata = ProviderMetadata {
    id: ProviderId::FutureAgi,
    display_name: "Future AGI",
    session_label: "AI credits",
    weekly_label: "Gateway requests",
    supports_opus: false,
    supports_credits: false,
    default_enabled: false,
    is_primary: false,
    dashboard_url: Some("https://app.futureagi.com/dashboard/settings/usage-summary"),
    status_page_url: None,
    tertiary_label_key: None,
};
