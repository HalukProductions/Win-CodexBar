//! Last usage read from the claude-swap or tcr account pool, so Overview can
//! show one row per pool account without starting a fetch.
//!
//! Every successful pool read (the Settings list) records each listed account.
//! An account whose read carries no live usage keeps its last good entry. A
//! read of the pool replaces the account list, so accounts that left the pool
//! drop out, and a read that started before a newer one never overwrites it.
//!
//! Entries hold the pool slot, the label Settings already shows (the email, or
//! the email with its organization when two accounts share one), and the rate
//! windows with their update time. Tokens and organization IDs are never stored.
//! Hide personal information is applied when the cache is read.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, FixedOffset};
use codexbar::providers::claude::claude_swap::{
    ClaudeSwapAccount, ClaudeSwapAccountList, ClaudeSwapAccountSource, project_accounts,
};
use codexbar::{atomic_file, secure_file};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use super::ordered_save::write_in_order;
use super::provider_account_usage::{AccountUsageEntry, AccountUsageWindow};
use crate::state::AppState;

const STORE_VERSION: u32 = 1;
const STORE_RELATIVE_PATH: &str = "claude-swap-usage-v1.json";
/// Emitted after the cache changes so an open Overview reloads its rows.
pub const CLAUDE_SWAP_USAGE_UPDATED_EVENT: &str = "claude-swap-usage-updated";
const FIVE_HOUR_MINUTES: u32 = 300;
const SEVEN_DAY_MINUTES: u32 = 10_080;

/// One pool account as Overview needs it. `label` is the unmasked label that
/// Settings shows; the read path masks it when personal information is hidden.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSwapAccountRecord {
    pub slot: u32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AccountUsageEntry>,
}

/// The cached pool: which tool it came from, and its accounts keyed by
/// `<source>:<slot>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeSwapUsageCache {
    pub source: Option<ClaudeSwapAccountSource>,
    pub accounts: BTreeMap<String, ClaudeSwapAccountRecord>,
}

/// Cache plus the counters that keep writes and reads in order. Lives in
/// `AppState` and is changed only under its lock.
#[derive(Debug, Default)]
pub struct ClaudeSwapUsageState {
    pub cache: ClaudeSwapUsageCache,
    /// Bumped on every change, so disk writes apply in change order.
    pub revision: u64,
    /// Pool reads begun so far. Each read takes the next number when it starts.
    pub reads_started: u64,
    /// Highest read number applied so far. An older read that finishes late is
    /// ignored, so it cannot bring back an account a newer read dropped.
    pub reads_applied: u64,
}

/// What one pool read saw.
#[derive(Clone, Debug)]
pub(crate) enum PoolObservation {
    /// The integration is off or has no executable, so no pool can be read.
    Inactive,
    /// A pool read succeeded. Accounts come unmasked, in the adapter's order.
    Pool {
        source: ClaudeSwapAccountSource,
        accounts: Vec<ClaudeSwapAccount>,
    },
}

/// Rows and usage for Overview, read from the cache without fetching.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSwapOverviewAccount {
    pub id: String,
    pub slot: u32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSwapOverviewAccountsBridge {
    pub accounts: Vec<ClaudeSwapOverviewAccount>,
    pub usage: BTreeMap<String, AccountUsageEntry>,
}

/// On-disk form. Holds pool slots, labels and rate windows only.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSwapUsageStore {
    version: u32,
    #[serde(default)]
    source: Option<ClaudeSwapAccountSource>,
    #[serde(default)]
    accounts: BTreeMap<String, ClaudeSwapAccountRecord>,
}

fn window(
    window: &codexbar::providers::claude::claude_swap::ClaudeSwapUsageWindowDto,
    minutes: u32,
) -> AccountUsageWindow {
    AccountUsageWindow {
        used_percent: window.used_percent.clamp(0.0, 100.0),
        window_minutes: Some(minutes),
        resets_at: window.resets_at.map(|at| at.to_rfc3339()),
    }
}

