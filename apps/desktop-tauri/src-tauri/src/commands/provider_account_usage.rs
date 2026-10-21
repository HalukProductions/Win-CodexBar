//! Last successful usage for each saved token account, so Overview can show one
//! row per account without starting a fetch.
//!
//! The refresh that fetches the active token account records its successful
//! result here. A failed refresh keeps the last good entry, so the row shows
//! the age of its data through `updatedAt`. Entries hold only the rendered
//! rate windows and the update time: no identity, plan, email, token or error
//! text. Entries are keyed by provider and token account UUID.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use codexbar::core::ProviderId;
use codexbar::{atomic_file, secure_file};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::state::AppState;

const STORE_VERSION: u32 = 1;
const STORE_RELATIVE_PATH: &str = "provider-account-usage-v1.json";

/// One rate window as an Overview account row renders it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageWindow {
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// The last successful usage fetched for one account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageEntry {
    #[serde(default)]
    pub primary: Option<AccountUsageWindow>,
    #[serde(default)]
    pub secondary: Option<AccountUsageWindow>,
    pub updated_at: String,
}

/// Provider cli name is not used as the key here; the provider id is.
pub type ProviderAccountUsageCache = HashMap<ProviderId, HashMap<String, AccountUsageEntry>>;

/// Only providers that expand Overview account rows keep a per-account cache.
/// Add a provider here together with its Overview expansion.
pub fn records_account_usage(provider: ProviderId) -> bool {
    provider == ProviderId::Claude
}

fn window_entry(window: &super::RateWindowSnapshot) -> Option<AccountUsageWindow> {
    window.used_percent.is_finite().then(|| AccountUsageWindow {
        used_percent: window.used_percent.clamp(0.0, 100.0),
        window_minutes: window.window_minutes,
        resets_at: window.resets_at.clone(),
    })
}

/// Record one fetch outcome for a token account. Only a successful fetch
/// replaces the entry; a failed fetch leaves the last good entry in place.
/// Returns whether the cache changed.
pub fn observe_fetch(
    cache: &mut ProviderAccountUsageCache,
    provider: ProviderId,
    account_id: &str,
    snapshot: &super::ProviderUsageSnapshot,
) -> bool {
    if !records_account_usage(provider) || snapshot.error.is_some() {
        return false;
    }
    let updated_at = if snapshot.updated_at.trim().is_empty() {
        chrono::Utc::now().to_rfc3339()
    } else {
        snapshot.updated_at.clone()
    };
    let entry = AccountUsageEntry {
        primary: window_entry(&snapshot.primary),
        secondary: snapshot.secondary.as_ref().and_then(window_entry),
        updated_at,
    };
    cache
        .entry(provider)
        .or_default()
        .insert(account_id.to_string(), entry);
    true
}

/// Drop one account's entry. Returns whether an entry was removed.
pub fn forget_account(
    cache: &mut ProviderAccountUsageCache,
    provider: ProviderId,
    account_id: &str,
) -> bool {
    cache
        .get_mut(&provider)
        .is_some_and(|accounts| accounts.remove(account_id).is_some())
}

/// Drop every account entry for a provider. Returns whether any were removed.
pub fn forget_provider(cache: &mut ProviderAccountUsageCache, provider: ProviderId) -> bool {
    cache.remove(&provider).is_some()
}

