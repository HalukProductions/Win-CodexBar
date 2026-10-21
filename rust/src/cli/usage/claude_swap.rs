use serde::Serialize;

use super::parallel::fetch_in_selection_order;
use super::{
    UsageCommand, UsageOutput, UsageOutputFormat, append_status_line, fetch_provider_json_output,
    fetch_provider_text_output, format_percent, render_status_indicator, render_text_error,
    status_json,
};
use crate::core::{ProviderId, SourceMode};
use crate::providers::claude::claude_swap::{
    self, ClaudeSwapAccount, ClaudeSwapAccountAction, ClaudeSwapAccountSource,
    ClaudeSwapHistoricalUsageDto, ClaudeSwapScopedWindowDto, ClaudeSwapSpendWindowDto,
    ClaudeSwapUsageWindowDto,
};
use crate::settings::Settings;
use crate::status::{ProviderStatus as StatusInfo, fetch_provider_status};
/// Settings-derived context for the read-only claude-swap CLI paths
/// (`--all-accounts` and Auto `usage`).
struct ClaudeSwapCliContext {
    executable_path: String,
    source: ClaudeSwapAccountSource,
    hide_personal_info: bool,
    /// The configured Claude usage source is Auto (or unset). A configured
    /// non-auto source bypasses the active-slot route.
    usage_source_auto: bool,
}

/// Returns the adapter context only when the opt-in integration is enabled and
/// configured; otherwise `--all-accounts` behaves like a normal single fetch.
fn claude_swap_cli_context() -> Option<ClaudeSwapCliContext> {
    let settings = Settings::load();
    if !settings.claude_swap_enabled() {
        return None;
    }
    let executable_path = settings.claude_swap_executable_path().trim().to_string();
    if executable_path.is_empty() {
        return None;
    }
    Some(ClaudeSwapCliContext {
        executable_path,
        source: settings.claude_swap_source(),
        hide_personal_info: settings.hide_personal_info,
        usage_source_auto: SourceMode::parse(settings.usage_source(ProviderId::Claude))
            .unwrap_or(SourceMode::Auto)
            == SourceMode::Auto,
    })
}

/// Read and project the external accounts. Raw cswap output never escapes this
/// boundary: only the allow-listed projection is returned, and failures become
/// plain error strings (already sanitized by the adapter).
fn read_claude_swap_accounts(ctx: &ClaudeSwapCliContext) -> Result<Vec<ClaudeSwapAccount>, String> {
    claude_swap::read_account_list(&ctx.executable_path, ctx.source)
        .map(|list| claude_swap::project_accounts(&list, ctx.source, ctx.hide_personal_info))
        .map_err(|error| error.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSwapCliAccount<'a> {
    id: &'a str,
    slot: u32,
    label: &'a str,
    email: Option<&'a str>,
    organization: Option<&'a str>,
    alias: Option<&'a str>,
    is_active: bool,
    action: Option<ClaudeSwapAccountAction>,
    is_disabled: bool,
    status: &'a str,
    error: Option<&'a str>,
    five_hour: Option<&'a ClaudeSwapUsageWindowDto>,
    seven_day: Option<&'a ClaudeSwapUsageWindowDto>,
    scoped: &'a [ClaudeSwapScopedWindowDto],
    spend: Option<&'a ClaudeSwapSpendWindowDto>,
    usage_fetched_at: Option<chrono::DateTime<chrono::Utc>>,
    historical_usage: Option<&'a ClaudeSwapHistoricalUsageDto>,
}

impl<'a> From<&'a ClaudeSwapAccount> for ClaudeSwapCliAccount<'a> {
    fn from(account: &'a ClaudeSwapAccount) -> Self {
        Self {
            id: &account.id,
            slot: account.slot,
            label: &account.label,
            email: account.email.as_deref(),
            organization: account.organization.as_deref(),
            alias: account.alias.as_deref(),
            is_active: account.is_active,
            action: account.action,
            is_disabled: account.is_disabled,
            status: &account.status,
            error: account.error.as_deref(),
            five_hour: account.five_hour.as_ref(),
            seven_day: account.seven_day.as_ref(),
            scoped: &account.scoped,
            spend: account.spend.as_ref(),
            usage_fetched_at: account.usage_fetched_at,
            historical_usage: account.historical_usage.as_ref(),
        }
    }
}

