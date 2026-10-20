//! Neutral provider collection data shared by serve projections.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};

use crate::core::ProviderFetchResult;

/// One collected provider row: the fetch outcome plus routing metadata.
pub struct ProviderFetchEnvelope {
    pub id: String,
    pub display_name: String,
    pub session_label: String,
    pub weekly_label: String,
    pub fetch: Result<ProviderFetchResult, String>,
}

/// Local cost scan data for one provider (codex / claude only upstream).
pub struct RawCostPayload {
    pub today_usd: Option<f64>,
    pub last_30_days_usd: Option<f64>,
}

/// One collected account row for a provider's multi-account section.
pub struct AccountFetchEnvelope {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub fetch: Result<ProviderFetchResult, String>,
}

/// Multi-account section input: Claude token accounts ("claude-swap"
/// upstream), or any provider's token accounts under `serve --all-accounts`.
pub struct ClaudeAccountsInput {
    pub accounts: Result<Vec<AccountFetchEnvelope>, String>,
    /// Some configured account produced no row (its fetch task failed);
    /// the snapshot reports "Account list incomplete".
    pub incomplete: bool,
}

/// Raw collection shared by the dashboard and metrics projections.
pub struct SnapshotCollection {
    pub providers: Vec<ProviderFetchEnvelope>,
    pub costs: HashMap<String, RawCostPayload>,
    pub claude_accounts: Option<ClaudeAccountsInput>,
    /// Token-account sections for other providers, keyed by provider id.
    /// Only `serve --all-accounts` fills it.
    pub provider_accounts: HashMap<String, ClaudeAccountsInput>,
    pub generated_at: DateTime<Utc>,
    pub refresh_seconds: u32,
    /// Ordered provider ids from settings (`provider_order`).
    pub order: Vec<String>,
    pub enabled: BTreeSet<String>,
}
