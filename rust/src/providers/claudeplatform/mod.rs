//! Claude Platform (Claude Console) prepaid API credits.
//!
//! Ported from upstream CodexBar v0.76.0 (`Resources/Plugins/claudeplatform.ts`,
//! `Providers/ClaudePlatform/ClaudePlatformProviderDescriptor.swift`, #4510).
//! The signed-in platform.claude.com `sessionKey` cookie reads three
//! undocumented, read-only Console endpoints: the organization list, the
//! prepaid credit balance with its grants, and the optional month-to-date
//! spend. The provider is separate from the Claude subscription provider: it
//! never reads Claude's cookie cache, OAuth credentials or identity, and only
//! reports the Console organization name and a "Console session" label.
//! Response bodies are never echoed in errors and cookies are never logged.

#[cfg(test)]
mod tests;

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Datelike, NaiveDateTime, Utc};
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, COOKIE},
};
use serde_json::{Map, Value};

use crate::browser::session_scan::{BrowserCookieStore, SystemCookieStore};
use crate::core::{
    CostSnapshot, FetchContext, ManualEmptyCookiePolicy, Provider, ProviderDisplayDetail,
    ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata, ProviderStateKind,
    RateWindow, SourceMode, UsageSnapshot,
};
use crate::locale::LocaleKey;
use crate::providers::{BoundedBodyError, format, read_bounded_response};

const BASE_URL: &str = "https://platform.claude.com";
const COOKIE_DOMAIN: &str = "platform.claude.com";
const SIGN_IN_URL: &str = "https://platform.claude.com/";
const DASHBOARD_URL: &str = "https://platform.claude.com/settings/billing";
const STATUS_PAGE_URL: &str = "https://status.claude.com/";
/// The cookie upstream's cookie policy requires for an imported session.
const SESSION_COOKIE: &str = "sessionKey";
/// Upstream's fixed web timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Upstream grants with this program are the subscription API credit lane.
const SUBSCRIPTION_PROGRAM: &str = "subscription_api_credit";
const OTHER_GRANTS_SECTION: &str = "Other credit grants";
const BILLING_SECTION: &str = "Billing";
/// `Number.MAX_SAFE_INTEGER`: upstream accepts only safe-integer minor units.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

const SESSION_EXPIRED: &str = "Claude Console session expired. Sign in at platform.claude.com or paste a fresh Cookie header.";
const MISSING_SESSION: &str =
    "Sign in at platform.claude.com in a supported browser, or paste its Cookie header.";
const COOKIES_OFF: &str = "Claude Platform cookies are disabled.";
const BILLING_DENIED: &str =
    "Claude Console billing access was denied. Ask the organization owner for billing access.";
const NO_API_ORGANIZATION: &str = "This Claude account has no Claude Console API organization. Create one at platform.claude.com.";
const RATE_LIMITED: &str = "Claude Console requests are rate limited. Try again in a few minutes.";
const UNAVAILABLE: &str = "Claude Console is unavailable. Try again later.";

pub struct ClaudePlatformProvider {
    metadata: ProviderMetadata,
    client: Option<Client>,
    base_url: String,
}

/// Why one Console request did not return a JSON body.
enum RequestError {
    Unauthorized,
    Forbidden,
    Failed(ProviderError),
}

/// Why one browser session did not produce usage.
enum SessionError {
    /// The Console rejected the session; the next one may still work.
    Rejected,
    Failed(ProviderError),
}

impl From<ProviderError> for SessionError {
    fn from(error: ProviderError) -> Self {
        SessionError::Failed(error)
    }
}

impl ClaudePlatformProvider {
    pub fn new() -> Self {
        Self::with_base_url(BASE_URL)
    }

