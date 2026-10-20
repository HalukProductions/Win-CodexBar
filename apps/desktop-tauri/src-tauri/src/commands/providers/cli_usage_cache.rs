//! Publishes the opt-in usage cache the `codexbar` CLI falls back to
//! (upstream 0.77.0 #4554 `UsageStore+CLIUsageCache`). The file format and
//! the CLI side live in `codexbar::cli_usage_cache`.

use std::collections::HashMap;
use std::path::Path;

use codexbar::cli_usage_cache::{self, Entry};
use codexbar::core::{ProviderId, UsageSnapshot};

use crate::commands::ProviderUsageSnapshot;
use crate::state::AppState;

/// Record the outcome of one provider refresh. Only a fresh fetch under the
/// default account (no active token account or managed account) matches what
/// the CLI fetches by itself; anything else withdraws the provider's entry.
/// A failure that kept a last-good snapshot keeps the entry it came from.
pub(super) fn record_refresh(
    entries: &mut HashMap<ProviderId, Entry>,
    id: ProviderId,
    fresh_usage: Option<&UsageSnapshot>,
    uses_token_account: bool,
    published_has_error: bool,
) {
    let entry = match fresh_usage {
        Some(usage) if !uses_token_account => Entry::new(id, usage),
        Some(_) => None,
        None if !published_has_error => return,
        None => None,
    };
    match entry {
        Some(entry) => {
            entries.insert(id, entry);
        }
        None => {
            entries.remove(&id);
        }
    }
}

/// The entries to publish, or `None` to withdraw the file. Providers whose
/// cached snapshot is gone or an error (disabled, account changed, failed)
/// are left out, so a previous account's usage is never republished.
pub(super) fn entries_to_publish(
    share_usage_with_cli: bool,
    entries: &HashMap<ProviderId, Entry>,
    provider_cache: &[ProviderUsageSnapshot],
) -> Option<Vec<Entry>> {
    if !share_usage_with_cli {
        return None;
    }
    Some(
        provider_cache
            .iter()
            .filter(|snapshot| snapshot.error.is_none())
            .filter_map(|snapshot| {
                let id = ProviderId::from_cli_name(&snapshot.provider_id)?;
                entries.get(&id).cloned()
            })
            .collect(),
    )
}

/// Write or withdraw the cache at `path`. Failures are logged, never shown:
/// the cache is a convenience for the CLI.
pub(super) fn publish_to(path: &Path, entries: Option<Vec<Entry>>) {
    if entries.is_some()
        && let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("CLI usage cache directory could not be created: {error}");
        return;
    }
    if let Err(error) = cli_usage_cache::publish(path, entries) {
        tracing::warn!("CLI usage cache could not be updated: {error}");
    }
}

/// Publish the current state to the default cache path. Proof-harness runs
/// carry synthetic data and never touch the real config directory.
pub(crate) fn publish(state: &std::sync::Mutex<AppState>, share_usage_with_cli: bool) {
    let entries = {
        let Ok(guard) = state.lock() else {
            return;
        };
        if guard.proof_config.is_some() {
            return;
        }
        entries_to_publish(
            share_usage_with_cli,
            &guard.cli_usage_entries,
            &guard.provider_cache,
        )
    };
    if let Some(path) = cli_usage_cache::default_path() {
        publish_to(&path, entries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codexbar::cli_usage_cache::CliUsageCache;
    use codexbar::core::{ProviderStateKind, RateWindow, instantiate_provider};

    fn usage() -> UsageSnapshot {
        let mut usage = UsageSnapshot::new(RateWindow::with_details(42.0, Some(300), None, None));
        usage.account_email = Some("person@example.com".to_string());
        usage
    }

    fn ok_snapshot(id: ProviderId) -> ProviderUsageSnapshot {
        let mut snapshot = ProviderUsageSnapshot::from_error(
            id,
            instantiate_provider(id).metadata(),
            "placeholder".to_string(),
            ProviderStateKind::Unknown,
        );
        snapshot.error = None;
        snapshot
    }

    fn error_snapshot(id: ProviderId) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot::from_error(
            id,
            instantiate_provider(id).metadata(),
            "Authentication required".to_string(),
            ProviderStateKind::NeedsAuthentication,
        )
    }

    #[test]
    fn default_account_fetches_are_recorded_and_token_accounts_withdraw() {
        let mut entries = HashMap::new();
        record_refresh(
            &mut entries,
            ProviderId::Codex,
            Some(&usage()),
            false,
            false,
        );
        assert!(entries.contains_key(&ProviderId::Codex));

        record_refresh(&mut entries, ProviderId::Codex, Some(&usage()), true, false);
        assert!(!entries.contains_key(&ProviderId::Codex));
    }

    #[test]
    fn failures_keep_the_entry_only_while_a_last_good_snapshot_is_published() {
        let mut entries = HashMap::new();
        record_refresh(
            &mut entries,
            ProviderId::Claude,
            Some(&usage()),
            false,
            false,
        );
        record_refresh(&mut entries, ProviderId::Claude, None, false, false);
        assert!(entries.contains_key(&ProviderId::Claude));
        record_refresh(&mut entries, ProviderId::Claude, None, false, true);
        assert!(!entries.contains_key(&ProviderId::Claude));
    }

    #[test]
    fn sharing_off_withdraws_and_errored_or_missing_providers_are_left_out() {
        let mut entries = HashMap::new();
        for id in [ProviderId::Codex, ProviderId::Claude, ProviderId::Gemini] {
            record_refresh(&mut entries, id, Some(&usage()), false, false);
        }
        let cache = vec![
            ok_snapshot(ProviderId::Codex),
            error_snapshot(ProviderId::Claude),
        ];

        assert!(entries_to_publish(false, &entries, &cache).is_none());
        let published = entries_to_publish(true, &entries, &cache).unwrap();
        let providers: Vec<&str> = published
            .iter()
            .map(|entry| entry.provider.as_str())
            .collect();
        assert_eq!(providers, ["codex"]);
    }

    #[test]
    fn publish_writes_without_identity_and_sharing_off_removes_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "codexbar-cli-usage-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let path = dir.join("CodexBar").join("cli-usage-cache.json");
        let mut entries = HashMap::new();
        record_refresh(
            &mut entries,
            ProviderId::Codex,
            Some(&usage()),
            false,
            false,
        );
        let cache = vec![ok_snapshot(ProviderId::Codex)];

        publish_to(&path, entries_to_publish(true, &entries, &cache));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("person@example.com"), "{raw}");
        let loaded = CliUsageCache::load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);

        publish_to(&path, entries_to_publish(false, &entries, &cache));
        assert!(!path.exists());
    }
}