fn entry_from_windows(
    five_hour: Option<&codexbar::providers::claude::claude_swap::ClaudeSwapUsageWindowDto>,
    seven_day: Option<&codexbar::providers::claude::claude_swap::ClaudeSwapUsageWindowDto>,
    fetched_at: chrono::DateTime<chrono::Utc>,
) -> Option<AccountUsageEntry> {
    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }
    Some(AccountUsageEntry {
        primary: five_hour.map(|w| window(w, FIVE_HOUR_MINUTES)),
        secondary: seven_day.map(|w| window(w, SEVEN_DAY_MINUTES)),
        updated_at: fetched_at.to_rfc3339(),
    })
}

/// The usage this read reported for one account: the live windows when they
/// carry a fetch time, otherwise the adapter's last good usage. `None` when
/// neither has windows. Live windows without a fetch time cannot say how old
/// they are, so they are never recorded.
fn usage_from_read(account: &ClaudeSwapAccount) -> Option<AccountUsageEntry> {
    let live = account.usage_fetched_at.and_then(|fetched_at| {
        entry_from_windows(
            account.five_hour.as_ref(),
            account.seven_day.as_ref(),
            fetched_at,
        )
    });
    live.or_else(|| {
        let historical = account.historical_usage.as_ref()?;
        entry_from_windows(
            historical.five_hour.as_ref(),
            historical.seven_day.as_ref(),
            historical.fetched_at,
        )
    })
}

/// Parse an entry's update time for comparison. An unreadable time sorts first.
fn updated_key(entry: &AccountUsageEntry) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(&entry.updated_at).ok()
}

/// Keep whichever entry was fetched later, so an adapter-side cache that is
/// older than an earlier read never replaces newer data.
fn newer_entry(
    fresh: Option<AccountUsageEntry>,
    kept: Option<AccountUsageEntry>,
) -> Option<AccountUsageEntry> {
    match (fresh, kept) {
        (Some(fresh), Some(kept)) => {
            if updated_key(&fresh) >= updated_key(&kept) {
                Some(fresh)
            } else {
                Some(kept)
            }
        }
        (fresh, kept) => fresh.or(kept),
    }
}

/// Build the cache entries for one pool read. Accounts absent from the read are
/// not carried over, which is how removed accounts leave the cache.
fn next_accounts(
    accounts: &[ClaudeSwapAccount],
    previous: &BTreeMap<String, ClaudeSwapAccountRecord>,
) -> BTreeMap<String, ClaudeSwapAccountRecord> {
    accounts
        .iter()
        .map(|account| {
            let kept = previous.get(&account.id).and_then(|r| r.usage.clone());
            let record = ClaudeSwapAccountRecord {
                slot: account.slot,
                label: account.label.clone(),
                usage: newer_entry(usage_from_read(account), kept),
            };
            (account.id.clone(), record)
        })
        .collect()
}

/// Start a pool read. Call under the state lock before the read runs.
pub(crate) fn begin_read(usage: &mut ClaudeSwapUsageState) -> u64 {
    usage.reads_started += 1;
    usage.reads_started
}

/// Apply one read's observation. Returns whether the cache changed. A read
/// numbered at or below the last applied read is ignored.
pub(crate) fn apply_observation(
    usage: &mut ClaudeSwapUsageState,
    read: u64,
    observation: &PoolObservation,
) -> bool {
    if read <= usage.reads_applied {
        return false;
    }
    usage.reads_applied = read;
    match observation {
        PoolObservation::Inactive => {
            if usage.cache == ClaudeSwapUsageCache::default() {
                return false;
            }
            usage.cache = ClaudeSwapUsageCache::default();
            true
        }
        PoolObservation::Pool { source, accounts } => {
            let same_source = usage.cache.source == Some(*source);
            let previous = if same_source {
                usage.cache.accounts.clone()
            } else {
                BTreeMap::new()
            };
            let next = next_accounts(accounts, &previous);
            if same_source && next == previous {
                return false;
            }
            usage.cache = ClaudeSwapUsageCache {
                source: Some(*source),
                accounts: next,
            };
            true
        }
    }
}

