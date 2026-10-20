//! Selected-team quota read from the dev.meta.ai browser session.
//!
//! The Muse login response omits `subs_usage` while the 5-hour window is idle.
//! When the user opted in (automatic browser import or a pasted `llama_dev_sess`
//! cookie) and chose a browser team, the same subscription quota is read from
//! the dev.meta.ai portal. A session can see several teams and nothing in the
//! responses links a team to the CLI login, so the team is never guessed.
//!
//! Wire shapes come from upstream CodexBar v0.68.0 (`muse.ts`, #4011). The
//! device-code token is never sent to dev.meta.ai; those requests carry only
//! the session cookie.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::time::{Duration, timeout};

use super::{MAX_RESET_SECONDS, WEEKLY_WINDOW_MINUTES};
use crate::browser::cookies::{CookieError, get_cookie_headers_for_domain};
use crate::core::{
    FetchContext, ProviderDisplayDetail, ProviderFetchResult, RateWindow, UsageSnapshot,
};
use crate::providers::{cookie_values, normalize_cookie_header, read_bounded_response};

#[cfg(test)]
mod tests;

const DEV_META_ORIGIN: &str = "https://dev.meta.ai";
const DEV_META_DOMAIN: &str = "dev.meta.ai";
const SESSION_COOKIE: &str = "llama_dev_sess";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// Upstream budget: five requests across every candidate session.
const REQUEST_BUDGET: u8 = 5;
/// The 15 s login request plus this deadline stay inside the 60 s Muse fetch.
const WEB_DEADLINE: Duration = Duration::from_secs(45);
const MAX_BODY_BYTES: usize = 256 * 1024;
const MIN_WINDOW_SECONDS: f64 = 60.0;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Identity from the login response that the browser session must match.
#[derive(Debug, Clone, Default)]
pub(super) struct LoginIdentity {
    pub(super) email: Option<String>,
    pub(super) plan: Option<String>,
}

/// How the fetch context allows dev.meta.ai to be contacted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CookieAccess {
    /// No cookie was configured: never read a browser or contact dev.meta.ai.
    Off,
    /// A pasted Cookie header or cURL capture.
    Manual(String),
    /// Import the session from an installed browser.
    Browser,
}