pub(super) fn claude_swap_json_payload(
    account: &ClaudeSwapAccount,
    status: Option<&StatusInfo>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "provider": ProviderId::Claude.cli_name(),
        "source": account.source.label(),
        "account": ClaudeSwapCliAccount::from(account),
    });
    if let Some(status) = status {
        payload["status"] = status_json(status);
    }
    payload
}

fn claude_swap_windows(account: &ClaudeSwapAccount) -> Vec<String> {
    window_labels(
        account.five_hour.as_ref(),
        account.seven_day.as_ref(),
        &account.scoped,
    )
}

/// `Session N% | Weekly N% | <scoped> N%` parts, skipping absent windows.
fn window_labels(
    five_hour: Option<&ClaudeSwapUsageWindowDto>,
    seven_day: Option<&ClaudeSwapUsageWindowDto>,
    scoped: &[ClaudeSwapScopedWindowDto],
) -> Vec<String> {
    let mut windows = Vec::new();
    if let Some(window) = five_hour {
        windows.push(format!("Session {}", format_percent(window.used_percent)));
    }
    if let Some(window) = seven_day {
        windows.push(format!("Weekly {}", format_percent(window.used_percent)));
    }
    for window in scoped {
        windows.push(format!(
            "{} {}",
            window.name,
            format_percent(window.used_percent)
        ));
    }
    windows
}

fn claude_swap_spend_line(
    spend: Option<&crate::providers::claude::claude_swap::ClaudeSwapSpendWindowDto>,
) -> Option<String> {
    spend.map(|spend| {
        format!(
            "Spend {:.2}/{:.2} {} ({})",
            spend.used,
            spend.limit,
            spend.currency_code.as_deref().unwrap_or(""),
            format_percent(spend.used_percent)
        )
    })
}

fn claude_swap_historical_windows(historical: &ClaudeSwapHistoricalUsageDto) -> Vec<String> {
    window_labels(
        historical.five_hour.as_ref(),
        historical.seven_day.as_ref(),
        &historical.scoped,
    )
}

pub(super) fn render_claude_swap_text(
    account: &ClaudeSwapAccount,
    status: Option<&StatusInfo>,
    use_color: bool,
) -> String {
    let mut lines = Vec::new();
    let active = if account.is_active { " (active)" } else { "" };
    let status_indicator = render_status_indicator(status, use_color);
    lines.push(format!(
        "{} ({})  {}{}{}",
        ProviderId::Claude.display_name(),
        account.source.label(),
        account.label,
        active,
        status_indicator
    ));
    append_status_line(&mut lines, status);
    let windows = claude_swap_windows(account);
    if !windows.is_empty() {
        lines.push(format!("  {}", windows.join(" | ")));
    }
    if let Some(spend) = claude_swap_spend_line(account.spend.as_ref()) {
        lines.push(format!("  {spend}"));
    }
    if let Some(fetched_at) = account.usage_fetched_at {
        lines.push(format!("  Measured {}", fetched_at.to_rfc3339()));
    }
    if let Some(historical) = &account.historical_usage {
        let windows = claude_swap_historical_windows(historical);
        let spend = claude_swap_spend_line(historical.spend.as_ref());
        let details = [(!windows.is_empty()).then(|| windows.join(" | ")), spend]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if !details.is_empty() {
            lines.push(format!(
                "  Last known usage (captured {}): {}",
                historical.fetched_at.to_rfc3339(),
                details.join(" | ")
            ));
        }
    }
    if account.is_disabled {
        lines.push(format!("  Disabled by {}.", account.source.label()));
    }
    if let Some(error) = &account.error {
        lines.push(format!("  {error}"));
    }
    lines.join("\n")
}

pub(super) fn render_claude_swap_brief(
    source: ClaudeSwapAccountSource,
    accounts: &[ClaudeSwapAccount],
    status: Option<&StatusInfo>,
    use_color: bool,
) -> String {
    let status_indicator = render_status_indicator(status, use_color);
    let mut account_parts = Vec::new();
    for account in accounts {
        let active = if account.is_active { " (active)" } else { "" };
        let windows = claude_swap_windows(account);
        let suffix = if windows.is_empty() {
            account
                .error
                .as_deref()
                .map(|error| format!(" | {error}"))
                .unwrap_or_default()
        } else {
            format!(" | {}", windows.join(" | "))
        };
        account_parts.push(format!("{}{}{}", account.label, active, suffix));
    }
    let accounts_text = if account_parts.is_empty() {
        "no accounts".to_string()
    } else {
        account_parts.join(" | ")
    };
    let provider_status = status
        .map(|status| format!(" | Status {}", status.summary()))
        .unwrap_or_default();
    format!(
        "{} ({}){}: {}{}",
        ProviderId::Claude.display_name(),
        source.label(),
        status_indicator,
        accounts_text,
        provider_status
    )
}

