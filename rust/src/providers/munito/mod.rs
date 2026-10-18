//! Munito organization spend limits.
//!
//! Ported from upstream CodexBar v0.77.0 (`Resources/Plugins/munito.ts`,
//! `Providers/Munito/MunitoProviderDescriptor.swift`). Reads the read-only
//! Admin API `GET https://api.munito.ai/admin/v1/limits` with the admin key as
//! a bearer token, plus a best-effort `GET /admin/v1/organization` for the
//! organization name, status and data region. The key needs the "Read account
//! data" permission. Response bodies are never echoed in errors and the key is
//! never logged.

#[cfg(test)]
mod tests;

use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use regex_lite::Regex;
use reqwest::{Client, StatusCode, header::ACCEPT};
use serde_json::{Map, Value};

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderDisplayDetail, ProviderError,
    ProviderFetchResult, ProviderId, ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};
use crate::providers::{BoundedBodyError, format, read_bounded_response};

const BASE_URL: &str = "https://api.munito.ai/admin/v1";
const CREDENTIAL_TARGET: &str = "codexbar-munito";
pub const API_KEY_ENV: &str = "MUNITO_API_KEY";
const API_KEY_ENV_ALIAS: &str = "MUNITO_ADMIN_KEY";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The organization read is a companion request; it must not hold the card
/// back for long when the limits request has already answered.
const ORGANIZATION_TIMEOUT: Duration = Duration::from_secs(5);
const LOGIN_METHOD: &str = "Admin API key";
const SPEND_SECTION: &str = "Spend limits";
const ORGANIZATION_SECTION: &str = "Organization";
const DAY_WINDOW_MINUTES: u32 = 24 * 60;

const MISSING_KEY: &str = "Set a Munito admin API key in Settings or MUNITO_API_KEY.";
const PERMISSION_DENIED: &str =
    "Munito rejected the key: the Admin API needs a key with Read account data permission.";
const UNRECOGNIZED: &str = "Munito returned an unrecognized spend limits response.";

static MONEY_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*-?\d+(?:\.\d+)?\s*$").expect("static money regex"));
static ISO_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$")
        .expect("static timestamp regex")
});

pub struct MunitoProvider {
    client: Client,
    base_url: String,
}

impl MunitoProvider {
    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    fn with_base_url(base_url: &str) -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    async fn fetch_usage_impl(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let key = resolve_key(ctx.api_key.as_deref())?;
        let (limits, organization) =
            tokio::join!(self.fetch_limits(&key), self.fetch_organization(&key));
        let body = limits?;
        parse_usage(&body, organization.as_ref())
    }

    async fn fetch_limits(&self, key: &str) -> Result<Vec<u8>, ProviderError> {
        let response = self
            .client
            .get(format!("{}/limits", self.base_url))
            .bearer_auth(key)
            .header(ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(status_error(status));
        }
        read_bounded_response(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Read(error) => ProviderError::Network(error),
                BoundedBodyError::TooLarge => ProviderError::Parse(UNRECOGNIZED.into()),
            })
    }

    /// Best effort: any failure leaves the organization out and keeps the
    /// spend limits authoritative.
    async fn fetch_organization(&self, key: &str) -> Option<Map<String, Value>> {
        let response = self
            .client
            .get(format!("{}/organization", self.base_url))
            .bearer_auth(key)
            .header(ACCEPT, "application/json")
            .timeout(ORGANIZATION_TIMEOUT)
            .send()
            .await
            .ok()?;
        if response.status() != StatusCode::OK {
            return None;
        }
        let body = read_bounded_response(response, MAX_RESPONSE_BYTES)
            .await
            .ok()?;
        match serde_json::from_slice::<Value>(&body).ok()? {
            Value::Object(organization) => Some(organization),
            _ => None,
        }
    }
}