/// Overview rows for the pool the integration is configured to read. A cache
/// from another source, or from an integration that is now off, shows nothing.
pub(crate) fn overview_accounts(
    cache: &ClaudeSwapUsageCache,
    active_source: Option<ClaudeSwapAccountSource>,
) -> ClaudeSwapOverviewAccountsBridge {
    let matches_active = active_source.is_some_and(|source| cache.source == Some(source));
    if !matches_active {
        return ClaudeSwapOverviewAccountsBridge {
            accounts: Vec::new(),
            usage: BTreeMap::new(),
        };
    }
    let mut accounts: Vec<ClaudeSwapOverviewAccount> = cache
        .accounts
        .iter()
        .map(|(id, record)| ClaudeSwapOverviewAccount {
            id: id.clone(),
            slot: record.slot,
            label: record.label.clone(),
        })
        .collect();
    accounts.sort_by_key(|account| account.slot);
    let usage = cache
        .accounts
        .iter()
        .filter_map(|(id, record)| record.usage.clone().map(|entry| (id.clone(), entry)))
        .collect();
    ClaudeSwapOverviewAccountsBridge { accounts, usage }
}

fn encode_cache(cache: &ClaudeSwapUsageCache) -> serde_json::Result<String> {
    serde_json::to_string_pretty(&ClaudeSwapUsageStore {
        version: STORE_VERSION,
        source: cache.source,
        accounts: cache.accounts.clone(),
    })
}

/// An unreadable or unsupported store is ignored so it never blocks startup.
fn decode_cache(raw: &str) -> Option<ClaudeSwapUsageCache> {
    let store: ClaudeSwapUsageStore = serde_json::from_str(raw).ok()?;
    (store.version == STORE_VERSION).then_some(ClaudeSwapUsageCache {
        source: store.source,
        accounts: store.accounts,
    })
}

fn cache_store_path() -> Option<PathBuf> {
    codexbar::logging::config_root().map(|root| root.join(STORE_RELATIVE_PATH))
}

/// Last-known pool usage, restored at launch so Overview rows can show it
/// before the Settings list is opened again.
pub(crate) fn load_claude_swap_usage() -> ClaudeSwapUsageCache {
    let Some(path) = cache_store_path() else {
        return ClaudeSwapUsageCache::default();
    };
    match secure_file::read_string(&path) {
        Ok(raw) => decode_cache(&raw).unwrap_or_else(|| {
            tracing::warn!("ignoring unreadable claude-swap usage cache");
            ClaudeSwapUsageCache::default()
        }),
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!("failed to read claude-swap usage cache: {error}");
            }
            ClaudeSwapUsageCache::default()
        }
    }
}

fn save_claude_swap_usage(cache: &ClaudeSwapUsageCache) -> io::Result<()> {
    let Some(path) = cache_store_path() else {
        return Ok(());
    };
    let raw = encode_cache(cache).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Saves are serialized by SAVE_GATE, so one fixed staging name is never
    // written by two saves at once.
    let staged = path.with_extension("json.tmp");
    secure_file::write_string(&staged, &raw)?;
    atomic_file::replace_staged(&staged, &path)
}

/// Highest revision accepted for a disk write, held for the whole write.
static SAVE_GATE: Mutex<u64> = Mutex::new(0);

/// A cache copy taken under the state lock, tagged with its revision.
pub(crate) struct ClaudeSwapCacheSnapshot {
    revision: u64,
    cache: ClaudeSwapUsageCache,
}

/// Call under the state lock right after the cache changes.
fn claude_swap_snapshot_after_change(usage: &mut ClaudeSwapUsageState) -> ClaudeSwapCacheSnapshot {
    usage.revision += 1;
    ClaudeSwapCacheSnapshot {
        revision: usage.revision,
        cache: usage.cache.clone(),
    }
}