impl CookieAccess {
    fn from_context(ctx: &FetchContext) -> Self {
        match ctx.manual_cookie_header.as_deref() {
            Some(raw) if !raw.trim().is_empty() => Self::Manual(raw.to_string()),
            _ if ctx.browser_cookie_import => Self::Browser,
            _ => Self::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowserTeam {
    id: String,
    name: String,
}

/// Why no quota is shown even though a browser session was available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeamNote {
    ChooseTeam,
    TeamNotVisible,
    NoQuota,
    PlanDiffers,
    CookiesProtected,
}

impl TeamNote {
    fn message(self) -> &'static str {
        match self {
            Self::ChooseTeam => "Choose a browser team in Muse Code settings",
            Self::TeamNotVisible => "The selected browser team is not visible to this session",
            Self::NoQuota => "No subscription quota for the selected team",
            Self::PlanDiffers => "The selected team's plan differs from the Muse login",
            Self::CookiesProtected => {
                "Browser cookies are protected by app-bound encryption; paste the Cookie header manually"
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TeamQuota {
    team: BrowserTeam,
    primary: RateWindow,
    secondary: RateWindow,
}

/// What the browser session revealed. `None` from the readers means the
/// browser path contributed nothing and the login-only result stands.
#[derive(Debug, Clone, Default)]
pub(super) struct WebReading {
    teams: Vec<BrowserTeam>,
    note: Option<TeamNote>,
    quota: Option<TeamQuota>,
}

/// One dev.meta.ai GET. The transport is a trait so the request budget and
/// session handling are testable without a network.
#[allow(
    clippy::double_must_use,
    reason = "async-trait marks its boxed futures #[must_use]"
)]
#[async_trait]
trait DevMetaApi: Send + Sync {
    async fn get(&self, path: &str, session_cookie: &str) -> Result<DevMetaResponse, Abort>;
}

struct DevMetaResponse {
    status: u16,
    body: Vec<u8>,
}

/// A transport or decoding failure that ends the whole browser path, like an
/// exception in the upstream plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Abort;

struct ReqwestDevMeta {
    client: Client,
}

impl ReqwestDevMeta {
    fn new() -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }
}

#[async_trait]
impl DevMetaApi for ReqwestDevMeta {
    async fn get(&self, path: &str, session_cookie: &str) -> Result<DevMetaResponse, Abort> {
        let response = self
            .client
            .get(format!("{DEV_META_ORIGIN}{path}"))
            .header("Cookie", session_cookie)
            .header("User-Agent", "CodexBar")
            .send()
            .await
            .map_err(|_| Abort)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Ok(DevMetaResponse {
                status,
                body: Vec::new(),
            });
        }
        let body = read_bounded_response(response, MAX_BODY_BYTES)
            .await
            .map_err(|_| Abort)?;
        Ok(DevMetaResponse { status, body })
    }
}

/// Read the selected team's quota, or `None` when the browser path is off or
/// yields nothing usable. Never returns an error: the login result stands.
pub(super) async fn fetch_reading(
    ctx: &FetchContext,
    identity: &LoginIdentity,
) -> Option<WebReading> {
    let access = CookieAccess::from_context(ctx);
    if access == CookieAccess::Off {
        return None;
    }
    let team_id = ctx
        .workspace_id
        .as_deref()
        .map_or("", str::trim)
        .to_string();
    let identity = identity.clone();
    timeout(WEB_DEADLINE, async move {
        let sessions = resolve_sessions(access).await;
        if sessions.cookies.is_empty() {
            return sessions.protected.then(|| WebReading {
                note: Some(TeamNote::CookiesProtected),
                ..WebReading::default()
            });
        }
        read_team_quota(
            &ReqwestDevMeta::new(),
            &sessions.cookies,
            &identity,
            &team_id,
            Utc::now(),
        )
        .await
    })
    .await
    .unwrap_or_else(|_| {
        tracing::debug!("Muse browser team quota timed out");
        None
    })
}

struct Sessions {
    /// Cookie headers reduced to `llama_dev_sess=<value>`.
    cookies: Vec<String>,
    /// Chromium app-bound encryption hid every browser cookie.
    protected: bool,
}

impl Sessions {
    fn none() -> Self {
        Self {
            cookies: Vec::new(),
            protected: false,
        }
    }
}

async fn resolve_sessions(access: CookieAccess) -> Sessions {
    match access {
        CookieAccess::Off => Sessions::none(),
        CookieAccess::Manual(raw) => Sessions {
            cookies: manual_session_cookie(&raw).into_iter().collect(),
            protected: false,
        },
        CookieAccess::Browser => {
            let scan =
                tokio::task::spawn_blocking(|| get_cookie_headers_for_domain(DEV_META_DOMAIN))
                    .await;
            match scan {
                Ok(Ok(headers)) => {
                    let mut cookies: Vec<String> = Vec::new();
                    for cookie in headers
                        .iter()
                        .filter_map(|(_, header)| session_cookie(header))
                    {
                        if !cookies.contains(&cookie) {
                            cookies.push(cookie);
                        }
                    }
                    Sessions {
                        cookies,
                        protected: false,
                    }
                }
                Ok(Err(CookieError::AppBoundEncryption { .. })) => Sessions {
                    cookies: Vec::new(),
                    protected: true,
                },
                _ => Sessions::none(),
            }
        }
    }
}

/// Accepts a bare Cookie header, a `Cookie:` line, or a DevTools cURL capture.
fn manual_session_cookie(raw: &str) -> Option<String> {
    let header = if crate::core::looks_like_curl_capture(raw) {
        let fields = crate::core::header_fields(raw);
        crate::core::header_value("cookie", &fields)?
    } else {
        raw.to_string()
    };
    session_cookie(&normalize_cookie_header(&header)?)
}

/// Reduce a Cookie header to the session cookie alone; anything else the
/// browser holds for dev.meta.ai is not forwarded. Duplicate values are
/// ambiguous, so they are rejected rather than picking one.
fn session_cookie(header: &str) -> Option<String> {
    let values = cookie_values(header, SESSION_COOKIE);
    let [value] = values.as_slice() else {
        return None;
    };
    value
        .chars()
        .all(|c| c.is_ascii_graphic())
        .then(|| format!("{SESSION_COOKIE}={value}"))
}

struct Budget(u8);

/// Requests for one session. `rejected` is set on 401/403 so the caller moves
/// on to the next session.
struct SessionClient<'a> {
    api: &'a dyn DevMetaApi,
    cookie: &'a str,
    budget: &'a mut Budget,
    rejected: bool,
}