async fn claude_swap_provider_status(command: &UsageCommand) -> Option<StatusInfo> {
    if command.fetch_status {
        fetch_provider_status(ProviderId::Claude).await
    } else {
        None
    }
}

/// True when the provider should be expanded through the claude-swap adapter.
fn uses_claude_swap_all_accounts(command: &UsageCommand, provider_id: ProviderId) -> bool {
    command.all_accounts && provider_id == ProviderId::Claude
}

/// Source label suffix for an ordinary fetch that replaced the active
/// claude-swap slot (upstream #4631), e.g. `web (claude-swap fallback)`.
fn fallback_label_suffix(source: ClaudeSwapAccountSource) -> String {
    format!(" ({} fallback)", source.label())
}

/// How Auto `usage --provider claude` resolves against the adapter (upstream #4631).
enum AutoUsageRoute {
    /// Not eligible (explicit source or account, integration off, or a
    /// configured non-auto source): the ordinary fetch runs unchanged.
    Ordinary,
    /// The active slot answers; the ordinary fetch does not run.
    ActiveSlot(Box<ClaudeSwapAccount>),
    /// Eligible, but the adapter failed or the active slot has no usable usage:
    /// the ordinary fetch runs and its source names the adapter fallback.
    Fallback(ClaudeSwapAccountSource),
}

/// Upstream `ClaudeSwapAccountProjection`: a live measurement is served for
/// `ok` and `unavailable` rows; any other status falls back to the row's
/// last-good measurement, which keeps its own timestamp and diagnostic.
fn slot_has_usable_usage(account: &ClaudeSwapAccount) -> bool {
    let live = matches!(account.status.as_str(), "ok" | "unavailable")
        && (account.five_hour.is_some()
            || account.seven_day.is_some()
            || !account.scoped.is_empty()
            || account.spend.is_some());
    let last_known = account.status != "ok"
        && account.historical_usage.as_ref().is_some_and(|historical| {
            historical.five_hour.is_some()
                || historical.seven_day.is_some()
                || !historical.scoped.is_empty()
                || historical.spend.is_some()
        });
    live || last_known
}

/// Only the active row may answer. An inactive healthy slot is never borrowed.
fn active_slot_for_usage(accounts: Vec<ClaudeSwapAccount>) -> Option<ClaudeSwapAccount> {
    accounts
        .into_iter()
        .find(|account| account.is_active)
        .filter(slot_has_usable_usage)
}

fn resolve_auto_usage_route(command: &UsageCommand, provider_id: ProviderId) -> AutoUsageRoute {
    let eligible = provider_id == ProviderId::Claude
        && !command.all_accounts
        && command.account.is_none()
        && command.ctx.source_mode == SourceMode::Auto;
    if !eligible {
        return AutoUsageRoute::Ordinary;
    }
    let Some(ctx) = claude_swap_cli_context() else {
        return AutoUsageRoute::Ordinary;
    };
    if !ctx.usage_source_auto {
        return AutoUsageRoute::Ordinary;
    }
    match read_claude_swap_accounts(&ctx) {
        Ok(accounts) => match active_slot_for_usage(accounts) {
            Some(account) => AutoUsageRoute::ActiveSlot(Box::new(account)),
            None => AutoUsageRoute::Fallback(ctx.source),
        },
        Err(error) => {
            tracing::debug!(
                "claude-swap active slot unavailable; using ordinary Claude usage: {error}"
            );
            AutoUsageRoute::Fallback(ctx.source)
        }
    }
}

