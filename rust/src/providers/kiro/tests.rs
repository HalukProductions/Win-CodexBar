use chrono::{TimeZone, Utc};

use super::{KiroProvider, usage_limits};
use crate::core::{Provider, ProviderError, ProviderFetchResult, ProviderId, RateWindow};

fn parse(output: &str) -> crate::core::UsageSnapshot {
    KiroProvider::new()
        .parse_cli_output(output)
        .expect("Kiro CLI output should parse")
}

fn limits(plan_limit: f64, plan_used: f64) -> usage_limits::KiroUsageLimits {
    usage_limits::KiroUsageLimits {
        plan_limit,
        plan_used,
        overage_used: 0.0,
        overage_cap: None,
        overage_enabled: Some(false),
        overage_charges: None,
        overage_rate: None,
        currency_code: "USD".to_string(),
        resets_at: Utc.timestamp_opt(1_790_812_800, 0).single().unwrap(),
        has_unseparated_bonus: false,
    }
}

#[test]
fn summary_preserves_plan_without_inventing_usage() {
    let usage = parse("\u{1b}[32mPlan: KIRO PRO MAX | 1 usage breakdowns\u{1b}[0m\n");

    assert_eq!(usage.login_method.as_deref(), Some("KIRO PRO MAX"));
    assert!(usage.primary.is_informational);
    assert_eq!(usage.primary.used_percent, 0.0);
    assert_eq!(
        usage.primary.reset_description.as_deref(),
        Some("Usage unavailable")
    );
}

#[test]
fn managed_plan_without_metrics_is_unavailable() {
    let usage = parse("Plan: Q Developer Pro\nYour plan is managed by admin\n");

    assert_eq!(usage.login_method.as_deref(), Some("Q Developer Pro"));
    assert!(usage.primary.is_informational);
}

#[test]
fn managed_marker_without_plan_is_not_a_valid_summary() {
    assert!(
        KiroProvider::new()
            .parse_cli_output("Your plan is managed by admin\n")
            .is_err()
    );
}

#[test]
fn malformed_summaries_do_not_become_plan_only_usage() {
    for output in [
        "Plan: KIRO PRO MAX",
        "Plan: KIRO PRO MAX | usage breakdowns",
        "Plan: KIRO PRO MAX | -1 usage breakdowns",
        "Plan: KIRO PRO MAX | 1.5 usage breakdowns",
        "Plan: KIRO PRO MAX | 1 usage breakdowns failed",
        "Plan: | 1 usage breakdowns",
        "echo Plan: KIRO PRO MAX | 1 usage breakdowns",
        "Plan: KIRO PRO MAX |\n1 usage breakdowns",
    ] {
        assert!(
            KiroProvider::new().parse_cli_output(output).is_err(),
            "expected parse failure for {output:?}"
        );
    }
}

#[test]
fn summary_with_real_zero_usage_keeps_available_allowance() {
    let usage = parse(
        "Plan: KIRO PRO MAX | 1 usage breakdowns\nCredits (0 of 5000 covered in plan)\nresets on 12/31\n",
    );

    assert_eq!(usage.login_method.as_deref(), Some("KIRO PRO MAX"));
    assert!(!usage.primary.is_informational);
    assert_eq!(usage.primary.used_percent, 0.0);
    assert!(usage.primary.resets_at.is_some());
}

#[test]
fn cli_brief_summary_reports_unavailable_instead_of_zero() {
    let result =
        ProviderFetchResult::new(parse("Plan: KIRO PRO MAX | 1 usage breakdowns\n"), "test");
    let brief = crate::cli::usage::render_brief_text(ProviderId::Kiro, &result);

    assert!(brief.contains("Session unavailable"));
    assert!(!brief.contains("Session 0%"));
    assert!(brief.contains("KIRO PRO MAX"));
}

#[test]
fn plan_only_summary_keeps_bonus_and_overage_metadata() {
    let usage = parse(
        "Plan: KIRO PRO MAX | 1 usage breakdowns\n\
         Bonus credits: 10/100 credits used, expires in 3 days\n\
         Overages: Enabled\n\
         Credits used: 4.5\n\
         Est. cost: $1.25 USD\n",
    );

    assert!(usage.primary.is_informational);
    assert_eq!(
        usage.secondary.as_ref().map(|window| window.used_percent),
        Some(10.0)
    );
    assert_eq!(usage.extra_rate_windows.len(), 2);
    assert!(
        usage
            .extra_rate_windows
            .iter()
            .any(|row| row.id == "kiro-overage-credits" && row.window.is_informational)
    );
    assert!(
        usage
            .extra_rate_windows
            .iter()
            .any(|row| row.id == "kiro-overage-cost" && row.window.is_informational)
    );
}