impl SessionClient<'_> {
    /// `Ok(None)` for a rejected session, a non-200 status, or a non-object
    /// body; `Err` for an exhausted budget, transport failure, or invalid JSON.
    async fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<Option<T>, Abort> {
        if self.budget.0 == 0 {
            return Err(Abort);
        }
        self.budget.0 -= 1;
        let response = self.api.get(path, self.cookie).await?;
        if matches!(response.status, 401 | 403) {
            self.rejected = true;
            return Ok(None);
        }
        if response.status != 200 {
            return Ok(None);
        }
        let value: Value = serde_json::from_slice(&response.body).map_err(|_| Abort)?;
        if !value.is_object() {
            return Ok(None);
        }
        Ok(serde_json::from_value(value).ok())
    }
}

/// Deserialize a field leniently: a value of the wrong type is treated as absent.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Deserialize)]
struct MeResponse {
    #[serde(default, deserialize_with = "lenient")]
    email: Option<String>,
}

#[derive(Deserialize)]
struct TeamsResponse {
    #[serde(default, deserialize_with = "lenient")]
    teams: Option<Vec<Value>>,
}

#[derive(Deserialize, Default)]
struct TeamEntry {
    #[serde(default, deserialize_with = "lenient")]
    team_id: Option<TeamId>,
    #[serde(default, deserialize_with = "lenient")]
    team_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TeamId {
    Text(String),
    Number(u64),
}

impl TeamEntry {
    /// Entries without a digits-only id are ignored, as upstream does.
    fn into_team(self) -> Option<BrowserTeam> {
        let id = match self.team_id? {
            TeamId::Text(text) => text,
            TeamId::Number(number) if number <= MAX_SAFE_INTEGER => number.to_string(),
            TeamId::Number(_) => return None,
        };
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let name = self
            .team_name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        Some(BrowserTeam { id, name })
    }
}

#[derive(Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    subscription_quota: Value,
}

/// Limits and usage are weighted token counts encoded as decimal strings.
/// Every field stays a raw `Value` because null, absent, and malformed are
/// different cases for the reset times.
#[derive(Deserialize, Default)]
struct QuotaRecord {
    #[serde(default)]
    tier: Value,
    #[serde(default)]
    window_weighted_used: Value,
    #[serde(default)]
    window_weighted_limit: Value,
    #[serde(default)]
    window_duration_secs: Value,
    #[serde(default)]
    window_resets_at: Value,
    #[serde(default)]
    weekly_weighted_used: Value,
    #[serde(default)]
    weekly_weighted_limit: Value,
    #[serde(default)]
    weekly_resets_at: Value,
}

/// Walk the candidate sessions until one belongs to the login's account.
async fn read_team_quota(
    api: &dyn DevMetaApi,
    sessions: &[String],
    identity: &LoginIdentity,
    team_id: &str,
    now: DateTime<Utc>,
) -> Option<WebReading> {
    let mut budget = Budget(REQUEST_BUDGET);
    for cookie in sessions {
        if budget.0 == 0 {
            break;
        }
        let mut client = SessionClient {
            api,
            cookie,
            budget: &mut budget,
            rejected: false,
        };
        match read_session(&mut client, identity, team_id, now).await {
            Ok(SessionOutcome::Next) => {}
            Ok(SessionOutcome::Done(reading)) => return reading.map(|reading| *reading),
            Err(Abort) => {
                tracing::debug!("Muse browser team quota read stopped");
                return None;
            }
        }
    }
    None
}

