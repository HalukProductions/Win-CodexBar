//! Opt-in personal AI Gateway budget (upstream CodexBar 0.79 `vercel-budget.js`,
//! #4623 and #4650).
//!
//! With a Vercel access token configured, the provider identifies the token's
//! user with `GET https://api.vercel.com/v2/user`, then reads the team's
//! budgets with `GET https://api.vercel.com/ai-gateway/budgets/list` (bounded
//! pagination, `scopeType=user`). The budget is matched to the user's native
//! ID or its `usr_`-prefixed form. A custom budget wins over an inherited
//! default. Daily, weekly and monthly budgets reset at the documented UTC
//! boundaries; `none` never resets. The AI Gateway key is never sent here, and
//! the access token is never sent to `ai-gateway.vercel.sh`.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, Utc};
use reqwest::StatusCode;
use serde_json::Value;

use super::{TEAM_ENV, VercelProvider};
use crate::core::{
    FetchContext, ProviderDisplayDetail, ProviderError, ProviderFetchResult, RateWindow,
    UsageSnapshot, format_currency,
};
use crate::providers::{BoundedBodyError, read_bounded_response};

const MAX_BUDGET_PAGES: usize = 20;
const PAGE_LIMIT: &str = "200";
const MAX_BUDGET_RESPONSE_BYTES: usize = 1024 * 1024;
const BUDGET_SECTION: &str = "Personal budget";
const LOGIN_METHOD: &str = "Access token";
const NOT_CONFIGURED_LABEL: &str = "No personal budget configured";
const TEAM_HINT: &str =
    "Set the Vercel Team ID in Settings or VERCEL_TEAM_ID to read your personal budget.";
const TEAM_FORMAT: &str = "Invalid Vercel Team ID. Enter the team ID or slug without spaces.";
const UNRECOGNIZED: &str = "Vercel returned an unrecognized personal budget.";

/// The access token from settings, else `VERCEL_ACCESS_TOKEN`. Blank values
/// count as unconfigured, so the team-credit path keeps running.
pub(super) fn pick_access_token(stored: Option<&str>, env: Option<&str>) -> Option<String> {
    [stored, env]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The Team ID from the provider's Team ID setting, else `VERCEL_TEAM_ID`,
/// validated before it is placed in a query string.
pub(super) fn resolve_team(
    configured: Option<&str>,
    env: Option<&str>,
) -> Result<String, ProviderError> {
    let raw = [configured, env]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty());
    let Some(team) = raw else {
        return Err(ProviderError::Other(TEAM_HINT.into()));
    };
    if !is_team_id(team) {
        return Err(ProviderError::Other(TEAM_FORMAT.into()));
    }
    Ok(team.to_string())
}

