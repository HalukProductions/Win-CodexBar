//! Cost command implementation
//!
//! Scans local JSONL logs to calculate token costs for Codex and Claude.

use clap::Args;

use super::cost_period::{
    cost_totals_json, resolve_period, rolling_window_days, stamp_period, window_days,
};
use super::usage::{OutputFormat, ProviderSelection};
use crate::core::format_count as format_number;
use crate::core::{
    CostCliExtension, CostCliInvocation, CostCliMode, CostGroupedReportContext, CostGrouping,
    CostScanOptions, ProviderId, cost_cli_extension_for, cost_cli_extensions,
};
use crate::cost_reporting_period::CostReportingPeriod;
use crate::cost_scanner::{CostScanner, CostSummary};
use crate::settings::Settings;
use crate::spend_contract::build_local_spend_contract_from_summary;

/// Arguments for the cost command
#[derive(Args, Debug, Default)]
pub struct CostArgs {
    /// Provider to query (codex, claude, pi, muse, antigravity, cursor, gemini, copilot, all, both)
    #[arg(short, long)]
    pub provider: Option<String>,

    /// Output format: text or json
    #[arg(short, long, default_value = "text")]
    pub format: OutputFormat,

    /// Shorthand for --format json
    #[arg(long)]
    pub json: bool,

    /// Disable ANSI colors in text output
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Pretty-print JSON output
    #[arg(long)]
    pub pretty: bool,

    /// Cost history window in days (1..=365); always rolling and overrides --period
    #[arg(short, long)]
    pub days: Option<u32>,

    /// Cost period: month-to-date or all. Without --days or --period the
    /// saved cost period applies (30 days by default)
    #[arg(long)]
    pub period: Option<String>,

    /// A16 (upstream 0.48.0): exclude pi/OMP-compatible agent session mirrors,
    /// reporting only the provider-native local JSONL logs. When omitted
    /// (default), pi mirrors are included for backward compatibility.
    ///
    /// NOTE: locally there are no pi/OMP mirror sessions on this Windows
    /// build, so this flag is a documented divergence — it is accepted and
    /// routed through CostScanOptions::include_pi_sessions but has no
    /// observable effect in the current environment.
    #[arg(long = "provider-native-only")]
    pub provider_native_only: bool,

    /// Group Codex text output by local project or conversation/session.
    #[arg(long = "group-by", value_parser = ["project", "session"])]
    pub group_by: Option<String>,

    /// Read one SSH host; separate summaries unless --identified-only is selected.
    #[arg(long)]
    pub remote: Option<String>,

    /// Emit the versioned native Codex summary contract as JSON.
    #[arg(long = "summary-only")]
    pub summary_only: bool,

    /// Native Codex identified-request subtotal JSON; prices unavailable.
    #[arg(long = "identified-only")]
    pub identified_only: bool,

    /// Identified subtotal window end, inclusive Unix milliseconds (UTC).
    #[arg(long = "identified-end", value_name = "UNIX_MS")]
    pub identified_end: Option<i64>,

    /// Antigravity only: when a recorded model has no known public price, wait
    /// for one bounded models.dev pricing refresh and rescan. Without it the
    /// CLI starts no pricing download.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CostGroupBy {
    None,
    Project,
    Session,
}

impl CostGroupBy {
    fn from_arg(raw: Option<&str>) -> Self {
        match raw {
            Some("project") => Self::Project,
            Some("session") => Self::Session,
            _ => Self::None,
        }
    }
}

/// Run the cost command
pub async fn run(args: CostArgs) -> anyhow::Result<()> {
    run_with_settings(args, Settings::load).await
}