enum SessionOutcome {
    /// This session was rejected or belongs to another account.
    Next,
    Done(Option<Box<WebReading>>),
}

async fn read_session(
    client: &mut SessionClient<'_>,
    identity: &LoginIdentity,
    team_id: &str,
    now: DateTime<Utc>,
) -> Result<SessionOutcome, Abort> {
    // The browser session must belong to the same Meta account as the CLI login.
    let me: Option<MeResponse> = client.get("/api/auth/me").await?;
    let web_email = me
        .and_then(|me| me.email)
        .map(|email| email.trim().to_lowercase());
    let login_email = identity.email.as_deref().map(str::to_lowercase);
    if login_email.is_none() || web_email != login_email {
        return Ok(SessionOutcome::Next);
    }

    let listed: Option<TeamsResponse> = client.get("/api/portal/teams").await?;
    if client.rejected {
        return Ok(SessionOutcome::Next);
    }
    let Some(entries) = listed.and_then(|listed| listed.teams) else {
        return Ok(SessionOutcome::Done(None));
    };
    let teams: Vec<BrowserTeam> = entries
        .into_iter()
        .filter_map(|entry| {
            serde_json::from_value::<TeamEntry>(entry)
                .unwrap_or_default()
                .into_team()
        })
        .collect();

    let done = |teams: Vec<BrowserTeam>, note: Option<TeamNote>, quota: Option<TeamQuota>| {
        Ok(SessionOutcome::Done(Some(Box::new(WebReading {
            teams,
            note,
            quota,
        }))))
    };
    if team_id.is_empty() {
        return done(teams, Some(TeamNote::ChooseTeam), None);
    }
    let Some(team) = teams.iter().find(|team| team.id == team_id).cloned() else {
        return done(teams, Some(TeamNote::TeamNotVisible), None);
    };

    let response: Option<QuotaResponse> = client
        .get(&format!("/api/portal/teams/{}/subscription-quota", team.id))
        .await?;
    if client.rejected {
        return Ok(SessionOutcome::Next);
    }
    let record = response
        .map(|response| response.subscription_quota)
        .filter(Value::is_object)
        .and_then(|value| serde_json::from_value::<QuotaRecord>(value).ok());
    let Some(record) = record else {
        return done(teams, Some(TeamNote::NoQuota), None);
    };
    // The team's quota must be for the same plan the CLI login reports.
    let tier = record.tier.as_str().map(str::trim);
    if identity.plan.is_none() || tier != identity.plan.as_deref() {
        return done(teams, Some(TeamNote::PlanDiffers), None);
    }
    let quota = parse_team_quota(&record, now).map(|(primary, secondary)| TeamQuota {
        team,
        primary,
        secondary,
    });
    done(teams, None, quota)
}

fn amount(value: &Value) -> Option<f64> {
    let parsed = match value {
        Value::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse::<f64>().ok()?
        }
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

fn percent(used: &Value, limit: &Value) -> Option<f64> {
    let (used, limit) = (amount(used)?, amount(limit)?);
    percent_from_amounts(used, limit)
}

fn percent_from_amounts(used: f64, limit: f64) -> Option<f64> {
    (limit > 0.0).then(|| (used / limit * 100.0).min(100.0))
}

fn reset_at(value: &Value) -> Option<DateTime<Utc>> {
    let seconds = amount(value)?;
    if seconds <= 0.0 || seconds > MAX_RESET_SECONDS {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "MAX_RESET_SECONDS bounds the value inside i64"
    )]
    let whole_seconds = seconds.trunc() as i64;
    DateTime::<Utc>::from_timestamp(whole_seconds, 0)
}