/// Shape accepted for a Vercel team ID or slug. Also used when saving the
/// Team ID setting, so an invalid value is rejected before any request.
pub(crate) fn is_team_id(value: &str) -> bool {
    value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Fixed message only: response data never reaches the user-facing error.
fn parse_failure() -> ProviderError {
    ProviderError::Parse(UNRECOGNIZED.into())
}

/// Status mapping keeps the code but never the response body.
fn status_error(status: StatusCode) -> ProviderError {
    let code = status.as_u16();
    match status {
        StatusCode::UNAUTHORIZED => ProviderError::Other(
            "Vercel rejected the access token. Check or replace it in Settings.".into(),
        ),
        StatusCode::FORBIDDEN => ProviderError::Other(format!(
            "Vercel returned HTTP {code}; the token's user needs a non-Contributor role on this team to read budgets."
        )),
        StatusCode::TOO_MANY_REQUESTS => {
            ProviderError::Other(format!("Vercel rate limit reached (HTTP {code})."))
        }
        status if status.is_server_error() => {
            ProviderError::Other(format!("Vercel is unavailable (HTTP {code})."))
        }
        _ => ProviderError::Other(format!("Vercel returned HTTP {code}.")),
    }
}

/// The token's user, from `GET /v2/user`.
#[derive(Debug, PartialEq, Eq)]
struct BudgetUser {
    id: String,
    email: Option<String>,
}

fn parse_user(body: &Value) -> Result<BudgetUser, ProviderError> {
    let user = body.get("user").ok_or_else(parse_failure)?;
    let id = user
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(parse_failure)?;
    let email = user.get("email").and_then(Value::as_str).map(str::to_owned);
    Ok(BudgetUser {
        id: id.to_owned(),
        email,
    })
}

/// Whether a budget row applies to this user and is active. Rows scoped to
/// someone else are skipped; a row that applies with malformed flags fails the
/// whole refresh.
fn matching_budget<'a>(
    budget: &'a Value,
    user_id: &str,
) -> Result<Option<&'a Value>, ProviderError> {
    if budget.get("scopeType").and_then(Value::as_str) != Some("user") {
        return Ok(None);
    }
    let prefixed = format!("usr_{user_id}");
    let scope_matches = matches!(
        budget.get("scopeId").and_then(Value::as_str),
        Some(id) if id == user_id || id == prefixed
    );
    if !scope_matches {
        return Ok(None);
    }
    let active = budget
        .get("active")
        .and_then(Value::as_bool)
        .ok_or_else(parse_failure)?;
    let archived = budget
        .get("archived")
        .and_then(Value::as_bool)
        .ok_or_else(parse_failure)?;
    match budget.get("source") {
        None => {}
        Some(Value::String(source)) if source == "default" => {}
        Some(_) => return Err(parse_failure()),
    }
    Ok((active && !archived).then_some(budget))
}

/// Custom budgets (no `source`) take precedence over inherited defaults. Two
/// budgets that both apply to the user are ambiguous and fail the refresh.
fn effective_budget(matches: &[Value]) -> Result<Option<&Value>, ProviderError> {
    let custom: Vec<&Value> = matches
        .iter()
        .filter(|budget| budget.get("source").is_none())
        .collect();
    let effective: Vec<&Value> = if custom.is_empty() {
        matches.iter().collect()
    } else {
        custom
    };
    match effective.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(*only)),
        _ => Err(parse_failure()),
    }
}

/// Start and end of the current budget period, in UTC.
type PeriodBounds = (DateTime<Utc>, DateTime<Utc>);