    fn with_base_url(base_url: &str) -> Self {
        let client = crate::core::credentialed_http_client_builder()
            .cookie_store(false)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok();
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::ClaudePlatform,
                display_name: "Claude Platform",
                session_label: "API credit",
                weekly_label: "Purchased credit",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(DASHBOARD_URL),
                status_page_url: Some(STATUS_PAGE_URL),
                tertiary_label_key: None,
            },
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// A pasted or shell-resolved header is the only session tried. Otherwise
    /// each browser holding a `sessionKey` for platform.claude.com is tried in
    /// browser order, moving on only after the Console rejects a session
    /// (upstream `rejectCookie`).
    async fn fetch_web<S: BrowserCookieStore>(
        &self,
        ctx: &FetchContext,
        store: S,
        now: DateTime<Utc>,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let client = self.client.as_ref().ok_or_else(|| {
            ProviderError::Other("Could not create a secure Claude Platform HTTP client.".into())
        })?;
        let timeout = Duration::from_secs(ctx.web_timeout.max(1)).min(REQUEST_TIMEOUT);
        if let Some(raw) = ctx.manual_cookie_header.as_deref() {
            let cookie =
                crate::providers::normalize_cookie_header(raw).ok_or_else(missing_session)?;
            return match self.fetch_session(client, &cookie, timeout, now).await {
                Ok(result) => Ok(result),
                Err(SessionError::Rejected) => Err(ProviderError::Other(SESSION_EXPIRED.into())),
                Err(SessionError::Failed(error)) => Err(error),
            };
        }
        if ctx.manual_cookie_missing {
            return Err(missing_session());
        }

        let mut sessions =
            crate::providers::browser_cookie_sessions_from(store, ctx, &[COOKIE_DOMAIN]);
        let mut rejected = false;
        for header in sessions.by_ref() {
            let Some(cookie) = crate::providers::normalize_cookie_header(&header) else {
                continue;
            };
            if crate::providers::cookie_values(&cookie, SESSION_COOKIE).is_empty() {
                continue;
            }
            match self.fetch_session(client, &cookie, timeout, now).await {
                Ok(result) => return Ok(result),
                Err(SessionError::Rejected) => rejected = true,
                Err(SessionError::Failed(error)) => return Err(error),
            }
        }
        if rejected {
            return Err(ProviderError::Other(SESSION_EXPIRED.into()));
        }
        match sessions.exhaustion_error() {
            ProviderError::NoCookies => Err(missing_session()),
            error => {
                tracing::debug!(%error, "Claude Platform browser sessions are unavailable");
                Err(error)
            }
        }
    }

    async fn fetch_session(
        &self,
        client: &Client,
        cookie: &str,
        timeout: Duration,
        now: DateTime<Utc>,
    ) -> Result<ProviderFetchResult, SessionError> {
        // The organization list doubles as the session check; a rejected
        // cookie answers 403 there.
        let organizations = match self
            .get_json(client, "/api/organizations", cookie, timeout)
            .await
        {
            Ok(value) => value,
            Err(RequestError::Unauthorized | RequestError::Forbidden) => {
                return Err(SessionError::Rejected);
            }
            Err(RequestError::Failed(error)) => return Err(error.into()),
        };
        let candidates = api_organizations(&organizations)?;
        if candidates.is_empty() {
            return Err(ProviderError::Other(NO_API_ORGANIZATION.into()).into());
        }

        // The first API organization whose billing this member can read wins.
        let mut selected = None;
        for candidate in candidates {
            let path = format!("/api/organizations/{}/prepaid/credits", candidate.id);
            match self.get_json(client, &path, cookie, timeout).await {
                Ok(credits) => {
                    selected = Some((candidate, credits));
                    break;
                }
                Err(RequestError::Forbidden) => continue,
                Err(RequestError::Unauthorized) => return Err(SessionError::Rejected),
                Err(RequestError::Failed(error)) => return Err(error.into()),
            }
        }
        let Some((organization, credits)) = selected else {
            return Err(ProviderError::Other(BILLING_DENIED.into()).into());
        };
        let credits = parse_credits(&credits, now)?;

        // Spend is optional: any failure keeps the authenticated balance.
        let spend_path = format!("/api/organizations/{}/current_spend", organization.id);
        let spend = match self.get_json(client, &spend_path, cookie, timeout).await {
            Ok(value) => parse_spend(&value),
            Err(_) => None,
        };
        Ok(build_result(organization, &credits, spend, now))
    }

