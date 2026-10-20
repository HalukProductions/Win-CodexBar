//! Gate in front of the Claude OAuth usage endpoint (nesszer/Win-CodexBar#775).
//!
//! * After a 429 no request goes out for that account until
//!   `max(Retry-After.min(60 min), (5 min * 2^(n-1)).min(30 min))` has passed.
//!   `n` counts consecutive 429s of that account and resets only on a success
//!   of the same account, never because the block expired or because another
//!   account succeeded. A 429 blocks only the account that received it, so one
//!   saved account's rate limit never hides another account's usage (#787).
//!   Concurrent 429s while a block is active do not escalate.
//! * Accounts are keyed by `account_key`: the hashed saved-account id when it is
//!   known, otherwise the token fingerprint. A renewed access token keeps the
//!   same account key, so a token refresh does not lift the block.
//! * The state lives in one small JSON file under `%LOCALAPPDATA%\CodexBar\`,
//!   so every CodexBar process of the user (tray app and one-off
//!   `codexbar usage` runs) shares it. An in-memory copy keeps the gate working
//!   when the file cannot be read or written.
//! * The last successful response body of each account is kept in the same file
//!   and served again for 3 minutes to that same account. The response holds
//!   only utilization percentages, reset timestamps and credit amounts (see
//!   `OAuthUsageResponse` in `oauth/mod.rs`), no token or account identity.
//!   Saved limit-reset grants are stored as counts and dates only; their
//!   redemption handles are removed first (`reset_credits::redact_usage_body`).
//!   Precedent: the CLI probe screen is persisted across processes the same way.
//!   The token itself is never written; keys are truncated SHA-256 digests.
//! * Blocks and cached bodies are bounded to `MAX_CACHED_SIGN_INS` accounts.
//! * Files written before per-account state still load. A single-slot
//!   `last_success` is migrated into the map on the next read. A global block
//!   from an older build cannot be attributed to one account, so it is dropped.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

const BASE_BACKOFF_MS: i64 = 5 * 60 * 1000;
/// Cap of the exponential ramp.
const MAX_BACKOFF_MS: i64 = 30 * 60 * 1000;
/// Cap of an honored `Retry-After`; also the longest wait ever reported.
const MAX_RETRY_AFTER_MS: i64 = 60 * 60 * 1000;
const MIN_POLL_INTERVAL_MS: i64 = 3 * 60 * 1000;
/// Accounts whose block or last good body is kept at once. Saved accounts are
/// few; the bound only stops rotated-away sign-ins from growing the file forever.
const MAX_CACHED_SIGN_INS: usize = 8;
const FILE_NAME: &str = "claude-oauth-usage-gate.json";