/// Entries for one provider in stable account-id order.
pub fn entries_for(
    cache: &ProviderAccountUsageCache,
    provider: ProviderId,
) -> BTreeMap<String, AccountUsageEntry> {
    cache
        .get(&provider)
        .map(|accounts| {
            accounts
                .iter()
                .map(|(id, entry)| (id.clone(), entry.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Holds only rendered windows and update times. Credentials stay in the
/// token account store and are never written here.
#[derive(Serialize, Deserialize)]
struct ProviderAccountUsageStore {
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, BTreeMap<String, AccountUsageEntry>>,
}

fn encode_cache(cache: &ProviderAccountUsageCache) -> serde_json::Result<String> {
    let providers = cache
        .iter()
        .filter(|(provider, _)| records_account_usage(**provider))
        .map(|(provider, accounts)| {
            let accounts = accounts
                .iter()
                .map(|(id, entry)| (id.clone(), entry.clone()))
                .collect();
            (provider.cli_name().to_string(), accounts)
        })
        .collect();
    serde_json::to_string_pretty(&ProviderAccountUsageStore {
        version: STORE_VERSION,
        providers,
    })
}

/// An unreadable or unsupported store is ignored so it never blocks startup.
fn decode_cache(raw: &str) -> Option<ProviderAccountUsageCache> {
    let store: ProviderAccountUsageStore = serde_json::from_str(raw).ok()?;
    (store.version == STORE_VERSION).then(|| {
        store
            .providers
            .into_iter()
            .filter_map(|(name, accounts)| {
                let provider = ProviderId::from_cli_name(&name)?;
                records_account_usage(provider).then(|| (provider, accounts.into_iter().collect()))
            })
            .collect()
    })
}

fn cache_store_path() -> Option<PathBuf> {
    codexbar::logging::config_root().map(|root| root.join(STORE_RELATIVE_PATH))
}

/// Last-known per-account usage, restored at launch so Overview rows can show
/// it before the first refresh completes.
pub(crate) fn load_provider_account_usage() -> ProviderAccountUsageCache {
    let Some(path) = cache_store_path() else {
        return HashMap::new();
    };
    match secure_file::read_string(&path) {
        Ok(raw) => decode_cache(&raw).unwrap_or_else(|| {
            tracing::warn!("ignoring unreadable provider account usage cache");
            HashMap::new()
        }),
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!("failed to read provider account usage cache: {error}");
            }
            HashMap::new()
        }
    }
}

fn save_provider_account_usage(cache: &ProviderAccountUsageCache) -> io::Result<()> {
    let Some(path) = cache_store_path() else {
        return Ok(());
    };
    let raw = encode_cache(cache).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One fixed staging name is overwritten on each save, so no temp files accumulate.
    let staged = path.with_extension("json.tmp");
    secure_file::write_string(&staged, &raw)?;
    atomic_file::replace_staged(&staged, &path)
}

/// Write a cache snapshot taken under the state lock. Runs after the lock is
/// released so disk I/O never blocks other commands.
pub(crate) fn persist(cache: &ProviderAccountUsageCache) {
    if let Err(error) = save_provider_account_usage(cache) {
        tracing::warn!("failed to persist provider account usage: {error}");
    }
}

/// Remove one token account's cached usage, e.g. when the account is deleted.
pub(crate) fn forget_token_account(
    app: &tauri::AppHandle,
    provider: ProviderId,
    account_id: uuid::Uuid,
) {
    let account_id = account_id.to_string();
    let snapshot = app.try_state::<Mutex<AppState>>().and_then(|state| {
        let mut state = state.lock().ok()?;
        forget_account(&mut state.provider_account_usage, provider, &account_id)
            .then(|| state.provider_account_usage.clone())
    });
    if let Some(cache) = snapshot {
        persist(&cache);
    }
}

/// Remove every cached account entry for a provider, e.g. when its token
/// accounts are cleared.
pub(crate) fn forget_provider_accounts(app: &tauri::AppHandle, provider: ProviderId) {
    let snapshot = app.try_state::<Mutex<AppState>>().and_then(|state| {
        let mut state = state.lock().ok()?;
        forget_provider(&mut state.provider_account_usage, provider)
            .then(|| state.provider_account_usage.clone())
    });
    if let Some(cache) = snapshot {
        persist(&cache);
    }
}

/// Cached usage for one provider's token accounts, keyed by account id.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountUsageBridge {
    pub provider_id: String,
    pub accounts: BTreeMap<String, AccountUsageEntry>,
}

/// Cache-only read for Overview account rows. Never fetches.
#[tauri::command]
pub fn get_provider_account_usage(
    app: tauri::AppHandle,
    provider_id: String,
) -> Result<ProviderAccountUsageBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(ProviderAccountUsageBridge {
        provider_id: id.cli_name().to_string(),
        accounts: entries_for(&state.provider_account_usage, id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(json: serde_json::Value) -> super::super::ProviderUsageSnapshot {
        serde_json::from_value(json).expect("valid bridge snapshot fixture")
    }

    fn fresh(used: f64, weekly: f64) -> super::super::ProviderUsageSnapshot {
        snapshot(serde_json::json!({
            "providerId": "claude",
            "primary": { "usedPercent": used, "windowMinutes": 300, "resetsAt": "2026-05-24T05:00:00Z" },
            "secondary": { "usedPercent": weekly, "windowMinutes": 10080 },
            "updatedAt": "2026-05-24T00:00:00Z",
            "accountEmail": "work@example.com",
            "planName": "Max",
        }))
    }

    fn failed() -> super::super::ProviderUsageSnapshot {
        snapshot(serde_json::json!({
            "providerId": "claude",
            "primary": { "usedPercent": 0.0 },
            "updatedAt": "2026-05-24T01:00:00Z",
            "error": "Claude usage check failed. Will retry automatically.",
            "errorState": "unknown",
        }))
    }

    #[test]
    fn successful_fetch_is_cached_and_failed_fetch_keeps_last_good_entry() {
        let mut cache = ProviderAccountUsageCache::new();
        assert!(observe_fetch(
            &mut cache,
            ProviderId::Claude,
            "acct-1",
            &fresh(22.0, 41.0)
        ));
        let entry = &entries_for(&cache, ProviderId::Claude)["acct-1"];
        assert_eq!(entry.primary.as_ref().map(|w| w.used_percent), Some(22.0));
        assert_eq!(entry.secondary.as_ref().map(|w| w.used_percent), Some(41.0));
        assert_eq!(entry.updated_at, "2026-05-24T00:00:00Z");

        // A failed refresh must not overwrite the last good usage or its age.
        assert!(!observe_fetch(
            &mut cache,
            ProviderId::Claude,
            "acct-1",
            &failed()
        ));
        assert_eq!(&entries_for(&cache, ProviderId::Claude)["acct-1"], entry);

        // A later success replaces the entry.
        assert!(observe_fetch(
            &mut cache,
            ProviderId::Claude,
            "acct-1",
            &fresh(5.0, 9.0)
        ));
        let entry = &entries_for(&cache, ProviderId::Claude)["acct-1"];
        assert_eq!(entry.primary.as_ref().map(|w| w.used_percent), Some(5.0));
    }

    #[test]
    fn failure_with_no_prior_entry_creates_nothing() {
        let mut cache = ProviderAccountUsageCache::new();
        assert!(!observe_fetch(
            &mut cache,
            ProviderId::Claude,
            "acct-1",
            &failed()
        ));
        assert!(entries_for(&cache, ProviderId::Claude).is_empty());
    }

    #[test]
    fn only_claude_records_per_account_usage() {
        let mut cache = ProviderAccountUsageCache::new();
        assert!(!records_account_usage(ProviderId::Codex));
        assert!(!observe_fetch(
            &mut cache,
            ProviderId::Codex,
            "acct-1",
            &fresh(1.0, 2.0)
        ));
        assert!(cache.is_empty());
    }

    #[test]
    fn removing_an_account_or_provider_drops_its_entries() {
        let mut cache = ProviderAccountUsageCache::new();
        observe_fetch(&mut cache, ProviderId::Claude, "acct-1", &fresh(1.0, 2.0));
        observe_fetch(&mut cache, ProviderId::Claude, "acct-2", &fresh(3.0, 4.0));

        assert!(forget_account(&mut cache, ProviderId::Claude, "acct-1"));
        assert!(!forget_account(&mut cache, ProviderId::Claude, "acct-1"));
        assert_eq!(
            entries_for(&cache, ProviderId::Claude)
                .keys()
                .collect::<Vec<_>>(),
            vec!["acct-2"]
        );

        assert!(forget_provider(&mut cache, ProviderId::Claude));
        assert!(entries_for(&cache, ProviderId::Claude).is_empty());
        assert!(!forget_provider(&mut cache, ProviderId::Claude));
    }

    #[test]
    fn store_round_trips_claude_entries_and_rejects_other_versions() {
        let mut cache = ProviderAccountUsageCache::new();
        observe_fetch(&mut cache, ProviderId::Claude, "acct-1", &fresh(22.0, 41.0));
        let raw = encode_cache(&cache).expect("encode");
        let decoded = decode_cache(&raw).expect("decode");
        assert_eq!(decoded, cache);

        let other_version = raw.replacen("\"version\": 1", "\"version\": 2", 1);
        assert!(decode_cache(&other_version).is_none());
        assert!(decode_cache("not json").is_none());
    }

    #[test]
    fn stored_entries_hold_only_usage_fields_and_no_identity() {
        let mut cache = ProviderAccountUsageCache::new();
        observe_fetch(&mut cache, ProviderId::Claude, "acct-1", &fresh(22.0, 41.0));
        let raw = encode_cache(&cache).expect("encode");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        let entry = &value["providers"]["claude"]["acct-1"];
        let mut keys: Vec<&str> = entry
            .as_object()
            .expect("entry object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["primary", "secondary", "updatedAt"]);
        assert!(!raw.contains("work@example.com"));
        assert!(!raw.contains("Max"));
    }
}