    async fn get_json(
        &self,
        client: &Client,
        path: &str,
        cookie: &str,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let response = client
            .get(format!("{}{path}", self.base_url))
            .header(ACCEPT, "application/json")
            .header(COOKIE, cookie)
            .timeout(timeout)
            .send()
            .await
            .map_err(|error| RequestError::Failed(ProviderError::Network(error)))?;
        let status = response.status();
        match status {
            StatusCode::UNAUTHORIZED => return Err(RequestError::Unauthorized),
            StatusCode::FORBIDDEN => return Err(RequestError::Forbidden),
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(RequestError::Failed(ProviderError::Other(
                    RATE_LIMITED.into(),
                )));
            }
            status if status.is_server_error() => {
                return Err(RequestError::Failed(ProviderError::Other(
                    UNAVAILABLE.into(),
                )));
            }
            StatusCode::OK => {}
            status => {
                return Err(RequestError::Failed(ProviderError::Other(format!(
                    "Claude Console returned HTTP {}. Try again later or report this.",
                    status.as_u16()
                ))));
            }
        }
        let body = read_bounded_response(response, MAX_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                RequestError::Failed(match error {
                    BoundedBodyError::Read(error) => ProviderError::Network(error),
                    BoundedBodyError::TooLarge => invalid("JSON response"),
                })
            })?;
        serde_json::from_slice(&body).map_err(|_| RequestError::Failed(invalid("JSON response")))
    }
}

impl Default for ClaudePlatformProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ClaudePlatformProvider {
    fn id(&self) -> ProviderId {
        ProviderId::ClaudePlatform
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                self.fetch_web(ctx, SystemCookieStore, Utc::now()).await
            }
            // The shell maps an Off cookie source to Cli for web-only
            // providers; upstream reports that cookies are disabled.
            SourceMode::Cli => Err(ProviderError::Other(COOKIES_OFF.into())),
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    /// Manual mode sends only the pasted header, so an empty manual source
    /// must not fall back to a browser session.
    fn manual_empty_cookie_policy(&self) -> ManualEmptyCookiePolicy {
        ManualEmptyCookiePolicy::FailClosedWeb
    }

    /// The provider walks browsers itself so a rejected session can move on
    /// to the next browser.
    fn owns_browser_cookie_resolution(&self) -> bool {
        true
    }

    fn error_state_kind(&self, error: &ProviderError) -> ProviderStateKind {
        match error {
            ProviderError::Other(message) if message == SESSION_EXPIRED => {
                ProviderStateKind::ExpiredSession
            }
            _ => error.state_kind(),
        }
    }
}

fn missing_session() -> ProviderError {
    ProviderError::BrowserSignInRequired {
        message: MISSING_SESSION.into(),
        sign_in_url: SIGN_IN_URL.into(),
    }
}

fn invalid(field: &str) -> ProviderError {
    ProviderError::Parse(format!(
        "Could not parse Claude Console {field}. The Console response format may have changed; update CodexBar or report this."
    ))
}

fn object<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, ProviderError> {
    value.as_object().ok_or_else(|| invalid(field))
}

/// Upstream `Number.isSafeInteger`: a JSON number with no fraction whose
/// magnitude fits in 2^53 - 1. Strings, floats with a fraction and huge
/// exponents are rejected.
fn safe_integer(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_number()?;
    if let Some(integer) = number.as_i64() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "comparison against the 2^53 - 1 safe-integer bound"
        )]
        return ((integer as f64).abs() <= MAX_SAFE_INTEGER).then_some(integer);
    }
    let float = number.as_f64()?;
    if float.is_finite() && float.fract() == 0.0 && float.abs() <= MAX_SAFE_INTEGER {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "checked to be a whole number within the safe-integer range"
        )]
        return Some(float as i64);
    }
    None
}

fn minor_units(value: Option<&Value>, field: &str) -> Result<i64, ProviderError> {
    safe_integer(value).ok_or_else(|| invalid(field))
}

/// A trimmed, non-empty string.
fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// An ISO timestamp, or `None` when absent or malformed: a bad date means an
/// unknown expiry, never a failed refresh.
fn timestamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let raw = text(value)?;
    let bytes = raw.as_bytes();
    let date_prefix = bytes.len() > 10
        && bytes[..10]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
        && bytes[10] == b'T';
    if !date_prefix {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|date| date.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|date| date.and_utc())
        })
}

#[derive(Debug, Clone)]
struct Organization {
    id: String,
    name: Option<String>,
}