/// JSON rows for one selected provider: claude-swap expands to one row per
/// account under `--all-accounts`; Auto Claude usage follows the active slot;
/// otherwise the provider contributes one row.
async fn provider_json_rows(
    provider_id: ProviderId,
    command: &UsageCommand,
) -> Vec<serde_json::Value> {
    if uses_claude_swap_all_accounts(command, provider_id)
        && let Some(ctx) = claude_swap_cli_context()
    {
        let status = claude_swap_provider_status(command).await;
        return match read_claude_swap_accounts(&ctx) {
            Ok(accounts) => accounts
                .iter()
                .map(|account| claude_swap_json_payload(account, status.as_ref()))
                .collect(),
            Err(error) => vec![claude_swap_error_payload(
                ctx.source,
                &error,
                status.as_ref(),
            )],
        };
    }
    match resolve_auto_usage_route(command, provider_id) {
        AutoUsageRoute::ActiveSlot(account) => {
            let status = claude_swap_provider_status(command).await;
            vec![claude_swap_json_payload(&account, status.as_ref())]
        }
        AutoUsageRoute::Fallback(source) => vec![
            fetch_provider_json_output(provider_id, command, &fallback_label_suffix(source)).await,
        ],
        AutoUsageRoute::Ordinary => {
            vec![fetch_provider_json_output(provider_id, command, "").await]
        }
    }
}

async fn collect_json_results(command: &UsageCommand) -> Vec<serde_json::Value> {
    fetch_in_selection_order(command.providers.as_slice(), |provider_id| {
        provider_json_rows(provider_id, command)
    })
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// JSON row for a failed claude-swap account read. Like every usage error
/// row it carries `errorKind`; an external tool failure is `unknown`.
fn claude_swap_error_payload(
    source: ClaudeSwapAccountSource,
    error: &str,
    status: Option<&StatusInfo>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "provider": ProviderId::Claude.cli_name(),
        "source": source.label(),
        "error": error,
        "errorKind": crate::cli::error_kind::ERROR_KIND_UNKNOWN,
    });
    if let Some(status) = status {
        payload["status"] = status_json(status);
    }
    payload
}

/// Text sections for one selected provider (see [`provider_json_rows`]).
async fn provider_text_sections(provider_id: ProviderId, command: &UsageCommand) -> Vec<String> {
    if uses_claude_swap_all_accounts(command, provider_id)
        && let Some(ctx) = claude_swap_cli_context()
    {
        let status = claude_swap_provider_status(command).await;
        return match read_claude_swap_accounts(&ctx) {
            Ok(accounts) => {
                if command.brief {
                    vec![render_claude_swap_brief(
                        ctx.source,
                        &accounts,
                        status.as_ref(),
                        command.use_color,
                    )]
                } else {
                    accounts
                        .iter()
                        .map(|account| {
                            render_claude_swap_text(account, status.as_ref(), command.use_color)
                        })
                        .collect()
                }
            }
            Err(error) => {
                let mut text = render_text_error(ProviderId::Claude, &error, command.use_color);
                if let Some(status) = status.as_ref() {
                    text.push_str(&format!(" | Status {}", status.summary()));
                }
                vec![text]
            }
        };
    }
    match resolve_auto_usage_route(command, provider_id) {
        AutoUsageRoute::ActiveSlot(account) => {
            let status = claude_swap_provider_status(command).await;
            let text = if command.brief {
                render_claude_swap_brief(
                    account.source,
                    std::slice::from_ref(account.as_ref()),
                    status.as_ref(),
                    command.use_color,
                )
            } else {
                render_claude_swap_text(&account, status.as_ref(), command.use_color)
            };
            vec![text]
        }
        AutoUsageRoute::Fallback(source) => vec![
            fetch_provider_text_output(provider_id, command, &fallback_label_suffix(source)).await,
        ],
        AutoUsageRoute::Ordinary => {
            vec![fetch_provider_text_output(provider_id, command, "").await]
        }
    }
}

