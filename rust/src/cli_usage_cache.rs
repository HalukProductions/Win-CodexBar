//! Usage cache the desktop app shares with the `codexbar` CLI (upstream
//! 0.77.0 #4554 `CLIUsageCache`).
//!
//! Opt-in through `Settings::share_usage_with_cli`. The app writes the rate
//! windows of its default-account snapshots to `cli-usage-cache.json` in the
//! config directory. `codexbar usage` reads the file only after a fetch failed
//! because a credential could not be read *and* the process saw a Windows
//! DPAPI decrypt failure. That is the Windows form of upstream's locked login
//! keychain: an OpenSSH key-auth logon, a service, or another session without
//! the user's DPAPI master key cannot decrypt `settings.json`, the secret
//! stores or browser cookies, while the app holds fresh numbers.
//!
//! The file holds rate windows and fetch times only: no credentials, cookies,
//! tokens, costs, emails, organizations or plans. It is plain JSON on purpose:
//! a DPAPI-wrapped cache could not be read in exactly the sessions it exists
//! for. It is written with the owner-only atomic writer into the per-user
//! config directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::{ProviderError, ProviderId, UsageSnapshot};

/// File name inside the CodexBar config directory.
pub const CACHE_FILE_NAME: &str = "cli-usage-cache.json";

/// Current (and only) file version.
pub const CACHE_VERSION: u32 = 1;

/// Older entries are ignored so the CLI reports its own error instead of
/// misleading numbers (upstream `CLIUsageCache.maximumAge`).
pub const MAXIMUM_AGE_SECONDS: i64 = 3600;

/// `source` of a usage row served from this cache.
pub const SOURCE_LABEL: &str = "codexbar-app";

/// A cache larger than this is not a cache the app wrote.
const MAX_CACHE_BYTES: u64 = 1024 * 1024;

static DPAPI_DECRYPT_FAILURE_OBSERVED: AtomicBool = AtomicBool::new(false);

/// Record that this process failed to decrypt a DPAPI-protected secret.
/// Called by `secure_file` and the browser cookie decryptor.
pub(crate) fn note_dpapi_decrypt_failure() {
    DPAPI_DECRYPT_FAILURE_OBSERVED.store(true, Ordering::Relaxed);
}

/// True once this process has failed a DPAPI decrypt. The flag is
/// process-wide: the CLI fetches each provider once, and an unreadable
/// secret store hides every provider's stored credentials alike.
pub fn dpapi_decrypt_failure_observed() -> bool {
    DPAPI_DECRYPT_FAILURE_OBSERVED.load(Ordering::Relaxed)
}

/// Default cache path: `%APPDATA%\CodexBar\cli-usage-cache.json`. The app
/// never reads `CODEXBAR_CONFIG`, so the CLI looks where the app writes.
pub fn default_path() -> Option<PathBuf> {
    crate::logging::config_root().map(|root| root.join(CACHE_FILE_NAME))
}

/// One provider's cached rate windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `ProviderId::cli_name`; a string so an older CLI skips providers it
    /// does not know instead of failing to decode the file.
    pub provider: String,
    /// Allowlisted fields only (see [`Entry::new`]).
    pub usage: UsageSnapshot,
}

impl Entry {
    /// `None` when the snapshot has no quota window worth sharing.
    pub fn new(provider: ProviderId, usage: &UsageSnapshot) -> Option<Self> {
        let usage = figures(usage);
        has_rate_windows(&usage).then(|| Self {
            provider: provider.cli_name().to_string(),
            usage,
        })
    }
}

/// The rate windows and fetch time of `usage`, without identity fields.
fn figures(usage: &UsageSnapshot) -> UsageSnapshot {
    let mut figures = UsageSnapshot::new(usage.primary.clone());
    figures.primary_label = usage.primary_label.clone();
    figures.secondary = usage.secondary.clone();
    figures.secondary_label = usage.secondary_label.clone();
    figures.model_specific = usage.model_specific.clone();
    figures.tertiary = usage.tertiary.clone();
    figures.extra_rate_windows = usage.extra_rate_windows.clone();
    figures.updated_at = usage.updated_at;
    figures
}

fn has_rate_windows(usage: &UsageSnapshot) -> bool {
    std::iter::once(&usage.primary)
        .chain(usage.secondary.as_ref())
        .chain(usage.model_specific.as_ref())
        .chain(usage.tertiary.as_ref())
        .chain(usage.extra_rate_windows.iter().map(|named| &named.window))
        .any(|window| !window.is_informational)
}