/// Every listed organization whose `capabilities` include `api`, in listed
/// order. Claude.ai chat organizations share the list and are ignored.
fn api_organizations(value: &Value) -> Result<Vec<Organization>, ProviderError> {
    let list = value.as_array().ok_or_else(|| invalid("organizations"))?;
    let mut candidates = Vec::new();
    for entry in list {
        let organization = object(entry, "organization")?;
        let is_api = organization
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|capabilities| {
                capabilities.iter().any(|item| item.as_str() == Some("api"))
            });
        if !is_api {
            continue;
        }
        let id = text(organization.get("uuid"))
            .filter(|id| {
                id.len() <= 64
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            .ok_or_else(|| invalid("organization ID"))?;
        candidates.push(Organization {
            id: id.to_owned(),
            name: text(organization.get("name")).map(str::to_owned),
        });
    }
    Ok(candidates)
}

#[derive(Debug, Clone, PartialEq)]
struct Grant {
    label: String,
    program: Option<String>,
    purchased: bool,
    remaining: i64,
    granted: i64,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct Credits {
    currency: String,
    scale: f64,
    balance: f64,
    grants: Vec<Grant>,
}

impl Credits {
    fn money(&self, minor: i64) -> String {
        #[expect(
            clippy::cast_precision_loss,
            reason = "minor units are bounded by the 2^53 - 1 safe-integer range"
        )]
        let amount = minor as f64 / self.scale;
        money(amount, &self.currency)
    }

    fn amounts(&self, grant: &Grant) -> String {
        format!(
            "{} of {} left",
            self.money(grant.remaining),
            self.money(grant.granted)
        )
    }
}

fn money(amount: f64, currency: &str) -> String {
    if currency == "USD" {
        format::usd(amount)
    } else {
        crate::core::format_currency(amount, currency)
    }
}

fn parse_credits(value: &Value, now: DateTime<Utc>) -> Result<Credits, ProviderError> {
    let credits = object(value, "credit balance")?;
    let currency = text(credits.get("currency")).unwrap_or("USD");
    if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid("currency"));
    }

    let mut exponent = 2;
    if let Some(balance) = credits.get("balance").filter(|value| !value.is_null()) {
        let balance = object(balance, "balance")?;
        let reported = match balance.get("credits").filter(|value| !value.is_null()) {
            Some(inner) => object(inner, "balance credits")?.get("exponent"),
            None => None,
        };
        if let Some(reported) = reported.filter(|value| !value.is_null()) {
            exponent = safe_integer(Some(reported))
                .filter(|exponent| (0..=4).contains(exponent))
                .ok_or_else(|| invalid("currency exponent"))?;
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "exponent is validated to 0..=4"
    )]
    let scale = 10_f64.powi(exponent as i32);
    #[expect(
        clippy::cast_precision_loss,
        reason = "minor units are bounded by the 2^53 - 1 safe-integer range"
    )]
    let balance = minor_units(credits.get("amount"), "credit balance")? as f64 / scale;

    let mut grants = Vec::new();
    for (field, purchased) in [("promo_tranches", false), ("tranches", true)] {
        let Some(list) = credits.get(field).filter(|value| !value.is_null()) else {
            continue;
        };
        let list = list.as_array().ok_or_else(|| invalid(field))?;
        for raw in list {
            let tranche = object(raw, "credit grant")?;
            let granted = minor_units(tranche.get("granted_amount_minor_units"), "granted credit")?;
            let remaining = minor_units(
                tranche.get("remaining_amount_minor_units"),
                "remaining credit",
            )?;
            let expires_at = timestamp(tranche.get("expires_at"));
            if granted <= 0 || expires_at.is_some_and(|expires| expires <= now) {
                continue;
            }
            let fallback = if purchased {
                "Purchased credit"
            } else {
                "API credit"
            };
            let label = text(tranche.get("name"))
                .filter(|name| ProviderDisplayDetail::is_valid_title(name))
                .unwrap_or(fallback);
            grants.push(Grant {
                label: label.to_owned(),
                program: text(tranche.get("program_id")).map(str::to_owned),
                purchased,
                remaining: remaining.max(0),
                granted,
                expires_at,
            });
        }
    }
    // Soonest expiry first; grants without an expiry last. The sort is stable.
    grants.sort_by(|lhs, rhs| match (lhs.expires_at, rhs.expires_at) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(lhs), Some(rhs)) => lhs.cmp(&rhs),
    });

    Ok(Credits {
        currency: currency.to_owned(),
        scale,
        balance,
        grants,
    })
}

