//! Cursor API client for fetching usage information
//!
//! Uses browser cookies to authenticate with cursor.com API

use super::team_budget::CursorMemberBudget;
use crate::core::{CostSnapshot, NamedRateWindow, ProviderError, RateWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE_URL: &str = "https://cursor.com";
pub(super) const GROK_BOT_WINDOW_ID: &str = "cursor-grok-bot";
pub(super) const GROK_BOT_TITLE: &str = "Grok Bot";
/// Optional monthly additional spend for unified personal memberships.
const UNIFIED_USAGE_PATH: &str = "/api/dashboard/v2/usage-status";
const UNIFIED_USAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug)]
pub struct CursorUsageResult {
    pub(super) primary: RateWindow,
    pub(super) secondary: Option<RateWindow>,
    pub(super) model_specific: Option<RateWindow>,
    pub(super) cost: Option<CostSnapshot>,
    pub(super) email: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) grok_bot: Option<NamedRateWindow>,
}

/// Cursor API client
pub struct CursorApi {
    client: reqwest::Client,
    base_url: String,
}

impl CursorApi {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: BASE_URL.to_string(),
        }
    }

    /// Point every request at a local mock server.
    #[cfg(test)]
    pub(super) fn with_base_url(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("the test client should build"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub(super) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(super) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Fetch usage information with an already resolved Cookie header.
    pub async fn fetch_usage_with_cookie_header(
        &self,
        cookie_header: &str,
    ) -> Result<CursorUsageResult, ProviderError> {
        // Fetch usage summary and user info in parallel
        let (usage_result, user_result, sand_result) = tokio::join!(
            self.fetch_usage_summary(cookie_header),
            self.fetch_user_info(cookie_header),
            self.fetch_sand_usage(cookie_header)
        );

        let usage_summary = usage_result?;
        let unified_personal = usage_summary.uses_unified_personal_billing();
        let user_info = user_result.ok();
        let team_budget = self
            .resolve_team_budget(&usage_summary, user_info.as_ref(), cookie_header)
            .await;
        let mut result = self.build_result_with_team_budget(usage_summary, user_info, team_budget);
        result.grok_bot = sand_result.ok().flatten();
        if unified_personal && let Some(cost) = self.fetch_unified_monthly_cost(cookie_header).await
        {
            // Upstream #4593: the monthly billing read replaces the summary's
            // spend, cap and reset. Without a valid read the summary values stay.
            result.cost = Some(cost);
        }
        Ok(result)
    }

    /// Optional monthly additional spend for unified personal memberships.
    /// Every failure collapses to `None` so the usage-summary values remain.
    async fn fetch_unified_monthly_cost(&self, cookie_header: &str) -> Option<CostSnapshot> {
        let url = format!("{}{UNIFIED_USAGE_PATH}", self.base_url);
        let status = self
            .client
            .get(&url)
            .query(&[("scope", "user")])
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(UNIFIED_USAGE_TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .ok()?
            .json::<UnifiedUsageStatus>()
            .await;
        match status {
            Ok(status) => unified_monthly_cost(&status, Utc::now()),
            Err(err) => {
                tracing::debug!("Cursor unified additional spend unavailable: {err}");
                None
            }
        }
    }

    async fn fetch_usage_summary(
        &self,
        cookie_header: &str,
    ) -> Result<UsageSummary, ProviderError> {
        let url = format!("{}/api/usage-summary", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Cursor API returned {}",
                response.status()
            )));
        }

        // Try structured deserialization first, fall back to raw JSON on failure
        let text = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        serde_json::from_str::<UsageSummary>(&text).map_err(|e| {
            tracing::warn!(
                "Cursor usage-summary parse error: {e}; response length: {} bytes",
                text.len()
            );
            ProviderError::Parse(e.to_string())
        })
    }

    async fn fetch_sand_usage(
        &self,
        cookie_header: &str,
    ) -> Result<Option<NamedRateWindow>, ProviderError> {
        let url = format!("{}/api/dashboard/get-sand-usage-status", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Origin", &self.base_url)
            .body("{}")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let status: SandUsageStatus = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        Ok(status.to_window(Utc::now()))
    }

    async fn fetch_user_info(&self, cookie_header: &str) -> Result<UserInfo, ProviderError> {
        let url = format!("{}/api/auth/me", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(
                "Failed to fetch user info".to_string(),
            ));
        }

        response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    fn build_result_with_team_budget(
        &self,
        summary: UsageSummary,
        user_info: Option<UserInfo>,
        team_budget: Option<CursorMemberBudget>,
    ) -> CursorUsageResult {
        let billing_end = summary
            .billing_cycle_end
            .as_ref()
            .and_then(|s| parse_iso_date(s));
        // Upstream `CursorStatusProbe`: plan lanes carry the billing cycle's
        // length so reset-window pace scores against the real cycle.
        let cycle_minutes = billing_cycle_window_minutes(
            summary
                .billing_cycle_start
                .as_deref()
                .and_then(parse_iso_date),
            billing_end,
        );

        let (percent_used, secondary, model_specific, cost_snapshot) =
            if let Some(team_budget) = team_budget {
                let percent = clamp_percent(team_budget.used_usd / team_budget.limit_usd * 100.0);
                let cost = Self::summary_on_demand_cost(&summary, billing_end).or_else(|| {
                    Some(Self::plan_cost(
                        team_budget.used_usd,
                        team_budget.limit_usd,
                        summary.billing_cycle_start.as_deref(),
                        billing_end,
                    ))
                });
                (percent, None, None, cost)
            } else if let Some(individual) = &summary.individual_usage {
                if let Some(plan) = &individual.plan {
                    let used_cents = plan.used.unwrap_or(0) as f64;
                    let limit_cents = plan
                        .limit
                        .or_else(|| plan.breakdown.as_ref().and_then(|b| b.total))
                        .unwrap_or(0) as f64;

                    // Upstream #2255: clamp plan usage at 100% when included usage
                    // exceeds the plan limit (overage must not paint >100% bars).
                    let percent = if let Some(percent) = plan.total_percent_used {
                        clamp_percent(percent)
                    } else if limit_cents > 0.0 {
                        clamp_percent((used_cents / limit_cents) * 100.0)
                    } else {
                        0.0
                    };

                    let secondary = plan.auto_percent_used.map(|v| {
                        RateWindow::with_details(clamp_percent(v), cycle_minutes, billing_end, None)
                    });

                    let model_specific = plan.api_percent_used.map(|v| {
                        RateWindow::with_details(clamp_percent(v), cycle_minutes, billing_end, None)
                    });

                    let cost =
                        Self::summary_on_demand_cost(&summary, billing_end).unwrap_or_else(|| {
                            // Plan-included spend (cents → USD) when on-demand is off.
                            Self::plan_cost(
                                used_cents / 100.0,
                                limit_cents / 100.0,
                                summary.billing_cycle_start.as_deref(),
                                billing_end,
                            )
                        });

                    (percent, secondary, model_specific, Some(cost))
                } else if let Some(overall) = &individual.overall {
                    let percent = Self::usage_percent(overall).unwrap_or(0.0);
                    let cost = Self::on_demand_cost(Some(overall), billing_end);
                    (percent, None, None, cost)
                } else {
                    (0.0, None, None, None)
                }
            } else if let Some(team) = &summary.team_usage {
                if let Some(pooled) = &team.pooled {
                    let percent = Self::usage_percent(pooled).unwrap_or(0.0);
                    let cost = Self::on_demand_cost(Some(pooled), billing_end);
                    (percent, None, None, cost)
                } else {
                    (0.0, None, None, None)
                }
            } else {
                (0.0, None, None, None)
            };

        let primary = RateWindow::with_details(percent_used, cycle_minutes, billing_end, None);

        let plan_type = summary
            .membership_type
            .as_ref()
            .map(|t| match t.to_lowercase().as_str() {
                "enterprise" => "Cursor Enterprise".to_string(),
                "pro" => "Cursor Pro".to_string(),
                "hobby" => "Cursor Hobby".to_string(),
                "team" => "Cursor Team".to_string(),
                other => format!("Cursor {}", capitalize(other)),
            });

        let email = user_info.as_ref().and_then(|u| u.email.clone());

        CursorUsageResult {
            primary,
            secondary,
            model_specific,
            cost: cost_snapshot,
            email,
            plan_type,
            grok_bot: None,
        }
    }

    fn plan_cost(
        used_usd: f64,
        limit_usd: f64,
        billing_cycle_start: Option<&str>,
        billing_end: Option<DateTime<Utc>>,
    ) -> CostSnapshot {
        let mut cost = CostSnapshot::new(used_usd, "USD", plan_period_label(billing_cycle_start));
        if limit_usd > 0.0 {
            cost = cost.with_limit(limit_usd);
        }
        if let Some(reset) = billing_end {
            cost = cost.with_resets_at(reset);
        }
        cost
    }

    /// Individual on-demand spend, else the team's.
    fn summary_on_demand_cost(
        summary: &UsageSummary,
        billing_end: Option<DateTime<Utc>>,
    ) -> Option<CostSnapshot> {
        let individual = summary
            .individual_usage
            .as_ref()
            .and_then(|individual| individual.on_demand.as_ref());
        Self::on_demand_cost(individual, billing_end).or_else(|| {
            summary
                .team_usage
                .as_ref()
                .and_then(|team| Self::on_demand_cost(team.on_demand.as_ref(), billing_end))
        })
    }

    fn on_demand_cost(
        on_demand: Option<&OnDemandUsage>,
        billing_end: Option<DateTime<Utc>>,
    ) -> Option<CostSnapshot> {
        let usage = on_demand?;
        if usage.enabled == Some(false) {
            return None;
        }

        let (used_cents, limit_cents) = usage.used_and_limit_cents();

        if used_cents <= 0.0 && limit_cents <= 0.0 {
            return None;
        }

        // usage-summary exposes on-demand spend in cents for the billing cycle.
        // Label it explicitly so the tray/detail cost line is not a vague "Monthly".
        let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", "On-demand (billing cycle)");
        if limit_cents > 0.0 {
            cost = cost.with_limit(limit_cents / 100.0);
        }
        if let Some(reset) = billing_end {
            cost = cost.with_resets_at(reset);
        }
        Some(cost)
    }

    fn usage_percent(usage: &OnDemandUsage) -> Option<f64> {
        let (used, limit) = usage.used_and_limit_cents();
        (limit > 0.0).then_some(clamp_percent(used / limit * 100.0))
    }
}

fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 100.0)
}

/// Period label for plan-included spend from usage-summary (no new network calls).
fn plan_period_label(billing_cycle_start: Option<&str>) -> String {
    // Upstream 0.50.1 #2951: match the Cursor dashboard's name for the
    // included-usage pool (Cursor + third-party models).
    match billing_cycle_start {
        Some(start) if !start.is_empty() => format!("Cursor and Third Party (since {start})"),
        _ => "Cursor and Third Party (billing cycle)".to_string(),
    }
}
impl Default for CursorApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- API Response Types ---

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSummary {
    billing_cycle_start: Option<String>,
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    limit_type: Option<String>,
    is_unlimited: Option<bool>,
    individual_usage: Option<IndividualUsage>,
    team_usage: Option<TeamUsage>,
}

impl UsageSummary {
    /// Upstream `usesUnifiedPersonalBilling`: unified membership with a
    /// user-level limit. Team budgets keep their own path.
    pub(super) fn uses_unified_personal_billing(&self) -> bool {
        self.membership_type
            .as_deref()
            .is_some_and(|membership| membership.to_ascii_lowercase().starts_with("unified_"))
            && self
                .limit_type
                .as_deref()
                .is_some_and(|limit_type| limit_type.eq_ignore_ascii_case("user"))
    }