impl Default for MunitoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MunitoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Munito
    }

    fn metadata(&self) -> &ProviderMetadata {
        &MUNITO_METADATA
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

fn status_error(status: StatusCode) -> ProviderError {
    let code = status.as_u16();
    match code {
        403 => ProviderError::Other(PERMISSION_DENIED.into()),
        _ => ProviderError::Other(format!("Munito returned HTTP {code}.")),
    }
}

/// Settings key first, then the keyring, then `MUNITO_API_KEY` and its
/// `MUNITO_ADMIN_KEY` alias.
fn resolve_key(explicit: Option<&str>) -> Result<String, ProviderError> {
    if let Some(key) = explicit.and_then(cleaned_key) {
        return Ok(key);
    }
    if let Ok(entry) = keyring::Entry::new(CREDENTIAL_TARGET, "api_key")
        && let Ok(stored) = entry.get_password()
        && let Some(key) = cleaned_key(&stored)
    {
        return Ok(key);
    }
    key_from_environment(|name| std::env::var(name).ok())
        .ok_or_else(|| ProviderError::NotInstalled(MISSING_KEY.into()))
}

fn key_from_environment(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    [API_KEY_ENV, API_KEY_ENV_ALIAS]
        .into_iter()
        .find_map(|name| lookup(name).as_deref().and_then(cleaned_key))
}

/// Upstream's `SettingsValue.cleaned`: trimmed, one pair of wrapping quotes
/// removed, trimmed again, and blank keys treated as absent.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodKind {
    Day,
    Month,
}

impl PeriodKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Day => "Today",
            Self::Month => "This month",
        }
    }
}

#[derive(Debug)]
struct Period {
    kind: PeriodKind,
    spent: f64,
    limit: Option<f64>,
    organization_limit: Option<f64>,
    account_limit: Option<f64>,
    stopped_until: Option<DateTime<Utc>>,
}

impl Period {
    fn positive_limit(&self) -> Option<f64> {
        self.limit.filter(|limit| *limit > 0.0)
    }

    /// A per-scope cap worth its own row: positive and different from the
    /// effective limit already shown on the period row.
    fn distinct_cap(&self, cap: Option<f64>) -> Option<f64> {
        cap.filter(|cap| *cap > 0.0 && Some(*cap) != self.limit)
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn money(value: Option<&Value>, field: &str) -> Result<f64, ProviderError> {
    let number = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) if MONEY_TEXT.is_match(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    number
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or_else(|| ProviderError::Parse(format!("Munito returned an unusable {field}.")))
}

fn optional_money(value: Option<&Value>, field: &str) -> Result<Option<f64>, ProviderError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(_) => money(value, field).map(Some),
    }
}

fn parse_period(raw: &Value) -> Result<Option<Period>, ProviderError> {
    let entry = raw.as_object().ok_or_else(unrecognized)?;
    let kind = match text(entry.get("period")).as_deref() {
        Some("day") => PeriodKind::Day,
        Some("month") => PeriodKind::Month,
        _ => return Ok(None),
    };
    let stopped_until = match text(entry.get("stopped_until")) {
        Some(stopped) => {
            if !ISO_TIMESTAMP.is_match(&stopped) {
                return Err(unrecognized());
            }
            let parsed = DateTime::parse_from_rfc3339(&stopped).map_err(|_| unrecognized())?;
            Some(parsed.with_timezone(&Utc))
        }
        None => None,
    };
    let name = kind.wire();
    Ok(Some(Period {
        kind,
        spent: money(entry.get("spent_usd"), &format!("{name} spend"))?,
        limit: optional_money(entry.get("limit_usd"), &format!("{name} limit"))?,
        organization_limit: optional_money(
            entry.get("organization_limit_usd"),
            &format!("{name} organization limit"),
        )?,
        account_limit: optional_money(
            entry.get("account_limit_usd"),
            &format!("{name} account limit"),
        )?,
        stopped_until,
    }))
}

/// Upstream `ctx.pct`: clamped to 0...100, and 100 when the limit is unusable.
fn percent(used: f64, limit: f64) -> f64 {
    if !(used.is_finite() && limit.is_finite() && limit > 0.0) {
        return 100.0;
    }
    (used / limit * 100.0).clamp(0.0, 100.0)
}