/// Write a snapshot taken under the state lock. Runs after the lock is released.
fn persist_claude_swap_usage(snapshot: &ClaudeSwapCacheSnapshot) {
    let result = write_in_order(&SAVE_GATE, snapshot.revision, || {
        save_claude_swap_usage(&snapshot.cache)
    });
    if let Err(error) = result {
        tracing::warn!("failed to persist claude-swap usage: {error}");
    }
}

/// Start a pool read on the shared state. Returns `None` if the state is
/// unavailable, in which case the read is not recorded.
pub(crate) fn begin_pool_read(app: &tauri::AppHandle) -> Option<u64> {
    let state = app.try_state::<Mutex<AppState>>()?;
    let mut state = state.lock().ok()?;
    Some(begin_read(&mut state.claude_swap_usage))
}

/// Record one pool read's observation, persist the change, and tell open
/// windows to reload their Overview rows.
pub(crate) fn record_observation(
    app: &tauri::AppHandle,
    read: Option<u64>,
    observation: &PoolObservation,
) {
    let Some(read) = read else {
        return;
    };
    let snapshot = {
        let Some(state) = app.try_state::<Mutex<AppState>>() else {
            return;
        };
        let Ok(mut state) = state.lock() else {
            return;
        };
        if !apply_observation(&mut state.claude_swap_usage, read, observation) {
            return;
        }
        claude_swap_snapshot_after_change(&mut state.claude_swap_usage)
    };
    persist_claude_swap_usage(&snapshot);
    let _ = app.emit(CLAUDE_SWAP_USAGE_UPDATED_EVENT, ());
}

/// Build the observation for a successful list read. The projection is taken
/// unmasked so the stored label survives a later change to personal-info hiding.
pub(crate) fn observe_list(
    list: &ClaudeSwapAccountList,
    source: ClaudeSwapAccountSource,
) -> PoolObservation {
    PoolObservation::Pool {
        source,
        accounts: project_accounts(list, source, false),
    }
}

