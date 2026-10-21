//! Reporting-period selection shared by `codexbar cost` and `serve` `/cost`
//! (upstream 0.67.0 `decodeCostReportingPeriod`, `costTotals`).

use chrono::Utc;
use serde_json::{Value, json};

use crate::cost_reporting_period::{CostReportingPeriod, CostTimeZone, MAX_ROLLING_DAYS};
use crate::cost_scanner::CostSummary;

/// Pick the period for one `cost` invocation.
///
/// `--days N` always selects a rolling window and wins over `--period`;
/// without either flag the saved selection applies. Host summaries
/// (`--remote`, `--summary-only`) speak a 1..=365 day protocol, so All needs an
/// explicit `--days`.
pub(super) fn resolve_period(
    days: Option<u32>,
    period: Option<&str>,
    saved: CostReportingPeriod,
    host_summary: bool,
) -> anyhow::Result<CostReportingPeriod> {
    if let Some(days) = days {
        return Ok(CostReportingPeriod::rolling(days));
    }
    let period = match period {
        Some(raw) => match CostReportingPeriod::parse(raw) {
            Some(CostReportingPeriod::MonthToDate) => CostReportingPeriod::MonthToDate,
            Some(CostReportingPeriod::AllAvailable) => CostReportingPeriod::AllAvailable,
            _ => anyhow::bail!("--period must be month-to-date or all"),
        },
        None => saved,
    };
    if host_summary && period == CostReportingPeriod::AllAvailable {
        anyhow::bail!("host summaries support 1...365 days; override All with --days N");
    }
    Ok(period)
}

/// Days covered by `period` today, in the zone the local scanners bucket by.
pub(crate) fn window_days(period: CostReportingPeriod) -> u32 {
    period.days(Utc::now(), CostTimeZone::Local, None)
}

/// Days for consumers that only understand a rolling 1..=365 window (daily
/// chart rows, conversation grouping). Month to date fits; All is capped.
pub(crate) fn rolling_window_days(period: CostReportingPeriod) -> u32 {
    window_days(period).min(MAX_ROLLING_DAYS)
}

/// Add `reportingPeriod` (raw) and `historyLabel` to one provider payload.
pub(crate) fn stamp_period(payload: &mut Value, period: CostReportingPeriod) {
    if let Some(object) = payload.as_object_mut() {
        object.insert("reportingPeriod".to_string(), json!(period.raw()));
        object.insert("historyLabel".to_string(), json!(period.label()));
    }
}