/// UTC reset boundaries for a refresh period. `Ok(None)` for a cumulative
/// (`none`) budget, which has no reset.
fn period_bounds(period: &str, now: DateTime<Utc>) -> Result<Option<PeriodBounds>, ProviderError> {
    let today = now.date_naive();
    let (start, end): (NaiveDate, NaiveDate) = match period {
        "daily" => (today, today.succ_opt().ok_or_else(parse_failure)?),
        "weekly" => {
            let start = today - Days::new(u64::from(today.weekday().num_days_from_monday()));
            (start, start + Days::new(7))
        }
        "monthly" => {
            let start = today.with_day(1).ok_or_else(parse_failure)?;
            let end = start
                .checked_add_months(Months::new(1))
                .ok_or_else(parse_failure)?;
            (start, end)
        }
        "none" => return Ok(None),
        _ => return Err(parse_failure()),
    };
    let to_utc = |date: NaiveDate| {
        date.and_hms_opt(0, 0, 0)
            .map(|midnight| midnight.and_utc())
            .ok_or_else(parse_failure)
    };
    Ok(Some((to_utc(start)?, to_utc(end)?)))
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn budget_row(id: &str, label: &str, value: String) -> Option<ProviderDisplayDetail> {
    ProviderDisplayDetail::new(format!("vercel-budget-{id}"), label, value)?
        .with_section_title(BUDGET_SECTION)
}

/// Builds the card from the effective budget, or the unconfigured state when
/// the user has none. The percentage and reset are only present when the
/// budget has a limit and a refresh period.
fn build_result(
    budget: Option<&Value>,
    team: &str,
    user: &BudgetUser,
    now: DateTime<Utc>,
) -> Result<ProviderFetchResult, ProviderError> {
    let (primary, rows) = match budget {
        None => (
            RateWindow::informational(BUDGET_SECTION),
            vec![budget_row("status", NOT_CONFIGURED_LABEL, "—".into())],
        ),
        Some(budget) => {
            let limit = finite_number(budget.get("limitAmount"))
                .filter(|limit| *limit > 0.0)
                .ok_or_else(parse_failure)?;
            let spent = finite_number(budget.get("currentSpend"))
                .filter(|spent| *spent >= 0.0)
                .ok_or_else(parse_failure)?;
            let period = budget
                .get("refreshPeriod")
                .and_then(Value::as_str)
                .ok_or_else(parse_failure)?;
            let bounds = period_bounds(period, now)?;
            let spent_text = format_currency(spent, "USD");
            let limit_text = format_currency(limit, "USD");
            let remaining_text = format_currency((limit - spent).max(0.0), "USD");

            let mut window = RateWindow::new(spent / limit * 100.0);
            window.reset_description = Some(format!("{spent_text} / {limit_text}"));
            window.description_is_detail = true;
            if let Some((start, end)) = bounds {
                window.window_minutes = u32::try_from((end - start).num_minutes()).ok();
                window.resets_at = Some(end);
            }
            let rows = vec![
                budget_row("spend", "Spend", spent_text),
                budget_row("limit", "Limit", limit_text),
                budget_row("remaining", "Remaining", remaining_text),
            ];
            (window, rows)
        }
    };

    let mut usage = UsageSnapshot::new(primary)
        .with_primary_label(BUDGET_SECTION)
        .with_login_method(LOGIN_METHOD)
        .with_organization(team);
    if let Some(email) = user.email.as_deref() {
        usage = usage.with_email(email);
    }
    Ok(ProviderFetchResult::new(usage, "api").with_display_details(rows.into_iter().flatten()))
}

impl VercelProvider {
    /// Personal budget path. `token` is the Vercel access token and is sent
    /// only to `api_base_url`.
    pub(super) async fn fetch_personal_budget(
        &self,
        ctx: &FetchContext,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let team_env = std::env::var(TEAM_ENV).ok();
        let team = resolve_team(ctx.workspace_id.as_deref(), team_env.as_deref())?;
        let user_url = format!("{}/v2/user", self.api_base_url);
        let user = parse_user(&self.get_budget_json(&user_url, token, &[]).await?)?;

        let budgets_url = format!("{}/ai-gateway/budgets/list", self.api_base_url);
        let mut matches: Vec<Value> = Vec::new();
        let mut seen_cursors: HashSet<String> = HashSet::new();
        let mut cursor: Option<String> = None;
        let mut finished = false;
        for page in 0..MAX_BUDGET_PAGES {
            let mut query = vec![
                ("teamId", team.as_str()),
                ("scopeType", "user"),
                ("limit", PAGE_LIMIT),
            ];
            if let Some(current) = cursor.as_deref() {
                query.push(("cursor", current));
            }
            let body = self.get_budget_json(&budgets_url, token, &query).await?;
            let rows = body
                .get("budgets")
                .and_then(Value::as_array)
                .ok_or_else(parse_failure)?;
            for row in rows {
                if let Some(budget) = matching_budget(row, &user.id)? {
                    matches.push(budget.clone());
                }
            }
            match body.get("nextCursor") {
                None | Some(Value::Null) => {
                    finished = true;
                    break;
                }
                Some(Value::String(next))
                    if !next.is_empty()
                        && page + 1 < MAX_BUDGET_PAGES
                        && seen_cursors.insert(next.clone()) =>
                {
                    cursor = Some(next.clone());
                }
                Some(_) => return Err(parse_failure()),
            }
        }
        if !finished {
            return Err(parse_failure());
        }

        let budget = effective_budget(&matches)?;
        build_result(budget, &team, &user, now)
    }

    async fn get_budget_json(
        &self,
        url: &str,
        token: &str,
        query: &[(&str, &str)],
    ) -> Result<Value, ProviderError> {
        let response = self
            .client
            .get(url)
            .query(query)
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(status_error(status));
        }
        let body = read_bounded_response(response, MAX_BUDGET_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Read(error) => ProviderError::Network(error),
                BoundedBodyError::TooLarge => parse_failure(),
            })?;
        serde_json::from_slice(&body).map_err(|_| parse_failure())
    }
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