/// Cache-only read for Overview account rows. Never fetches.
#[tauri::command]
pub fn get_claude_swap_overview_accounts(
    app: tauri::AppHandle,
) -> Result<ClaudeSwapOverviewAccountsBridge, String> {
    let settings = codexbar::settings::Settings::load();
    let active_source = (settings.claude_swap_enabled()
        && !settings.claude_swap_executable_path().trim().is_empty())
    .then(|| settings.claude_swap_source());
    let state = app.state::<Mutex<AppState>>();
    let state = state.lock().map_err(|e| e.to_string())?;
    Ok(overview_accounts(
        &state.claude_swap_usage.cache,
        active_source,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codexbar::providers::claude::claude_swap::parse_account_list;
    use serde_json::json;

    fn pool(source: ClaudeSwapAccountSource, rows: serde_json::Value) -> PoolObservation {
        // The list's active slot must agree with the rows' `active` flags.
        let active = rows
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["active"] == json!(true)))
            .map(|row| row["number"].clone())
            .unwrap_or(serde_json::Value::Null);
        let raw = json!({
            "schemaVersion": 1,
            "activeAccountNumber": active,
            "accounts": rows,
        });
        let list = parse_account_list(&raw.to_string()).unwrap();
        observe_list(&list, source)
    }

    fn ok_row(number: u32, five_hour: f64, fetched_at: &str) -> serde_json::Value {
        json!({
            "number": number,
            "email": format!("user{number}@example.com"),
            "organizationName": "Org",
            "active": number == 1,
            "usageStatus": "ok",
            "usage": {
                "fiveHour": { "pct": five_hour, "resetsAt": "2026-09-12T01:00:00Z" },
                "sevenDay": { "pct": 18.0 }
            },
            "usageFetchedAt": fetched_at
        })
    }

    fn expired_row(number: u32) -> serde_json::Value {
        json!({
            "number": number,
            "email": format!("user{number}@example.com"),
            "active": false,
            "usageStatus": "token_expired"
        })
    }

    /// Apply one read the way the command does: begin, then record.
    fn apply(usage: &mut ClaudeSwapUsageState, observation: &PoolObservation) -> bool {
        let read = begin_read(usage);
        apply_observation(usage, read, observation)
    }

    #[test]
    fn records_live_usage_and_label_for_each_listed_account() {
        let mut usage = ClaudeSwapUsageState::default();
        let observation = pool(
            ClaudeSwapAccountSource::ClaudeSwap,
            json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z"), expired_row(2)]),
        );
        assert!(apply(&mut usage, &observation));

        let cache = &usage.cache;
        assert_eq!(cache.source, Some(ClaudeSwapAccountSource::ClaudeSwap));
        assert_eq!(cache.accounts.len(), 2);
        let first = &cache.accounts["claude-swap:1"];
        assert_eq!(first.slot, 1);
        assert_eq!(first.label, "user1@example.com");
        let entry = first.usage.as_ref().unwrap();
        let primary = entry.primary.as_ref().unwrap();
        assert_eq!(primary.used_percent, 40.0);
        assert_eq!(primary.window_minutes, Some(FIVE_HOUR_MINUTES));
        assert_eq!(
            primary.resets_at.as_deref(),
            Some("2026-09-12T01:00:00+00:00")
        );
        assert_eq!(
            entry.secondary.as_ref().unwrap().window_minutes,
            Some(SEVEN_DAY_MINUTES)
        );
        assert_eq!(entry.updated_at, "2026-09-12T00:30:00+00:00");
        assert!(cache.accounts["claude-swap:2"].usage.is_none());
    }

    #[test]
    fn account_without_live_usage_keeps_its_last_good_entry() {
        let mut usage = ClaudeSwapUsageState::default();
        let first = pool(
            ClaudeSwapAccountSource::ClaudeSwap,
            json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
        );
        apply(&mut usage, &first);

        let failed = pool(ClaudeSwapAccountSource::ClaudeSwap, json!([expired_row(1)]));
        assert!(!apply(&mut usage, &failed));
        let kept = usage.cache.accounts["claude-swap:1"].usage.clone().unwrap();
        assert_eq!(kept.primary.unwrap().used_percent, 40.0);
        assert_eq!(kept.updated_at, "2026-09-12T00:30:00+00:00");
    }

    #[test]
    fn account_without_live_usage_records_the_adapter_last_good_usage() {
        let mut row = expired_row(1);
        row["lastGoodUsage"] = json!({
            "fiveHour": { "pct": 44.0 },
            "sevenDay": { "pct": 19.0 }
        });
        row["lastGoodFetchedAt"] = json!("2026-09-12T00:45:00Z");
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(ClaudeSwapAccountSource::ClaudeSwap, json!([row])),
        );

        let entry = usage.cache.accounts["claude-swap:1"].usage.clone().unwrap();
        assert_eq!(entry.primary.unwrap().used_percent, 44.0);
        assert_eq!(entry.secondary.unwrap().used_percent, 19.0);
        assert_eq!(entry.updated_at, "2026-09-12T00:45:00+00:00");
    }

    #[test]
    fn older_adapter_cache_never_replaces_newer_recorded_usage() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T01:00:00.000Z")]),
            ),
        );
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 5.0, "2026-09-12T00:00:00.000Z")]),
            ),
        );
        let entry = usage.cache.accounts["claude-swap:1"].usage.clone().unwrap();
        assert_eq!(entry.primary.unwrap().used_percent, 40.0);
    }

    #[test]
    fn account_that_leaves_the_pool_drops_its_row_and_usage() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([
                    ok_row(1, 40.0, "2026-09-12T00:30:00.000Z"),
                    ok_row(2, 10.0, "2026-09-12T00:30:00.000Z")
                ]),
            ),
        );
        assert!(apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        ));
        assert!(!usage.cache.accounts.contains_key("claude-swap:2"));
        let bridge = overview_accounts(&usage.cache, Some(ClaudeSwapAccountSource::ClaudeSwap));
        assert_eq!(bridge.accounts.len(), 1);
        assert!(!bridge.usage.contains_key("claude-swap:2"));
    }

    #[test]
    fn stale_read_started_earlier_cannot_bring_back_a_removed_account() {
        let mut usage = ClaudeSwapUsageState::default();
        let older = begin_read(&mut usage);
        let newer = begin_read(&mut usage);
        let without_second = pool(
            ClaudeSwapAccountSource::ClaudeSwap,
            json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
        );
        assert!(apply_observation(&mut usage, newer, &without_second));

        let with_second = pool(
            ClaudeSwapAccountSource::ClaudeSwap,
            json!([
                ok_row(1, 40.0, "2026-09-12T00:30:00.000Z"),
                ok_row(2, 10.0, "2026-09-12T00:30:00.000Z")
            ]),
        );
        assert!(!apply_observation(&mut usage, older, &with_second));
        assert!(!usage.cache.accounts.contains_key("claude-swap:2"));
    }

    #[test]
    fn inactive_integration_clears_the_cache_once() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        );
        assert!(apply(&mut usage, &PoolObservation::Inactive));
        assert_eq!(usage.cache, ClaudeSwapUsageCache::default());
        assert!(!apply(&mut usage, &PoolObservation::Inactive));
    }

    #[test]
    fn switching_source_replaces_the_other_pool() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        );
        assert!(apply(
            &mut usage,
            &pool(ClaudeSwapAccountSource::Tcr, json!([expired_row(7)])),
        ));
        assert_eq!(usage.cache.source, Some(ClaudeSwapAccountSource::Tcr));
        assert_eq!(
            usage.cache.accounts.keys().cloned().collect::<Vec<_>>(),
            vec!["tcr:7".to_string()]
        );
    }

    #[test]
    fn overview_shows_nothing_for_another_or_disabled_source() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        );
        let other = overview_accounts(&usage.cache, Some(ClaudeSwapAccountSource::Tcr));
        assert!(other.accounts.is_empty() && other.usage.is_empty());
        let disabled = overview_accounts(&usage.cache, None);
        assert!(disabled.accounts.is_empty() && disabled.usage.is_empty());
        let active = overview_accounts(&usage.cache, Some(ClaudeSwapAccountSource::ClaudeSwap));
        assert_eq!(active.accounts[0].id, "claude-swap:1");
        assert!(active.usage.contains_key("claude-swap:1"));
    }

    #[test]
    fn store_round_trips_and_rejects_other_versions() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::Tcr,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z"), expired_row(2)]),
            ),
        );
        let raw = encode_cache(&usage.cache).unwrap();
        assert_eq!(decode_cache(&raw), Some(usage.cache.clone()));

        let other_version = raw.replace("\"version\": 1", "\"version\": 2");
        assert_eq!(decode_cache(&other_version), None);
    }

    #[test]
    fn stored_entries_hold_only_slot_label_and_windows() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        );
        let value: serde_json::Value =
            serde_json::from_str(&encode_cache(&usage.cache).unwrap()).unwrap();
        let record = &value["accounts"]["claude-swap:1"];
        let mut keys: Vec<&str> = record
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["label", "slot", "usage"]);
        assert_eq!(record["label"], "user1@example.com");
        assert!(!record.to_string().contains("organization"));
        assert!(!record.to_string().contains("Org"));
    }

    #[test]
    fn revisions_rise_with_each_change() {
        let mut usage = ClaudeSwapUsageState::default();
        apply(
            &mut usage,
            &pool(
                ClaudeSwapAccountSource::ClaudeSwap,
                json!([ok_row(1, 40.0, "2026-09-12T00:30:00.000Z")]),
            ),
        );
        let first = claude_swap_snapshot_after_change(&mut usage).revision;
        apply(&mut usage, &PoolObservation::Inactive);
        let second = claude_swap_snapshot_after_change(&mut usage).revision;
        assert!(second > first);
    }
}