pub(super) async fn collect_usage_output(command: &UsageCommand) -> UsageOutput {
    match command.format {
        UsageOutputFormat::Text => {
            let sections = fetch_in_selection_order(command.providers.as_slice(), |provider_id| {
                provider_text_sections(provider_id, command)
            })
            .await;
            UsageOutput::Text(sections.into_iter().flatten().collect())
        }
        UsageOutputFormat::Json => UsageOutput::Json {
            results: collect_json_results(command).await,
            pretty: command.pretty,
        },
        UsageOutputFormat::Toon => UsageOutput::Toon(collect_json_results(command).await),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeSwapAccountSource, active_slot_for_usage, claude_swap_error_payload,
        claude_swap_json_payload, fallback_label_suffix, render_claude_swap_text,
    };
    use crate::providers::claude::claude_swap::{
        ClaudeSwapAccount, parse_account_list, project_accounts,
    };
    use serde_json::json;

    /// Parse and project synthetic rows, the same way the adapter path does.
    fn project(accounts: serde_json::Value, active: Option<u32>) -> Vec<ClaudeSwapAccount> {
        let raw = json!({
            "schemaVersion": 1,
            "activeAccountNumber": active,
            "accounts": accounts,
        });
        let parsed = parse_account_list(&raw.to_string()).unwrap();
        project_accounts(&parsed, ClaudeSwapAccountSource::ClaudeSwap, false)
    }

    #[test]
    fn active_slot_with_live_usage_answers_and_keeps_its_measurement_time() {
        let accounts = project(
            json!([
                {"number": 1, "email": "inactive@example.com", "active": false,
                 "usageStatus": "ok", "usage": {"fiveHour": {"pct": 0.0}}},
                {"number": 3, "email": "active@example.com", "active": true,
                 "usageStatus": "ok", "usageFetchedAt": "2026-10-18T12:00:00Z",
                 "usage": {"fiveHour": {"pct": 50.0}, "sevenDay": {"pct": 41.0}}}
            ]),
            Some(3),
        );
        let account = active_slot_for_usage(accounts).expect("active slot answers");
        assert_eq!(account.slot, 3);
        assert_eq!(
            account.five_hour.as_ref().map(|w| w.used_percent),
            Some(50.0)
        );
        let payload = claude_swap_json_payload(&account, None);
        assert_eq!(payload["source"], "claude-swap");
        assert!(
            payload["account"]["usageFetchedAt"]
                .as_str()
                .is_some_and(|at| at.starts_with("2026-10-18T12:00:00"))
        );
        assert!(
            render_claude_swap_text(&account, None, false).contains("Measured 2026-10-18T12:00:00")
        );
    }

    #[test]
    fn active_slot_without_usable_usage_never_borrows_an_inactive_slot() {
        let token_expired_no_last_good = project(
            json!([
                {"number": 1, "email": "inactive@example.com", "active": false,
                 "usageStatus": "ok", "usage": {"fiveHour": {"pct": 80.0}}},
                {"number": 3, "email": "active@example.com", "active": true,
                 "usageStatus": "token_expired"}
            ]),
            Some(3),
        );
        assert!(active_slot_for_usage(token_expired_no_last_good).is_none());

        let ok_without_windows = project(
            json!([{"number": 3, "email": "active@example.com", "active": true, "usageStatus": "ok"}]),
            Some(3),
        );
        assert!(active_slot_for_usage(ok_without_windows).is_none());

        let no_active_slot = project(
            json!([{"number": 3, "email": "inactive@example.com", "active": false,
                    "usageStatus": "ok", "usage": {"fiveHour": {"pct": 50.0}}}]),
            None,
        );
        assert!(active_slot_for_usage(no_active_slot).is_none());
    }

    #[test]
    fn active_slot_serves_last_good_usage_with_its_timestamp_and_diagnostic() {
        let accounts = project(
            json!([{
                "number": 3, "email": "active@example.com", "active": true,
                "usageStatus": "token_expired",
                "lastGoodUsage": {"fiveHour": {"pct": 50.0}},
                "lastGoodFetchedAt": "2026-10-18T12:00:00Z"
            }]),
            Some(3),
        );
        let account = active_slot_for_usage(accounts).expect("last-good usage answers");
        assert!(account.five_hour.is_none(), "live windows stay empty");
        assert!(account.historical_usage.is_some());
        assert!(
            account
                .error
                .as_deref()
                .is_some_and(|e| e.contains("Token expired"))
        );
        let text = render_claude_swap_text(&account, None, false);
        assert!(text.contains("Last known usage (captured 2026-10-18T12:00:00"));
    }

    #[test]
    fn adapter_fallback_label_names_the_actual_source() {
        assert_eq!(
            fallback_label_suffix(ClaudeSwapAccountSource::ClaudeSwap),
            " (claude-swap fallback)"
        );
        assert_eq!(
            fallback_label_suffix(ClaudeSwapAccountSource::Tcr),
            " (tcr fallback)"
        );
    }

    #[test]
    fn claude_swap_error_rows_carry_an_error_kind() {
        assert_eq!(
            claude_swap_error_payload(
                ClaudeSwapAccountSource::ClaudeSwap,
                "claude-swap exited with status 2",
                None
            ),
            json!({
                "provider": "claude",
                "source": "claude-swap",
                "error": "claude-swap exited with status 2",
                "errorKind": "unknown",
            })
        );
    }
}