/// `load_settings` runs only once a mode needs saved settings; tests inject
/// defaults so they never read the user's real settings file.
async fn run_with_settings(
    args: CostArgs,
    load_settings: impl FnOnce() -> Settings,
) -> anyhow::Result<()> {
    let format = if args.json {
        OutputFormat::Json
    } else {
        args.format
    };

    let providers = ProviderSelection::from_arg(args.provider.as_deref())?;
    let group_by = CostGroupBy::from_arg(args.group_by.as_deref());
    let use_color = !args.no_color && is_terminal();

    let selected_providers = providers.as_list();
    let invocation = CostCliInvocation {
        provider_arg: args.provider.as_deref(),
        providers: &selected_providers,
        json: format == OutputFormat::Json,
        pretty: args.pretty,
        days: args.days,
        period_set: args.period.is_some(),
        group_by_set: args.group_by.is_some(),
        remote: args.remote.as_deref(),
        summary_only: args.summary_only,
        identified_only: args.identified_only,
        identified_end: args.identified_end,
    };
    // An exclusive mode replaces the report and validates the provider
    // selection itself, so it is claimed by its flags alone.
    let exclusive = cost_cli_extensions()
        .find_map(|extension| Some((extension, extension.exclusive_mode(&invocation)?)));
    if let Some((extension, mode @ CostCliMode::BeforePeriod)) = exclusive {
        return extension.run_exclusive_mode(mode, &invocation, None).await;
    }

    let settings = load_settings();
    let period = resolve_period(
        args.days,
        args.period.as_deref(),
        settings.cost_reporting_period,
        exclusive.is_some(),
    )?;
    let days = window_days(period);

    if let Some((extension, mode)) = exclusive {
        return extension
            .run_exclusive_mode(mode, &invocation, Some(days))
            .await;
    }

    let mut scan_options = CostScanOptions::app_driven();
    let requested_providers = providers.as_list();
    let pi_selected = requested_providers.contains(&ProviderId::Pi);
    // When Pi is selected alongside native providers, the standalone Pi row
    // owns its mirrored Codex/Claude events. A single native-provider request
    // keeps the historical inclusive behavior unless explicitly narrowed.
    scan_options.include_pi_sessions = !args.provider_native_only && !pi_selected;
    let scanner = CostScanner::for_period(period).with_options(scan_options);

    tracing::debug!(
        "Running cost command: providers={:?}, format={:?}, period={}, days={}",
        providers.as_list(),
        format,
        period,
        days
    );

    // Collect cost data for requested providers
    let mut results: Vec<CostResult> = Vec::new();

    for provider in providers.as_list() {
        let (summary, supported, token_history) = match provider {
            ProviderId::Codex => (scanner.scan_codex(), true, None),
            ProviderId::Claude => {
                let summary = if pi_selected || args.provider_native_only {
                    scanner.scan_claude_with_cancel_and_pi_sessions(None, false)
                } else {
                    scanner.scan_claude()
                };
                (summary, true, None)
            }
            ProviderId::Pi => (scanner.scan_pi(), true, None),
            // Recorded OpenCode Go local history, honoring the selected window
            // (upstream #4606).
            ProviderId::OpenCodeGo => (scanner.scan_opencodego_with_cancel(None), true, None),
            ProviderId::Antigravity => (
                CostSummary::default(),
                true,
                Some(
                    crate::providers::antigravity::local_sessions::summarize_with_pricing_refresh(
                        days,
                        args.refresh,
                    )
                    .await,
                ),
            ),
            ProviderId::Muse => {
                let report = crate::providers::muse::local_usage::scan(days, None);
                (CostSummary::default(), true, Some(report.into()))
            }
            // Other providers don't have local logs to scan
            _ => (CostSummary::default(), false, None),
        };
        results.push(CostResult {
            provider: provider.cli_name().to_string(),
            display_name: provider.display_name().to_string(),
            summary,
            supported,
            token_history,
        });
    }

    match format {
        OutputFormat::Text => {
            print_text_output(
                &results,
                use_color,
                period,
                group_by,
                settings.hide_personal_info,
            );
        }
        OutputFormat::Json => {
            // Upstream 0.60.4 refreshes OpenCodex prices before the JSON
            // payload; on Windows the import lives in the Codex contract.
            if results.iter().any(|result| result.provider == "codex")
                && settings.open_codex_usage_logs_enabled
            {
                crate::spend_contract::refresh_opencodex_pricing_if_needed().await;
            }
            print_json_output(&results, args.pretty, period, days, &settings)?;
        }
    }

    Ok(())
}

/// Cost result for a provider
struct CostResult {
    provider: String,
    display_name: String,
    summary: CostSummary,
    supported: bool,
    token_history: Option<crate::spend_contract::LocalTokenHistorySummary>,
}

/// Appends one formatted line to a text report buffer.
macro_rules! outln {
    ($out:expr) => {
        $out.push('\n')
    };
    ($out:expr, $($arg:tt)*) => {{
        $out.push_str(&format!($($arg)*));
        $out.push('\n');
    }};
}