/// What the caller should do before hitting the endpoint.
#[derive(Debug, PartialEq)]
pub(super) enum Decision {
    Blocked(Duration),
    /// Raw body of a recent successful response, to be parsed by the caller.
    Cached(String),
    Proceed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct CachedBody {
    at_ms: i64,
    /// The account key this body belongs to (never a token).
    fp: String,
    body: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Block {
    /// Requests for this account are refused until this epoch millisecond.
    until_ms: i64,
    /// Consecutive 429s of this account; resets when this account succeeds.
    consecutive: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Persisted {
    /// 429 block and streak per account key, bounded to `MAX_CACHED_SIGN_INS`.
    #[serde(default)]
    blocks: BTreeMap<String, Block>,
    /// Last good body per account key, bounded to `MAX_CACHED_SIGN_INS`.
    #[serde(default)]
    last_successes: BTreeMap<String, CachedBody>,
    /// Single-slot cache written before `last_successes`. Read only so old files
    /// load; it is migrated into the map and never written back.
    #[serde(default, skip_serializing)]
    last_success: Option<CachedBody>,
}

impl Persisted {
    /// Fold a legacy single slot into the map, then enforce the bounds.
    fn normalize(&mut self) {
        if let Some(legacy) = self.last_success.take() {
            let newer = self
                .last_successes
                .get(&legacy.fp)
                .is_none_or(|kept| kept.at_ms < legacy.at_ms);
            if newer {
                self.last_successes.insert(legacy.fp.clone(), legacy);
            }
        }
        trim_to_bound(&mut self.last_successes, |cached| cached.at_ms);
        trim_to_bound(&mut self.blocks, |block| block.until_ms);
    }
}

/// Evict the entries with the smallest timestamp until the map is within its bound.
fn trim_to_bound<V>(map: &mut BTreeMap<String, V>, time_ms: impl Fn(&V) -> i64) {
    while map.len() > MAX_CACHED_SIGN_INS {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| time_ms(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

pub(super) struct UsageGate {
    path: Option<PathBuf>,
    state: Persisted,
}

/// First 16 hex characters of the SHA-256 of `text`.
fn digest16(text: &str) -> String {
    let mut hex = crate::core::sha256_hex(text.as_bytes());
    hex.truncate(16);
    hex
}

/// First 16 hex characters of the token's SHA-256. Never the token.
pub(super) fn token_fingerprint(token: &str) -> String {
    digest16(token)
}

/// Gate key for one Claude account. A known account is keyed by a digest of
/// its id, so renewing its token keeps the same key and its block. A sign-in
/// whose account is not known falls back to its token fingerprint.
pub(super) fn account_key(account_id: Option<&str>, access_token: &str) -> String {
    match account_id {
        Some(id) => format!("acct-{}", digest16(id)),
        None => token_fingerprint(access_token),
    }
}

pub(super) fn default_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("CodexBar").join(FILE_NAME))
}

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Anthropic often returns `Retry-After: 0` or `1` on the usage endpoint.
/// Honoring that literally re-hits 429 on the next poll and, after a
/// last-good miss, the tray maps the generic OAuth error to sign-in; the
/// exponential ramp is therefore the floor.
fn backoff_ms(retry_after: Duration, consecutive: u32) -> i64 {
    let exponential = BASE_BACKOFF_MS.saturating_mul(1i64 << consecutive.saturating_sub(1).min(10));
    let retry_after = i64::try_from(retry_after.as_millis()).unwrap_or(i64::MAX);
    retry_after
        .min(MAX_RETRY_AFTER_MS)
        .max(exponential.min(MAX_BACKOFF_MS))
}

impl UsageGate {
    pub(super) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            state: Persisted::default(),
        }
    }

    /// Replace memory with the shared file, which any process may have updated.
    fn refresh(&mut self) {
        let Some(path) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        if let Ok(mut disk) = serde_json::from_str::<Persisted>(&text) {
            disk.normalize();
            self.state = disk;
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let Ok(json) = serde_json::to_vec(&self.state) else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::debug!("Claude OAuth usage gate directory not created: {err}");
            return;
        }
        if let Err(err) = crate::atomic_file::write_atomic(path, &json) {
            tracing::debug!("Claude OAuth usage gate not persisted: {err}");
        }
    }

    /// Time left on this account's block, clamped so a skewed or hand-edited
    /// timestamp cannot lock requests out for longer than any real backoff.
    fn remaining(&self, now_ms: i64, key: &str) -> Option<Duration> {
        let until = self.state.blocks.get(key)?.until_ms;
        let left = until.saturating_sub(now_ms);
        (left > 0).then(|| Duration::from_millis(left.min(MAX_RETRY_AFTER_MS) as u64))
    }

    pub(super) fn check(&mut self, now_ms: i64, key: &str) -> Decision {
        self.refresh();
        if let Some(remaining) = self.remaining(now_ms, key) {
            return Decision::Blocked(remaining);
        }
        if let Some(last) = self.state.last_successes.get(key)
            && now_ms >= last.at_ms
            && now_ms - last.at_ms < MIN_POLL_INTERVAL_MS
        {
            return Decision::Cached(last.body.clone());
        }
        Decision::Proceed
    }

    /// Record a 429 for `key` and return how long that account is blocked.
    pub(super) fn record_rate_limit(
        &mut self,
        now_ms: i64,
        key: &str,
        retry_after: Duration,
    ) -> Duration {
        self.refresh();
        if let Some(remaining) = self.remaining(now_ms, key) {
            // Another request already escalated; do not count this 429 again.
            return remaining;
        }
        let block = self.state.blocks.entry(key.to_string()).or_default();
        block.consecutive = block.consecutive.saturating_add(1);
        let backoff = backoff_ms(retry_after, block.consecutive);
        block.until_ms = now_ms.saturating_add(backoff);
        trim_to_bound(&mut self.state.blocks, |block| block.until_ms);
        self.persist();
        Duration::from_millis(backoff as u64)
    }

    /// Remember a successful response body for `key`. The account's own 429
    /// streak ends with its own success. The refresh is needed so this write
    /// does not overwrite a block another process just recorded or another
    /// account's cached body.
    pub(super) fn record_success(&mut self, now_ms: i64, key: &str, body: String) {
        self.refresh();
        self.state.blocks.remove(key);
        self.state.last_successes.insert(
            key.to_string(),
            CachedBody {
                at_ms: now_ms,
                fp: key.to_string(),
                body,
            },
        );
        trim_to_bound(&mut self.state.last_successes, |cached| cached.at_ms);
        self.persist();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60 * 1000;
    const NOW: i64 = 1_000_000_000_000;

    fn gate(dir: &tempfile::TempDir) -> UsageGate {
        UsageGate::new(Some(dir.path().join(FILE_NAME)))
    }

    fn blocked_mins(d: Decision) -> u64 {
        match d {
            Decision::Blocked(d) => d.as_secs().div_ceil(60),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn consecutive_429s_escalate_even_after_the_block_expired() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        let mut now = NOW;
        for expected in [5, 10, 20, 30, 30] {
            let wait = g.record_rate_limit(now, "fp", Duration::from_secs(1));
            assert_eq!(wait.as_secs(), expected * 60);
            now += i64::try_from(expected).unwrap() * MIN + 1;
            assert_eq!(g.check(now, "fp"), Decision::Proceed);
        }
    }

    #[test]
    fn retry_after_wins_when_longer_and_is_capped_at_an_hour() {
        let wait = |secs| {
            gate(&tempfile::tempdir().unwrap())
                .record_rate_limit(NOW, "fp", Duration::from_secs(secs))
                .as_secs()
        };
        assert_eq!(wait(12 * 60), 720);
        assert_eq!(wait(45 * 60), 45 * 60);
        assert_eq!(wait(3 * 3600), 3600);
    }

    #[test]
    fn concurrent_429_during_an_active_block_does_not_escalate() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        g.record_rate_limit(NOW, "fp", Duration::ZERO);
        let again = g.record_rate_limit(NOW + MIN, "fp", Duration::ZERO);
        assert_eq!(again.as_secs(), 4 * 60);
        // Block expires; the next 429 is the second of the streak.
        assert_eq!(
            g.record_rate_limit(NOW + 6 * MIN, "fp", Duration::ZERO)
                .as_secs(),
            600
        );
    }

    #[test]
    fn success_of_the_blocked_account_resets_the_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        g.record_rate_limit(NOW, "fp", Duration::ZERO);
        g.record_rate_limit(NOW + 6 * MIN, "fp", Duration::ZERO);
        g.record_success(NOW + 20 * MIN, "fp", "ok".into());
        assert_eq!(
            g.record_rate_limit(NOW + 30 * MIN, "fp", Duration::ZERO)
                .as_secs(),
            300
        );
    }

    #[test]
    fn success_of_another_account_keeps_the_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        g.record_rate_limit(NOW, "fp-a", Duration::ZERO);
        g.record_success(NOW + 6 * MIN, "fp-b", "ok".into());
        assert_eq!(
            g.record_rate_limit(NOW + 7 * MIN, "fp-a", Duration::ZERO)
                .as_secs(),
            600
        );
    }

    #[test]
    fn block_is_shared_through_the_file_and_stays_with_its_account() {
        let dir = tempfile::tempdir().unwrap();
        gate(&dir).record_rate_limit(NOW, "acct-a", Duration::ZERO);
        let mut other = gate(&dir);
        assert_eq!(blocked_mins(other.check(NOW + MIN, "acct-a")), 4);
        assert_eq!(other.check(NOW + 5 * MIN + 1, "acct-a"), Decision::Proceed);
        // The count survives in the file, so the next 429 doubles.
        assert_eq!(
            other
                .record_rate_limit(NOW + 6 * MIN, "acct-a", Duration::ZERO)
                .as_secs(),
            600
        );
    }

    #[test]
    fn a_429_blocks_only_its_own_account() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        g.record_rate_limit(NOW, "acct-a", Duration::ZERO);
        assert_eq!(g.check(NOW + MIN, "acct-b"), Decision::Proceed);
        // Account B's own 429 starts B's streak; account A keeps its block.
        assert_eq!(
            g.record_rate_limit(NOW + MIN, "acct-b", Duration::ZERO)
                .as_secs(),
            5 * 60
        );
        assert_eq!(blocked_mins(g.check(NOW + MIN, "acct-a")), 4);
    }