    pub(super) fn is_team_plan(&self) -> bool {
        matches!(
            self.membership_type
                .as_deref()
                .map(|membership| membership.to_ascii_lowercase())
                .as_deref(),
            Some("enterprise" | "business" | "team" | "teams")
        ) || self
            .limit_type
            .as_deref()
            .is_some_and(|limit_type| limit_type.eq_ignore_ascii_case("team"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    plan: Option<PlanUsage>,
    on_demand: Option<OnDemandUsage>,
    overall: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    remaining: Option<i64>,
    breakdown: Option<PlanBreakdown>,
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
    total_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanBreakdown {
    included: Option<i64>,
    bonus: Option<i64>,
    total: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDemandUsage {
    enabled: Option<bool>,
    used: Option<i64>,
    limit: Option<i64>,
    remaining: Option<i64>,
}

impl OnDemandUsage {
    /// (used, limit) in cents; a missing limit falls back to remaining + used.
    fn used_and_limit_cents(&self) -> (f64, f64) {
        let used = self.used.unwrap_or(0);
        let limit = self
            .limit
            .or_else(|| self.remaining.map(|remaining| remaining + used))
            .unwrap_or(0);
        (used as f64, limit as f64)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUsage {
    on_demand: Option<OnDemandUsage>,
    pooled: Option<OnDemandUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandUsageStatus {
    current_period_start: Option<String>,
    next_reset_timestamp_utc: Option<String>,
    usage_percent: Option<f64>,
    has_non_zero_included_limit: Option<bool>,
    included_limit_zero: Option<bool>,
    sand_trial_expires_at: Option<String>,
}

impl SandUsageStatus {
    fn to_window(&self, now: DateTime<Utc>) -> Option<NamedRateWindow> {
        let has_limit = self
            .included_limit_zero
            .map(|is_zero| !is_zero)
            .or(self.has_non_zero_included_limit);
        let has_trial = has_limit != Some(true)
            && self
                .sand_trial_expires_at
                .as_deref()
                .and_then(parse_iso_date)
                .is_some_and(|expires_at| expires_at > now);
        if has_limit != Some(true) && !has_trial {
            return None;
        }
        let used = clamp_percent(self.usage_percent?);
        let start = self
            .current_period_start
            .as_deref()
            .and_then(parse_iso_date);
        let reset = (!has_trial)
            .then(|| {
                self.next_reset_timestamp_utc
                    .as_deref()
                    .and_then(parse_iso_date)
            })
            .flatten();
        let minutes = start.zip(reset).and_then(|(start, reset)| {
            let minutes = (reset - start).num_minutes();
            // Guarded by the minutes > 0 check; rate windows are far
            // shorter than u32::MAX minutes.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "guarded by the minutes > 0 check; rate windows are far shorter than u32::MAX minutes"
            )]
            let minutes_u32 = minutes as u32;
            (minutes > 0).then_some(minutes_u32)
        });
        Some(NamedRateWindow::new(
            GROK_BOT_WINDOW_ID,
            GROK_BOT_TITLE,
            RateWindow::with_details(used, minutes, reset, None),
        ))
    }
}

/// `/api/dashboard/v2/usage-status?scope=user`. Fields stay optional so a
/// missing or null value makes the response invalid instead of a zero.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnifiedUsageStatus {
    availability: Option<String>,
    monthly_usage: Option<UnifiedMonthlyUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnifiedMonthlyUsage {
    used_cents: Option<f64>,
    limit_cents: Option<f64>,
    period_start: Option<String>,
    resets_at: Option<String>,
}

/// Monthly additional spend as a cost snapshot, or `None` when the response is
/// unavailable, malformed, or outside its period. A valid zero stays zero.
fn unified_monthly_cost(status: &UnifiedUsageStatus, now: DateTime<Utc>) -> Option<CostSnapshot> {
    if status.availability.as_deref() != Some("available") {
        return None;
    }
    let usage = status.monthly_usage.as_ref()?;
    let used_cents = usage.used_cents?;
    let limit_cents = usage.limit_cents?;
    if !used_cents.is_finite() || used_cents < 0.0 || !limit_cents.is_finite() || limit_cents < 0.0
    {
        return None;
    }
    let start = parse_iso_date(usage.period_start.as_deref()?)?;
    let reset = parse_iso_date(usage.resets_at.as_deref()?)?;
    if start > now || now >= reset {
        return None;
    }
    let mut cost = CostSnapshot::new(used_cents / 100.0, "USD", "On-demand (monthly)");
    if limit_cents > 0.0 {
        cost = cost.with_limit(limit_cents / 100.0);
    }
    Some(cost.with_resets_at(reset))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UserInfo {
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    sub: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    picture: Option<String>,
}

impl UserInfo {
    /// The email comes from the authenticated `/api/auth/me` response, so it is
    /// the only identity allowed to select a team member budget.
    pub(super) fn verified_email(&self) -> Option<&str> {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|email| !email.is_empty())
    }
}

// --- Helper functions ---

/// Billing cycle length in whole minutes (rounded), or `None` when either end
/// is missing or the cycle is empty or reversed.
fn billing_cycle_window_minutes(
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Option<u32> {
    let seconds = (end? - start?).num_seconds();
    if seconds <= 0 {
        return None;
    }
    let minutes = (seconds + 30) / 60;
    u32::try_from(minutes).ok().filter(|m| *m > 0)
}

fn parse_iso_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests;