#[test]
fn positive_api_allowance_enriches_plan_only_summary() {
    let usage = usage_limits::apply_usage_limits(
        parse("Plan: KIRO PRO MAX | 1 usage breakdowns\n"),
        &limits(5000.0, 282.49),
    );

    assert!(!usage.primary.is_informational);
    assert!((usage.primary.used_percent - 5.6498).abs() < 0.0001);
    assert_eq!(
        usage.primary.resets_at,
        Some(limits(5000.0, 282.49).resets_at)
    );
    assert!(usage.primary.reset_description.is_none());
}

#[test]
fn zero_api_allowance_preserves_unknown_or_cli_metrics() {
    let unknown = usage_limits::apply_usage_limits(
        parse("Plan: KIRO PRO MAX | 1 usage breakdowns\n"),
        &limits(0.0, 0.0),
    );
    assert!(unknown.primary.is_informational);

    let known = usage_limits::apply_usage_limits(
        parse(
            "Plan: KIRO PRO MAX | 1 usage breakdowns\nCredits (20 of 50 covered in plan)\nresets on 12/31\n",
        ),
        &limits(0.0, 0.0),
    );
    assert!(!known.primary.is_informational);
    assert_eq!(known.primary.used_percent, 40.0);
    assert!(known.primary.resets_at.is_some());
}

#[test]
fn cli_credit_window_with_reset_gets_calendar_month_pace() {
    let usage = parse("Plan: KIRO PRO\nCredits (60 of 100 covered in plan)\nresets on 03/01\n");
    let resets_at = usage.primary.resets_at.expect("reset date");

    assert_eq!(
        usage.primary.window_minutes,
        RateWindow::calendar_month_window_minutes(resets_at)
    );
    let now = resets_at - chrono::Duration::days(10);
    assert!(
        ProviderId::Kiro
            .pace_capability()
            .supports_reset_window_pace(&usage.primary, now)
    );
}

#[test]
fn cli_credit_window_without_reset_has_no_window_length() {
    let usage = parse("Plan: KIRO PRO\nCredits (20 of 100 covered in plan)\n");

    assert_eq!(usage.primary.used_percent, 20.0);
    assert!(usage.primary.resets_at.is_none());
    assert_eq!(usage.primary.window_minutes, None);
    assert!(
        !ProviderId::Kiro
            .pace_capability()
            .supports_reset_window_pace(&usage.primary, Utc::now())
    );
}

#[test]
fn api_allowance_sizes_the_monthly_window_from_its_reset() {
    let reset = limits(5000.0, 282.49).resets_at;
    let usage = usage_limits::apply_usage_limits(
        parse("Plan: KIRO PRO MAX | 1 usage breakdowns\n"),
        &limits(5000.0, 282.49),
    );

    assert_eq!(
        usage.primary.window_minutes,
        RateWindow::calendar_month_window_minutes(reset)
    );
    assert!(
        ProviderId::Kiro
            .pace_capability()
            .supports_reset_window_pace(&usage.primary, reset - chrono::Duration::days(5))
    );
}

#[test]
fn bonus_window_does_not_get_monthly_pace() {
    let usage = parse(
        "Plan: KIRO PRO\nCredits (20 of 100 covered in plan)\nresets on 03/01\n\
         Bonus credits: 10/100 credits used, expires in 3 days\n",
    );
    let bonus = usage.secondary.expect("bonus window");

    assert!(
        !ProviderId::Kiro
            .pace_capability()
            .supports_reset_window_pace(&bonus, Utc::now())
    );
}

#[test]
fn cli_presence_maps_to_local_runtime_offline_but_state_db_stays_default() {
    assert_eq!(
        KiroProvider::new().error_state_kind(&ProviderError::NotInstalled(
            "kiro-cli not found. Install from https://kiro.dev".to_string(),
        )),
        crate::core::ProviderStateKind::LocalRuntimeOffline
    );
    // The state-database token lookup is auth-flavored and keeps the
    // default mapping.
    assert_eq!(
        KiroProvider::new().error_state_kind(&ProviderError::NotInstalled(
            "Kiro CLI state database not found at C:\\Users\\x\\Kiro-Cli\\data.sqlite3".to_string(),
        )),
        crate::core::ProviderStateKind::NeedsAuthentication
    );
}