/// Resolves the cost extension for a result's provider CLI name.
type ExtensionFor<'a> = &'a dyn Fn(&str) -> Option<&'a dyn CostCliExtension>;

fn registered_extension(provider: &str) -> Option<&'static dyn CostCliExtension> {
    ProviderId::from_cli_name(provider).and_then(cost_cli_extension_for)
}

/// Print text output
fn print_text_output(
    results: &[CostResult],
    use_color: bool,
    period: CostReportingPeriod,
    group_by: CostGroupBy,
    hide_personal_info: bool,
) {
    for (i, result) in results.iter().enumerate() {
        let mut out = String::new();
        render_result_text(
            &mut out,
            result,
            use_color,
            period,
            group_by,
            hide_personal_info,
            &registered_extension,
        );
        if i < results.len() - 1 {
            outln!(out);
        }
        print!("{out}");
    }
}

fn render_result_text(
    out: &mut String,
    result: &CostResult,
    use_color: bool,
    period: CostReportingPeriod,
    group_by: CostGroupBy,
    hide_personal_info: bool,
    extension_for: ExtensionFor<'_>,
) {
    let extension = extension_for(&result.provider);
    let grouped_report = |grouping| {
        extension?.grouped_report(
            grouping,
            &CostGroupedReportContext {
                summary: &result.summary,
                history_days: rolling_window_days(period),
                hide_personal_info,
            },
        )
    };
    let label = period.label();
    let title = if result.token_history.is_some() {
        format!("{} Token History ({label})", result.display_name)
    } else {
        format!("{} Cost ({label})", result.display_name)
    };
    if use_color {
        outln!(out, "\x1b[1m{title}\x1b[0m");
    } else {
        outln!(out, "{title}");
    }

    // Project grouping is checked before token history so token-history
    // providers report it unavailable; session grouping is checked after.
    if group_by == CostGroupBy::Project {
        match grouped_report(CostGrouping::Project) {
            Some(lines) => render_lines(out, &lines),
            None => outln!(
                out,
                "  Project grouping is only available for Codex local projects"
            ),
        }
    } else if let Some(history) = result.token_history.as_ref() {
        render_local_token_history(out, history);
    } else if group_by == CostGroupBy::Session {
        match grouped_report(CostGrouping::Session) {
            Some(lines) => render_lines(out, &lines),
            None => outln!(
                out,
                "  Session grouping is only available for Codex local conversations"
            ),
        }
    } else if !result.supported {
        outln!(out, "  Local cost scanning not available for this provider");
        outln!(out, "  (Only Codex and Claude have local logs)");
    } else if result.summary.sessions_count == 0 {
        if result.summary.incomplete_request_count > 0 {
            // Only preliminary proxy rows exist: usage is unavailable, not $0.
            outln!(out, "  No completed usage data found");
            render_incomplete_note(out, result.summary.incomplete_request_count);
        } else if result.summary.known_zero {
            outln!(out, "  No usage in the selected period (scan complete)");
        } else {
            outln!(out, "  No usage data found");
            outln!(
                out,
                "  Check that you have used {} locally",
                result.display_name
            );
        }
    } else {
        // Total cost
        let incomplete_suffix = incomplete_suffix(result.summary.incomplete_request_count);
        if use_color {
            outln!(
                out,
                "  Total:    \x1b[32m{}\x1b[0m{incomplete_suffix}",
                result.summary.format_total()
            );
        } else {
            outln!(
                out,
                "  Total:    {}{incomplete_suffix}",
                result.summary.format_total()
            );
        }

        // Token breakdown. Unknown when a row had no usable token counts; a
        // partial sum is never shown as the total.
        if result.summary.tokens_known() {
            outln!(
                out,
                "  Tokens:   {} input, {} output, {} cached",
                format_number(result.summary.input_tokens),
                format_number(result.summary.output_tokens),
                format_number(result.summary.cached_tokens)
            );
        } else {
            outln!(
                out,
                "  Tokens:   unknown ({} rows lacked complete token counts)",
                result.summary.unknown_token_rows
            );
        }

        // Sessions
        outln!(out, "  Sessions: {}", result.summary.sessions_count);
        render_incomplete_note(out, result.summary.incomplete_request_count);

        // Cost by model
        if !result.summary.by_model.is_empty() {
            outln!(out, "  By model:");
            let mut models: Vec<_> = result.summary.by_model.iter().collect();
            models.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (model, cost) in models {
                match extension.and_then(|ext| ext.model_token_detail(&result.summary, model)) {
                    Some(detail) => outln!(out, "    {}: ${:.2} · {}", model, cost, detail),
                    None => outln!(out, "    {}: ${:.2}", model, cost),
                }
            }
        }

        if !result.summary.by_speed.is_empty() {
            outln!(out, "  Codex speed:");
            for bucket in ["standard", "fast"] {
                if let Some(cost) = result.summary.by_speed.get(bucket) {
                    let tokens = result
                        .summary
                        .by_speed_tokens
                        .get(bucket)
                        .map(|counts| format_number(counts.total()))
                        .unwrap_or_else(|| "0".to_string());
                    outln!(out, "    {}: ${:.2} ({} tokens)", bucket, cost, tokens);
                }
            }
        }

        // F18 (upstream 0.48.0): label partial pricing completeness.
        if let crate::cost_scanner::ModelPricingCompleteness::Partial { unpriced_models } =
            &result.summary.model_pricing_completeness
            && !unpriced_models.is_empty()
        {
            outln!(
                out,
                "  Pricing:  partial (unpriced: {})",
                unpriced_models.join(", ")
            );
        }

        if let Some(notice) = extension.and_then(|ext| ext.coverage_notice(&result.summary)) {
            outln!(out, "{notice}");
        }
    }
}