    #[test]
    fn account_key_is_stable_across_token_renewal_and_hides_the_id() {
        let before = account_key(Some("uuid-1:org-1"), "token-old");
        let renewed = account_key(Some("uuid-1:org-1"), "token-new");
        assert_eq!(before, renewed);
        assert_ne!(before, account_key(Some("uuid-2:org-1"), "token-old"));
        assert!(!before.contains("uuid-1"));
        // Without a known account the key is the token's fingerprint.
        assert_eq!(
            account_key(None, "token-old"),
            token_fingerprint("token-old")
        );
        assert_ne!(
            account_key(None, "token-old"),
            account_key(None, "token-new")
        );
    }

    #[test]
    fn block_survives_token_renewal_of_the_same_account() {
        let dir = tempfile::tempdir().unwrap();
        let key = account_key(Some("a:org"), "old-token");
        gate(&dir).record_rate_limit(NOW, &key, Duration::ZERO);
        let renewed = account_key(Some("a:org"), "new-token");
        assert_eq!(blocked_mins(gate(&dir).check(NOW + MIN, &renewed)), 4);
    }

    #[test]
    fn skewed_block_timestamp_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "fp".to_string(),
            Block {
                until_ms: NOW + 1000 * 60 * MIN,
                consecutive: 1,
            },
        );
        let state = Persisted {
            blocks,
            ..Persisted::default()
        };
        std::fs::write(
            dir.path().join(FILE_NAME),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert_eq!(blocked_mins(gate(&dir).check(NOW, "fp")), 60);
    }

    #[test]
    fn legacy_global_block_is_not_attributed_to_any_account() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            format!(
                r#"{{"blocked_until_ms":{},"consecutive":1,"blocked_fp":"fp"}}"#,
                NOW + 10 * MIN
            ),
        )
        .unwrap();
        assert_eq!(gate(&dir).check(NOW, "fp"), Decision::Proceed);
    }

    #[test]
    fn memory_copy_works_without_a_usable_file() {
        let dir = tempfile::tempdir().unwrap();
        // The path's parent is a regular file, so reads and writes fail.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let mut g = UsageGate::new(Some(blocker.join(FILE_NAME)));
        g.record_rate_limit(NOW, "fp", Duration::ZERO);
        assert_eq!(blocked_mins(g.check(NOW + MIN, "fp")), 4);
        let mut none = UsageGate::new(None);
        none.record_rate_limit(NOW, "fp", Duration::ZERO);
        assert_eq!(blocked_mins(none.check(NOW, "fp")), 5);
    }

    #[test]
    fn missing_directory_is_created_and_state_is_shared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CodexBar").join("nested").join(FILE_NAME);
        UsageGate::new(Some(path.clone())).record_rate_limit(NOW, "fp", Duration::ZERO);
        assert!(path.is_file());
        let mut other = UsageGate::new(Some(path));
        assert_eq!(blocked_mins(other.check(NOW + MIN, "fp")), 4);
    }

    #[test]
    fn corrupt_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        let mut g = gate(&dir);
        assert_eq!(g.check(NOW, "fp"), Decision::Proceed);
        g.record_rate_limit(NOW, "fp", Duration::ZERO);
        assert_eq!(blocked_mins(g.check(NOW, "fp")), 5);
    }

    #[test]
    fn successful_polls_are_throttled_per_account_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        gate(&dir).record_success(NOW, "fp-a", "usage-a".into());
        let mut other = gate(&dir);
        assert_eq!(
            other.check(NOW + 2 * MIN, "fp-a"),
            Decision::Cached("usage-a".into())
        );
        assert_eq!(other.check(NOW + 2 * MIN, "fp-b"), Decision::Proceed);
        assert_eq!(other.check(NOW + 3 * MIN, "fp-a"), Decision::Proceed);
    }

    #[test]
    fn file_holds_no_token_and_only_a_truncated_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        let full = crate::core::sha256_hex(b"secret-token");
        let fp = token_fingerprint("secret-token");
        assert_eq!(fp.len(), 16);
        assert!(full.starts_with(&fp));
        g.record_rate_limit(NOW, &fp, Duration::ZERO);
        g.record_success(NOW, &fp, r#"{"fiveHour":{"utilization":12.0}}"#.into());
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert!(!text.contains("secret-token") && !text.contains(&full));
        assert!(text.contains(&fp));
    }

    #[test]
    fn two_saved_accounts_keep_their_cached_bodies_within_the_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        g.record_success(NOW, "fp-a", "usage-a".into());
        g.record_success(NOW + MIN, "fp-b", "usage-b".into());
        let mut other = gate(&dir);
        assert_eq!(
            other.check(NOW + 2 * MIN, "fp-a"),
            Decision::Cached("usage-a".into())
        );
        assert_eq!(
            other.check(NOW + 2 * MIN, "fp-b"),
            Decision::Cached("usage-b".into())
        );
        // Each slot still expires on its own clock.
        assert_eq!(other.check(NOW + 3 * MIN + 1, "fp-a"), Decision::Proceed);
    }

    #[test]
    fn success_cache_is_bounded_and_evicts_the_oldest_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = gate(&dir);
        for n in 0..=MAX_CACHED_SIGN_INS {
            let at = NOW + i64::try_from(n).unwrap() * 1000;
            g.record_success(at, &format!("fp-{n}"), format!("usage-{n}"));
        }
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        let file: Persisted = serde_json::from_str(&text).unwrap();
        assert_eq!(file.last_successes.len(), MAX_CACHED_SIGN_INS);
        let now = NOW + 60 * 1000;
        assert_eq!(g.check(now, "fp-0"), Decision::Proceed);
        assert_eq!(
            g.check(now, &format!("fp-{MAX_CACHED_SIGN_INS}")),
            Decision::Cached(format!("usage-{MAX_CACHED_SIGN_INS}"))
        );
    }

    #[test]
    fn legacy_single_slot_file_loads_and_is_served_then_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            format!(
                r#"{{"blocked_until_ms":0,"consecutive":0,"last_success":{{"at_ms":{NOW},"fp":"fp-a","body":"usage-a"}}}}"#
            ),
        )
        .unwrap();
        let mut g = gate(&dir);
        assert_eq!(
            g.check(NOW + MIN, "fp-a"),
            Decision::Cached("usage-a".into())
        );
        assert_eq!(g.check(NOW + MIN, "fp-b"), Decision::Proceed);
        g.record_success(NOW + 2 * MIN, "fp-b", "usage-b".into());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"last_success\""));
        let mut reread = gate(&dir);
        assert_eq!(
            reread.check(NOW + 2 * MIN, "fp-a"),
            Decision::Cached("usage-a".into())
        );
        assert_eq!(
            reread.check(NOW + 2 * MIN, "fp-b"),
            Decision::Cached("usage-b".into())
        );
    }

    #[test]
    fn legacy_null_or_missing_slot_and_older_files_load_without_error() {
        for body in [
            r#"{"blocked_until_ms":0,"consecutive":0,"last_success":null}"#,
            r#"{"blocked_until_ms":0,"consecutive":2}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(FILE_NAME), body).unwrap();
            assert_eq!(gate(&dir).check(NOW, "fp-a"), Decision::Proceed);
        }
    }
}