/// `(primary, secondary)` windows, or `None` when the reading cannot be
/// trusted: a stale weekly quota, a missing limit, or a malformed reset shows
/// nothing rather than an old or invented value.
fn parse_team_quota(quota: &QuotaRecord, now: DateTime<Utc>) -> Option<(RateWindow, RateWindow)> {
    let weekly_percent = percent(&quota.weekly_weighted_used, &quota.weekly_weighted_limit)?;
    // A weekly quota without a future reset is stale.
    let weekly_reset = reset_at(&quota.weekly_resets_at).filter(|reset| *reset > now)?;

    let seconds = amount(&quota.window_duration_secs)?;
    if seconds.fract() != 0.0 || seconds < MIN_WINDOW_SECONDS || seconds > MAX_SAFE_INTEGER as f64 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "seconds is a validated non-negative integer; out-of-range minutes are rejected below"
    )]
    let window_minutes = u32::try_from((seconds / 60.0).round() as u64).ok()?;

    let window_reset = reset_at(&quota.window_resets_at);
    let window_used = amount(&quota.window_weighted_used)?;
    let window_limit = amount(&quota.window_weighted_limit)?;
    let mut primary_percent = percent_from_amounts(window_used, window_limit)?;
    let no_window_reset = quota.window_resets_at.is_null();
    if (!no_window_reset && window_reset.is_none()) || (no_window_reset && window_used != 0.0) {
        return None;
    }
    // An idle 5-hour window has no reset time; one whose reset has passed
    // carries no usage into the next window.
    let active_reset = window_reset.filter(|reset| *reset > now);
    if active_reset.is_none() {
        primary_percent = 0.0;
    }
    Some((
        RateWindow::with_details(primary_percent, Some(window_minutes), active_reset, None),
        RateWindow::with_details(
            weekly_percent,
            Some(WEEKLY_WINDOW_MINUTES),
            Some(weekly_reset),
            None,
        ),
    ))
}

/// Result for an active login whose response omitted `subs_usage`, optionally
/// filled in from the selected browser team.
pub(super) fn windowless_result(
    identity: &LoginIdentity,
    reading: Option<WebReading>,
) -> ProviderFetchResult {
    let quota = reading.as_ref().and_then(|reading| reading.quota.as_ref());
    let (usage, source_label) = match quota {
        Some(quota) => (
            UsageSnapshot::new(quota.primary.clone()).with_secondary(quota.secondary.clone()),
            "oauth+web",
        ),
        None => (
            UsageSnapshot::new(RateWindow::informational(
                "Subscription active; quota was not included in this login response",
            )),
            "oauth",
        ),
    };
    let mut usage = usage.with_login_method("Muse login");
    if let Some(email) = &identity.email {
        usage = usage.with_email(email.clone());
    }
    // Weighted usage of a user-selected team is not the login's own exact
    // reading, so it never feeds pace advice.
    let mut result = ProviderFetchResult::new(usage, source_label).with_non_authoritative_pace();
    if quota.is_none() {
        result = result.with_display_detail(ProviderDisplayDetail::new(
            "quota",
            "Quota",
            "Not included in this login response",
        ));
    }
    if let Some(plan) = &identity.plan {
        result = result.with_display_detail(ProviderDisplayDetail::new("plan", "Plan", plan));
    }
    let Some(reading) = reading else {
        return result;
    };
    if let Some(note) = reading.note {
        result = result.with_display_detail(ProviderDisplayDetail::new(
            "browser-teams-status",
            "Browser teams",
            note.message(),
        ));
    }
    for team in &reading.teams {
        result = result.with_display_detail(ProviderDisplayDetail::new(
            format!("browser-team-{}", team.id),
            &team.name,
            &team.id,
        ));
    }
    if let Some(quota) = &reading.quota {
        result = result
            .with_display_detail(ProviderDisplayDetail::new(
                "browser-team",
                "Browser team quota (dev.meta.ai)",
                &quota.team.name,
            ))
            .with_display_detail(ProviderDisplayDetail::new(
                "five-hour",
                "5 hours",
                super::format_percent(quota.primary.used_percent),
            ))
            .with_display_detail(ProviderDisplayDetail::new(
                "weekly",
                "Weekly",
                super::format_percent(quota.secondary.used_percent),
            ));
    }
    result
}