fn render_lines(out: &mut String, lines: &[String]) {
    for line in lines {
        outln!(out, "{line}");
    }
}

/// Token object for the default JSON payload. `null` when a row had no usable
/// token counts, so an unknown total is never reported as zero.
fn tokens_json(summary: &crate::cost_scanner::CostSummary) -> serde_json::Value {
    if !summary.tokens_known() {
        return serde_json::Value::Null;
    }
    serde_json::json!({
        "input": summary.input_tokens,
        "output": summary.output_tokens,
        "cached": summary.cached_tokens,
    })
}

/// " · Incomplete" marker for totals that exclude preliminary Claude proxy
/// rows (upstream 0.60.5 #3688).
fn incomplete_suffix(count: u32) -> &'static str {
    if count > 0 { " · Incomplete" } else { "" }
}

fn render_incomplete_note(out: &mut String, count: u32) {
    if count > 0 {
        outln!(
            out,
            "  Incomplete: {count} requests lacked final usage and were excluded from tokens and cost."
        );
    }
}

fn render_local_token_history(
    out: &mut String,
    history: &crate::spend_contract::LocalTokenHistorySummary,
) {
    use crate::spend_contract::LocalHistoryCoverage;
    match history.coverage {
        LocalHistoryCoverage::Partial if history.lower_bound => {
            outln!(
                out,
                "  Tokens:   at least {} (lower bound; scan stopped short)",
                format_number(history.total_tokens)
            );
            outln!(out, "  Sessions: at least {}", history.session_count);
        }
        LocalHistoryCoverage::Complete if history.total_tokens == 0 => {
            outln!(
                out,
                "  No token usage in the selected period (scan complete)"
            );
        }
        LocalHistoryCoverage::Complete => {
            outln!(
                out,
                "  Tokens:   {} total",
                format_number(history.total_tokens)
            );
            outln!(out, "  Sessions: {}", history.session_count);
        }
        LocalHistoryCoverage::Partial | LocalHistoryCoverage::Unavailable => {
            outln!(out, "  Local token history is unavailable or incomplete");
        }
    }
    if let Some(cost) = history.total_usd() {
        outln!(
            out,
            "  API list-price estimate: ${cost:.2} (not billed spend)"
        );
    } else if let Some(cost) = history.cost_estimate.known_subtotal_usd {
        if history.coverage == LocalHistoryCoverage::Complete {
            outln!(
                out,
                "  Known API list-price subtotal: ${cost:.2} ({} unpriced requests)",
                history.cost_estimate.coverage.unpriced
            );
        } else {
            outln!(
                out,
                "  Known API list-price subtotal: at least ${cost:.2} (history incomplete)"
            );
        }
    } else {
        outln!(out, "  Local token history; dollar costs unavailable");
    }
}