/// Month-to-date spend in minor units, or `None` when the report is
/// malformed or negative.
fn parse_spend(value: &Value) -> Option<i64> {
    safe_integer(value.as_object()?.get("amount")).filter(|amount| *amount >= 0)
}

/// Console grants expire at UTC midnight, so the UTC calendar date is shown
/// instead of shifting it into the local zone.
fn expiry(date: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let label = format!("{} {}", date.format("%b"), date.day());
    if date.year() == now.year() {
        format!("Expires {label} (UTC)")
    } else {
        format!("Expires {label}, {} (UTC)", date.year())
    }
}

/// Upstream `ctx.pct`: clamped to 0..=100, and 100 for an unusable limit.
fn percent(used: i64, limit: i64) -> f64 {
    if limit <= 0 {
        return 100.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "minor units are bounded by the 2^53 - 1 safe-integer range"
    )]
    let percent = used as f64 / limit as f64 * 100.0;
    percent.clamp(0.0, 100.0)
}

fn lane(credits: &Credits, grant: &Grant, now: DateTime<Utc>) -> RateWindow {
    let amounts = credits.amounts(grant);
    let description = match grant.expires_at {
        Some(expires) => format!("{amounts} · {}", expiry(expires, now)),
        None => amounts,
    };
    // Grants expire instead of resetting, so the expiry stays a detail line.
    RateWindow::with_details(
        percent(grant.granted - grant.remaining, grant.granted),
        None,
        None,
        Some(description),
    )
    .with_description_as_detail()
}

fn build_result(
    organization: Organization,
    credits: &Credits,
    spend: Option<i64>,
    now: DateTime<Utc>,
) -> ProviderFetchResult {
    let grants = &credits.grants;
    // The soonest-expiring subscription grant that still has credit is the
    // one about to be lost. An exhausted subscription grant is not a lane.
    let subscription = grants.iter().position(|grant| {
        grant.program.as_deref() == Some(SUBSCRIPTION_PROGRAM) && grant.remaining > 0
    });
    let purchased = grants
        .iter()
        .position(|grant| grant.purchased && grant.remaining > 0)
        .or_else(|| grants.iter().position(|grant| grant.purchased));

    let balance_text = money(credits.balance, &credits.currency);
    let primary = match subscription {
        Some(index) => lane(credits, &grants[index], now),
        // Without a subscription lane the tray falls back to purchased
        // credit; the card shows the balance in its Credits block.
        None => RateWindow::informational(format!("Balance: {balance_text}")).hidden_on_card(),
    };
    let mut usage = UsageSnapshot::new(primary).with_login_method("Console session");
    if let Some(index) = purchased {
        usage = usage.with_secondary(lane(credits, &grants[index], now));
    }
    if let Some(name) = organization.name {
        usage = usage.with_organization(name);
    }

    let mut rows = Vec::new();
    for (index, grant) in grants.iter().enumerate() {
        if Some(index) == subscription || Some(index) == purchased {
            continue;
        }
        let used = grant.granted - grant.remaining.min(grant.granted);
        let mut row = ProviderDisplayDetail::new(
            format!("grant-{index}"),
            grant.label.clone(),
            credits.amounts(grant),
        )
        .and_then(|row| row.with_section_title(OTHER_GRANTS_SECTION));
        if let Some(expires) = grant.expires_at {
            row = row.and_then(|row| row.with_secondary_value(expiry(expires, now)));
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "minor units are bounded by the 2^53 - 1 safe-integer range"
        )]
        let row = row.and_then(|row| row.with_progress(used as f64, grant.granted as f64));
        rows.push(row);
    }
    let spend_row = match spend {
        Some(amount) => {
            ProviderDisplayDetail::new("spend-month", "This month (UTC)", credits.money(amount))
        }
        None => ProviderDisplayDetail::new("spend", "Spend", "Unavailable"),
    };
    rows.push(spend_row.and_then(|row| row.with_section_title(BILLING_SECTION)));

    ProviderFetchResult::new(usage, "web")
        .with_cost(
            CostSnapshot::new(credits.balance, credits.currency.clone(), "Prepaid credits")
                .with_balance(credits.balance)
                .with_card_title(LocaleKey::CreditsTitle),
        )
        .with_display_details(rows.into_iter().flatten())
}