/// Totals for the selected window. `null` when the scan found nothing and did
/// not establish a known zero, so a missing scan is never shown as `$0`.
/// `totalTokens` uses the provider's own rule, the same total the desktop
/// spend rows show (Claude and Pi add cache tokens; Codex input already
/// includes them).
///
/// Token fields are `null` when some contributing row had no usable token counts
/// (`CostSummary::tokens_known`): a partial sum is not an exact total. Cost stays
/// numeric because it comes from the recorded cost field.
pub(crate) fn cost_totals_json(provider: &str, summary: &CostSummary) -> Value {
    if summary.sessions_count == 0 && !summary.known_zero {
        return Value::Null;
    }
    if !summary.tokens_known() {
        return json!({
            "inputTokens": Value::Null,
            "outputTokens": Value::Null,
            "cachedTokens": Value::Null,
            "reasoningTokens": Value::Null,
            "totalTokens": Value::Null,
            "totalCost": summary.total_cost_usd,
        });
    }
    json!({
        "inputTokens": summary.input_tokens,
        "outputTokens": summary.output_tokens,
        "cachedTokens": summary.cached_tokens,
        "reasoningTokens": summary.reasoning_tokens,
        "totalTokens": summary.total_tokens_for_provider(provider),
        "totalCost": summary.total_cost_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVED_MTD: CostReportingPeriod = CostReportingPeriod::MonthToDate;

    #[test]
    fn days_beats_period_and_clamps() {
        let resolved = resolve_period(Some(7), Some("all"), SAVED_MTD, false).unwrap();
        assert_eq!(resolved, CostReportingPeriod::Rolling(7));
        let clamped = resolve_period(Some(900), None, SAVED_MTD, false).unwrap();
        assert_eq!(clamped, CostReportingPeriod::Rolling(365));
        let floor = resolve_period(Some(0), None, SAVED_MTD, false).unwrap();
        assert_eq!(floor, CostReportingPeriod::Rolling(1));
    }

    #[test]
    fn days_wins_even_over_an_invalid_period() {
        let resolved = resolve_period(Some(7), Some("nonsense"), SAVED_MTD, false).unwrap();
        assert_eq!(resolved, CostReportingPeriod::Rolling(7));
    }

    #[test]
    fn absent_flags_use_the_saved_period() {
        let default = CostReportingPeriod::default();
        assert_eq!(
            resolve_period(None, None, default, false).unwrap(),
            CostReportingPeriod::Rolling(30)
        );
        assert_eq!(
            resolve_period(None, None, SAVED_MTD, false).unwrap(),
            SAVED_MTD
        );
    }

    #[test]
    fn explicit_period_overrides_saved() {
        let saved = CostReportingPeriod::Rolling(7);
        assert_eq!(
            resolve_period(None, Some("all"), saved, false).unwrap(),
            CostReportingPeriod::AllAvailable
        );
        assert_eq!(
            resolve_period(None, Some("month-to-date"), saved, false).unwrap(),
            CostReportingPeriod::MonthToDate
        );
    }

    #[test]
    fn invalid_period_is_rejected() {
        let error = resolve_period(None, Some("weekly"), SAVED_MTD, false).unwrap_err();
        assert_eq!(error.to_string(), "--period must be month-to-date or all");
        assert!(resolve_period(None, Some("rolling:0"), SAVED_MTD, false).is_err());
    }

    #[test]
    fn host_summaries_reject_all_without_explicit_days() {
        let error = resolve_period(None, Some("all"), SAVED_MTD, true).unwrap_err();
        assert!(error.to_string().contains("override All with --days N"));
        let saved_all = resolve_period(None, None, CostReportingPeriod::AllAvailable, true);
        assert!(saved_all.is_err());
        assert_eq!(
            resolve_period(Some(14), Some("all"), SAVED_MTD, true).unwrap(),
            CostReportingPeriod::Rolling(14)
        );
        assert_eq!(
            resolve_period(None, None, SAVED_MTD, true).unwrap(),
            SAVED_MTD
        );
    }

    #[test]
    fn rolling_window_is_capped_at_a_year_for_all() {
        assert_eq!(rolling_window_days(CostReportingPeriod::AllAvailable), 365);
        assert_eq!(rolling_window_days(CostReportingPeriod::Rolling(30)), 30);
        assert!(window_days(CostReportingPeriod::AllAvailable) > 365);
        assert!((1..=31).contains(&window_days(CostReportingPeriod::MonthToDate)));
    }

    #[test]
    fn stamp_adds_raw_period_and_label() {
        let mut payload = json!({"provider": "codex"});
        stamp_period(&mut payload, CostReportingPeriod::MonthToDate);
        assert_eq!(payload["reportingPeriod"], "month-to-date");
        assert_eq!(payload["historyLabel"], "Month to date");
        assert_eq!(payload["provider"], "codex");

        let mut rolling = json!({});
        stamp_period(&mut rolling, CostReportingPeriod::Rolling(30));
        assert_eq!(rolling["reportingPeriod"], "rolling:30");
        assert_eq!(rolling["historyLabel"], "Last 30 days");
    }

    #[test]
    fn totals_sum_tokens_and_keep_unknown_distinct_from_zero() {
        let summary = CostSummary {
            total_cost_usd: 1.5,
            input_tokens: 100,
            output_tokens: 20,
            cached_tokens: 7,
            sessions_count: 2,
            ..CostSummary::default()
        };
        let totals = cost_totals_json("codex", &summary);
        assert_eq!(totals["inputTokens"], 100);
        assert_eq!(totals["outputTokens"], 20);
        assert_eq!(totals["cachedTokens"], 7);
        assert_eq!(totals["totalTokens"], 120);
        assert_eq!(totals["totalCost"], 1.5);
        assert!(totals["reasoningTokens"].is_null());

        assert!(cost_totals_json("codex", &CostSummary::default()).is_null());
        let known_zero = CostSummary {
            known_zero: true,
            ..CostSummary::default()
        };
        assert_eq!(cost_totals_json("codex", &known_zero)["totalCost"], 0.0);
    }

    #[test]
    fn total_tokens_follow_each_provider_cache_rule() {
        let summary = CostSummary {
            input_tokens: 100,
            output_tokens: 20,
            cached_tokens: 7,
            sessions_count: 1,
            ..CostSummary::default()
        };
        // Codex input already includes cached input; Claude and Pi report
        // cache reads and writes outside input.
        assert_eq!(cost_totals_json("codex", &summary)["totalTokens"], 120);
        assert_eq!(cost_totals_json("claude", &summary)["totalTokens"], 127);
        assert_eq!(cost_totals_json("pi", &summary)["totalTokens"], 127);
    }
}