/// An entry served to the CLI, with its age at read time.
#[derive(Debug, Clone)]
pub struct Hit {
    pub usage: UsageSnapshot,
    pub age_seconds: i64,
}

/// The cache file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliUsageCache {
    pub version: u32,
    pub entries: Vec<Entry>,
}

impl CliUsageCache {
    pub fn new(entries: Vec<Entry>) -> Self {
        Self {
            version: CACHE_VERSION,
            entries,
        }
    }

    /// Atomic, owner-only write. The parent directory must exist.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::atomic_file::write_atomic_private(path, &bytes)
    }

    /// `None` unless `path` is a regular file (not a symlink or other reparse
    /// point) that decodes as a supported version.
    pub fn load(path: &Path) -> Option<Self> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_CACHE_BYTES {
            return None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return None;
            }
        }
        let bytes = std::fs::read(path).ok()?;
        let cache: Self = serde_json::from_slice(&bytes).ok()?;
        (cache.version == CACHE_VERSION).then_some(cache)
    }

    /// The entry for `provider` when it has a rate window and is between zero
    /// and [`MAXIMUM_AGE_SECONDS`] old. Future-dated entries are ignored.
    pub fn hit(&self, provider: ProviderId, now: DateTime<Utc>) -> Option<Hit> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.provider == provider.cli_name())?;
        let usage = figures(&entry.usage);
        if !has_rate_windows(&usage) {
            return None;
        }
        let age_seconds = (now - usage.updated_at).num_seconds();
        (0..=MAXIMUM_AGE_SECONDS)
            .contains(&age_seconds)
            .then_some(Hit { usage, age_seconds })
    }
}

/// Publish (`Some`) or withdraw (`None`) the cache at `path`. Withdrawing a
/// missing file is not an error.
pub fn publish(path: &Path, entries: Option<Vec<Entry>>) -> anyhow::Result<()> {
    match entries {
        Some(entries) => CliUsageCache::new(entries).write(path),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

/// Whether the CLI may consult the cache at all. A readable `settings.json`
/// decides through `share_usage_with_cli`. When it cannot be read (the DPAPI
/// failure this cache exists for), the cache file itself is the opt-in: the
/// app writes it only while sharing is on and removes it when it is turned
/// off.
pub fn cli_opted_in(settings_readable: bool, share_usage_with_cli: bool) -> bool {
    !settings_readable || share_usage_with_cli
}

/// True when `error` means the provider could not obtain a credential or
/// cookie, the symptom an undecryptable secret store produces. Server
/// rejections, expired sessions, parse errors and timeouts keep their own
/// error (upstream `isCredentialAccessFailure`).
pub fn is_credential_access_failure(error: &ProviderError) -> bool {
    matches!(
        error.without_failure_owner(),
        ProviderError::AuthRequired | ProviderError::NoCookies
    )
}

/// Inputs to the CLI fallback decision for one provider row.
#[derive(Debug, Clone, Copy)]
pub struct FallbackGate {
    /// Sharing is on, or `settings.json` was unreadable ([`cli_opted_in`]).
    pub opted_in: bool,
    /// No `--source`, `--account` or `--all-accounts`: the default-account
    /// fetch the app's cache describes.
    pub default_account_fetch: bool,
    /// [`dpapi_decrypt_failure_observed`] at the time of the failure.
    pub dpapi_failure_observed: bool,
}

impl FallbackGate {
    /// Whether a failed fetch with `error` may be answered from the cache.
    pub fn allows(self, error: &ProviderError) -> bool {
        self.opted_in
            && self.default_account_fetch
            && self.dpapi_failure_observed
            && is_credential_access_failure(error)
    }
}

/// The cached usage for `provider` after a failed fetch, or `None` when the
/// gate refuses or the cache has no fresh entry.
pub fn fallback_hit(
    gate: FallbackGate,
    provider: ProviderId,
    error: &ProviderError,
    path: Option<&Path>,
    now: DateTime<Utc>,
) -> Option<Hit> {
    if !gate.allows(error) {
        return None;
    }
    CliUsageCache::load(path?)?.hit(provider, now)
}

#[cfg(test)]
#[path = "cli_usage_cache_tests.rs"]
mod tests;