/// Print JSON output
fn build_json_payloads(
    results: &[CostResult],
    period: CostReportingPeriod,
    days: u32,
    settings: &Settings,
) -> Vec<serde_json::Value> {
    results
        .iter()
        .map(|r| {
            if let Some(history) = r.token_history.as_ref() {
                let mut payload =
                    crate::spend_contract::local_token_history_json(&r.provider, history, days);
                stamp_period(&mut payload, period);
                return payload;
            }
            if !r.supported {
                serde_json::json!({
                    "provider": r.provider,
                    "supported": false,
                    "error": "Local cost scanning not available for this provider"
                })
            } else {
                let spend_contract = matches!(r.provider.as_str(), "codex" | "claude" | "pi" | "opencodego")
                    .then(|| build_local_spend_contract_from_summary(
                        &r.provider,
                        days.clamp(1, 365),
                        settings.open_codex_usage_logs_enabled && r.provider == "codex",
                        settings.hide_native_codex_cost_when_open_codex_present && r.provider == "codex",
                        settings.hide_personal_info,
                        r.summary.clone(),
                    ));
                let mut payload = serde_json::json!({
                    "provider": r.provider,
                    "supported": true,
                    "days_scanned": days,
                    "totals": cost_totals_json(&r.provider, &r.summary),
                    "cost": {"total_usd": r.summary.total_cost_usd, "currency": "USD"},
                    "tokens": tokens_json(&r.summary),
                    "sessions_count": r.summary.sessions_count,
                    "historyCoverageIsEstablished": if matches!(r.provider.as_str(), "codex" | "pi") { serde_json::Value::Bool(r.summary.history_coverage_established) } else { serde_json::Value::Null },
                    "knownZero": if matches!(r.provider.as_str(), "codex" | "pi") { serde_json::Value::Bool(r.summary.known_zero) } else { serde_json::Value::Null },
                    "modelPricingCompleteness": match &r.summary.model_pricing_completeness {
                        crate::cost_scanner::ModelPricingCompleteness::Complete => serde_json::Value::String("complete".to_string()),
                        crate::cost_scanner::ModelPricingCompleteness::Partial { unpriced_models } => serde_json::json!({"partial": {"unpriced_models": unpriced_models}}),
                    },
                    "by_model": r.summary.by_model,
                    "by_speed": r.summary.by_speed,
                    "by_speed_tokens": r.summary.by_speed_tokens.iter().map(|(bucket, counts)| {
                        (bucket.clone(), serde_json::json!({"input": counts.input_tokens, "output": counts.output_tokens, "cached": counts.cached_tokens, "total": counts.total()}))
                    }).collect::<serde_json::Map<_, _>>(),
                    "period": {"start": r.summary.period_start.map(|d| d.to_string()), "end": r.summary.period_end.map(|d| d.to_string())},
                    "spendContract": spend_contract
                });
                // Only emitted when a scan excluded preliminary Claude proxy rows.
                if r.summary.incomplete_request_count > 0
                    && let Some(object) = payload.as_object_mut()
                {
                    object.insert(
                        "incompleteRequestCount".to_string(),
                        serde_json::json!(r.summary.incomplete_request_count),
                    );
                }
                // Only emitted when a scan had rows without usable token counts.
                if r.summary.unknown_token_rows > 0
                    && let Some(object) = payload.as_object_mut()
                {
                    object.insert(
                        "unknownTokenRowCount".to_string(),
                        serde_json::json!(r.summary.unknown_token_rows),
                    );
                }
                if let Some(object) = payload.as_object_mut()
                    && let Some(extension) = registered_extension(&r.provider)
                {
                    for (key, value) in extension.json_fields(&r.summary) {
                        object.insert(key.to_string(), value);
                    }
                }
                stamp_period(&mut payload, period);
                payload
            }
        })
        .collect()
}

fn print_json_output(
    results: &[CostResult],
    pretty: bool,
    period: CostReportingPeriod,
    days: u32,
    settings: &Settings,
) -> anyhow::Result<()> {
    let payloads = build_json_payloads(results, period, days, settings);

    super::print_json(&payloads, pretty)
}

/// Check if stdout is a terminal
fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

#[cfg(test)]
#[path = "cost_golden_tests.rs"]
mod golden_tests;
#[cfg(test)]
mod tests;
