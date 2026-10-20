//! Claude usage-limit resets ("Reset for free" in Claude Settings > Usage),
//! read from the `cedar_ember` block of the Claude Web and OAuth usage
//! responses.
//!
//! The result is a display-only [`ProviderInventoryItem`]: a count plus the
//! soonest expiry. Grant identifiers are redemption handles and are never
//! deserialized, so they cannot reach the bridge, CLI output, or any
//! persisted snapshot. A raw OAuth body that is kept on disk goes through
//! [`redact_usage_body`] first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};

use crate::core::ProviderInventoryItem;

/// Query flag that opts a usage request in to the `cedar_ember` block.
pub(super) const RESET_OPT_IN_QUERY: &str = "cedar_ember=1";

const INVENTORY_ID: &str = "reset-credits";
const INVENTORY_TITLE: &str = "Limit Reset Credits";
/// Grant fields [`Grant`] reads; everything else is dropped before a body is stored.
const GRANT_FIELDS: [&str; 5] = [
    "resets_left",
    "resets_total",
    "starts_at",
    "ends_at",
    "paused",
];

/// Observed grants hold one reset each; a larger total is treated as
/// malformed instead of allocated.
const MAX_RESETS: usize = 50;
/// Upper bound on grant records, checked before any record is decoded.
const MAX_GRANT_RECORDS: usize = 200;

/// Build the display inventory from the raw `cedar_ember` value.
///
/// Anything unreadable yields `None` and leaves the usage windows intact.
pub(super) fn inventory_from_block(
    block: &serde_json::Value,
    now: DateTime<Utc>,
) -> Option<ProviderInventoryItem> {
    ResetStatus::deserialize(block).ok()?.inventory(now)
}

/// A usage body safe to persist. The `cedar_ember` block keeps only
/// `eligible` and the grant fields this module reads, so redemption handles
/// never reach disk while a stored body still yields the same inventory. A
/// non-object grant record becomes `null`, which keeps the record count that
/// [`MAX_GRANT_RECORDS`] checks. A body without the block is returned as is.
pub(super) fn redact_usage_body(body: &str) -> String {
    let Ok(serde_json::Value::Object(mut root)) = serde_json::from_str(body) else {
        return body.to_string();
    };
    let Some(block) = root.get_mut("cedar_ember") else {
        return body.to_string();
    };
    *block = redact_block(block);
    serde_json::Value::Object(root).to_string()
}

fn redact_block(block: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(block) = block else {
        return serde_json::Value::Null;
    };
    let mut redacted = serde_json::Map::new();
    if let Some(eligible) = block.get("eligible") {
        redacted.insert("eligible".to_string(), eligible.clone());
    }
    if let Some(serde_json::Value::Array(grants)) = block.get("grants") {
        let grants = grants
            .iter()
            .map(|grant| match grant {
                serde_json::Value::Object(fields) => serde_json::Value::Object(
                    fields
                        .iter()
                        .filter(|(key, _)| GRANT_FIELDS.contains(&key.as_str()))
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
                _ => serde_json::Value::Null,
            })
            .collect();
        redacted.insert("grants".to_string(), serde_json::Value::Array(grants));
    }
    serde_json::Value::Object(redacted)
}

/// Raw `cedar_ember` block. Only `eligible: true` yields an inventory.
#[derive(Debug, Deserialize)]
struct ResetStatus {
    eligible: bool,
    #[serde(default, deserialize_with = "lossy_grants")]
    grants: Vec<Grant>,
}

/// `resets_left` and `resets_total` are `i64` so negative or oversized
/// values are read (and rejected by the bounds check) instead of coerced.
/// `paused` is required: a grant with unknown pause state is dropped, not
/// counted. `usable_now` is deliberately not read: a saved reset counts
/// even while Claude gates redemption.
#[derive(Debug, Deserialize)]
struct Grant {
    resets_left: i64,
    #[serde(default)]
    resets_total: Option<i64>,
    #[serde(default, deserialize_with = "optional_bound")]
    starts_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "optional_bound")]
    ends_at: Option<DateTime<Utc>>,
    paused: bool,
}

impl Grant {
    /// `resets_left` must lie within `0..=resets_total`.
    fn is_well_formed(&self) -> bool {
        self.resets_left >= 0
            && self
                .resets_total
                .is_none_or(|total| total >= self.resets_left)
    }

    /// Not paused, not used up, started, and not expired at `now`.
    fn is_available(&self, now: DateTime<Utc>) -> bool {
        !self.paused
            && self.resets_left > 0
            && self.starts_at.is_none_or(|start| start <= now)
            && self.ends_at.is_none_or(|end| end > now)
    }
}

impl ResetStatus {
    fn inventory(&self, now: DateTime<Utc>) -> Option<ProviderInventoryItem> {
        if !self.eligible {
            return None;
        }
        let mut count = 0usize;
        let mut next_expiry: Option<DateTime<Utc>> = None;
        for grant in self.grants.iter().filter(|grant| grant.is_available(now)) {
            let resets = usize::try_from(grant.resets_left).ok()?;
            if resets > MAX_RESETS - count {
                return None;
            }
            count += resets;
            if let Some(ends_at) = grant.ends_at {
                next_expiry = Some(next_expiry.map_or(ends_at, |current| current.min(ends_at)));
            }
        }
        Some(ProviderInventoryItem {
            id: INVENTORY_ID.to_string(),
            title: INVENTORY_TITLE.to_string(),
            available_count: u32::try_from(count).ok().filter(|count| *count > 0)?,
            next_expires_at: next_expiry,
        })
    }
}

/// A malformed grant is dropped without hiding the rest. A `grants` value
/// that is not an array yields no grants; more than [`MAX_GRANT_RECORDS`]
/// records fails the whole block.
fn lossy_grants<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Grant>, D::Error> {
    let serde_json::Value::Array(records) = serde_json::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    if records.len() > MAX_GRANT_RECORDS {
        return Err(serde::de::Error::custom("too many grant records"));
    }
    Ok(records
        .into_iter()
        .filter_map(|record| Grant::deserialize(record).ok())
        .filter(Grant::is_well_formed)
        .collect())
}

/// An absent or null bound is open; a supplied but unreadable bound makes the
/// grant malformed, never unbounded.
fn optional_bound<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(|raw| {
            DateTime::parse_from_rfc3339(&raw)
                .map(|parsed| parsed.with_timezone(&Utc))
                .map_err(|_| serde::de::Error::custom("unreadable ISO-8601 bound"))
        })
        .transpose()
}

#[cfg(test)]
mod tests;