/// JavaScript `Date.prototype.toISOString`: UTC with milliseconds.
fn iso_string(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn row_id(label: &str) -> String {
    label.to_ascii_lowercase().replace(' ', "-")
}

fn spend_row(title: &str, value: String) -> Option<ProviderDisplayDetail> {
    ProviderDisplayDetail::new(row_id(title), title, value)
        .and_then(|row| row.with_section_title(SPEND_SECTION))
}

fn period_rows(period: &Period) -> Vec<ProviderDisplayDetail> {
    let label = period.kind.label();
    let spend = format::usd(period.spent);
    let mut rows = Vec::new();
    let main = match period.positive_limit() {
        Some(limit) => spend_row(label, format!("{spend} of {}", format::usd(limit)))
            .and_then(|row| row.with_progress(period.spent.min(limit), limit)),
        None => spend_row(label, spend),
    };
    rows.extend(main);
    if let Some(cap) = period.distinct_cap(period.organization_limit) {
        rows.extend(spend_row(
            &format!("{label} organization limit"),
            format::usd(cap),
        ));
    }
    if let Some(cap) = period.distinct_cap(period.account_limit) {
        rows.extend(spend_row(
            &format!("{label} account limit"),
            format::usd(cap),
        ));
    }
    if let Some(stopped_until) = period.stopped_until {
        rows.extend(spend_row(
            &format!("{label} stopped until"),
            iso_string(stopped_until),
        ));
    }
    rows
}

fn organization_rows(organization: &Map<String, Value>) -> Vec<ProviderDisplayDetail> {
    let mut rows = Vec::new();
    if let Some(status) = text(organization.get("status")) {
        let mut chars = status.chars();
        let capitalized = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect::<String>())
            .unwrap_or_default();
        rows.extend(
            ProviderDisplayDetail::new("organization-status", "Status", capitalized)
                .and_then(|row| row.with_section_title(ORGANIZATION_SECTION)),
        );
    }
    if let Some(region) = text(organization.get("data_region")) {
        rows.extend(
            ProviderDisplayDetail::new("organization-data-region", "Data region", region)
                .and_then(|row| row.with_section_title(ORGANIZATION_SECTION)),
        );
    }
    rows
}

fn parse_usage(
    body: &[u8],
    organization: Option<&Map<String, Value>>,
) -> Result<ProviderFetchResult, ProviderError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| unrecognized())?;
    let root = root.as_object().ok_or_else(unrecognized)?;
    let raw_periods = root
        .get("periods")
        .and_then(Value::as_array)
        .ok_or_else(unrecognized)?;
    let mut periods = Vec::new();
    for raw in raw_periods {
        if let Some(period) = parse_period(raw)? {
            periods.push(period);
        }
    }
    let find = |kind: PeriodKind| periods.iter().find(|period| period.kind == kind);
    let day = find(PeriodKind::Day);
    let month = find(PeriodKind::Month);

    let usage_tier = text(root.get("usage_tier"))
        .or_else(|| organization.and_then(|org| text(org.get("usage_tier"))));

    let mut rows = Vec::new();
    for period in [day, month].into_iter().flatten() {
        rows.extend(period_rows(period));
    }
    if let Some(tier) = usage_tier {
        rows.extend(spend_row("Usage tier", tier));
    }
    if let Some(organization) = organization {
        rows.extend(organization_rows(organization));
    }

    let day_window = day.and_then(|day| {
        day.positive_limit().map(|limit| {
            RateWindow::with_details(
                percent(day.spent, limit),
                Some(DAY_WINDOW_MINUTES),
                None,
                None,
            )
        })
    });
    let month_window = month.and_then(|month| {
        month
            .positive_limit()
            .map(|limit| RateWindow::new(percent(month.spent, limit)))
    });

    // Upstream has no primary window without a positive daily limit; the
    // placeholder keeps the card to the detail rows and the monthly window.
    let primary =
        day_window.unwrap_or_else(|| RateWindow::informational(SPEND_SECTION).hidden_on_card());
    let mut usage = UsageSnapshot::new(primary).with_login_method(LOGIN_METHOD);
    if let Some(window) = month_window {
        usage = usage.with_secondary(window);
    }
    if let Some(name) = organization.and_then(|org| text(org.get("name"))) {
        usage = usage.with_organization(name);
    }

    let mut result = ProviderFetchResult::new(usage, "api").with_display_details(rows);
    if let Some(period) = month.or(day) {
        let label = period.kind.label();
        result = result.with_cost(
            CostSnapshot::new(period.spent, "USD", label).with_limit(period.limit.unwrap_or(0.0)),
        );
    }
    if let Some(id) = organization.and_then(|org| text(org.get("id"))) {
        result = result.with_account_identity(id);
    }
    Ok(result)
}

const MUNITO_METADATA: ProviderMetadata = ProviderMetadata {
    id: ProviderId::Munito,
    display_name: "Munito",
    session_label: "Today",
    weekly_label: "This month",
    supports_opus: false,
    supports_credits: false,
    default_enabled: false,
    is_primary: false,
    dashboard_url: Some("https://console.munito.ai"),
    status_page_url: None,
    tertiary_label_key: None,
};
