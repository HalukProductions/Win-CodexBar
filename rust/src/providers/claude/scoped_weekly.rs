use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::HashSet;

use crate::core::{NamedRateWindow, RateWindow};

use super::cli_reset::slug_claude_model;

/// Id prefix shared by every model-scoped weekly window (`claude-weekly-scoped-<model>`).
pub(super) const SCOPED_WEEKLY_ID_PREFIX: &str = "claude-weekly-scoped-";

/// One entry of the `limits` array. Entries are read field by field (see
/// [`ScopedWeeklyLimit::from_value`]), so a field of the wrong type reads as
/// missing and cannot drop the entry or its siblings.
#[derive(Debug, Default)]
pub(super) struct ScopedWeeklyLimit {
    kind: Option<String>,
    group: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<ScopedWeeklyScope>,
}

#[derive(Debug)]
struct ScopedWeeklyScope {
    model: Option<ScopedWeeklyModel>,
}

#[derive(Debug)]
struct ScopedWeeklyModel {
    id: Option<String>,
    display_name: Option<String>,
}

impl ScopedWeeklyLimit {
    /// Upstream `ClaudeWebExtraRateWindowParser.scopedWeeklyLimits`: a JSON
    /// Boolean is never a percentage, and a non-object entry carries no limit
    /// (every field reads as missing, so no window is built from it).
    fn from_value(entry: &Value) -> Self {
        let text = |object: &serde_json::Map<String, Value>, key: &str| {
            object.get(key).and_then(Value::as_str).map(str::to_owned)
        };
        let Some(object) = entry.as_object() else {
            return Self::default();
        };
        let scope = object.get("scope").and_then(Value::as_object).map(|scope| {
            let model =
                scope
                    .get("model")
                    .and_then(Value::as_object)
                    .map(|model| ScopedWeeklyModel {
                        id: text(model, "id"),
                        display_name: text(model, "display_name")
                            .or_else(|| text(model, "displayName")),
                    });
            ScopedWeeklyScope { model }
        });
        Self {
            kind: text(object, "kind"),
            group: text(object, "group"),
            percent: object.get("percent").and_then(Value::as_f64),
            resets_at: text(object, "resets_at").or_else(|| text(object, "resetsAt")),
            scope,
        }
    }
}

impl<'de> Deserialize<'de> for ScopedWeeklyLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_value(&Value::deserialize(deserializer)?))
    }
}

/// Reads a `limits` value entry by entry. A value that is not an array reads
/// as no limits; non-object entries are skipped.
pub(super) fn limits_from_value(value: Value) -> Vec<ScopedWeeklyLimit> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter(|entry| entry.is_object())
            .map(ScopedWeeklyLimit::from_value)
            .collect(),
        _ => Vec::new(),
    }
}

/// Serde adapter for the `limits` field: lenient per entry and per field.
pub(super) fn deserialize_limits<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ScopedWeeklyLimit>, D::Error> {
    Ok(limits_from_value(Value::deserialize(deserializer)?))
}

pub(super) fn scoped_weekly_windows(limits: &[ScopedWeeklyLimit]) -> Vec<NamedRateWindow> {
    let mut seen = HashSet::new();
    limits
        .iter()
        .filter_map(|limit| {
            if limit.kind.as_deref() != Some("weekly_scoped")
                || limit.group.as_deref() != Some("weekly")
            {
                return None;
            }
            let percent = limit.percent.filter(|value| value.is_finite())?;
            let model = limit.scope.as_ref()?.model.as_ref()?;
            let title = model.display_name.as_deref()?.trim();
            if title.is_empty() {
                return None;
            }
            let identity = model
                .id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(title);
            let slug = slug_claude_model(identity);
            if slug.is_empty() || !seen.insert(slug.clone()) {
                return None;
            }
            let resets_at = limit_resets_at(limit);
            Some(NamedRateWindow::new(
                format!("{SCOPED_WEEKLY_ID_PREFIX}{slug}"),
                format!("{title} only"),
                RateWindow::with_details(percent, Some(7 * 24 * 60), resets_at, None),
            ))
        })
        .collect()
}

/// All-models weekly window from `limits[]` (`kind == "weekly_all"`).
///
/// Prefer this over legacy `seven_day.utilization` when Anthropic migrates
/// weekly totals into the limits array (avoids phantom 100% from stale fields).
pub(super) fn weekly_all_window(limits: &[ScopedWeeklyLimit]) -> Option<RateWindow> {
    limits.iter().find_map(|limit| {
        let kind = limit.kind.as_deref()?;
        if !matches!(kind, "weekly_all" | "all_models" | "weekly_models") {
            return None;
        }
        if limit.group.as_deref().is_some_and(|g| g != "weekly") {
            return None;
        }
        limit_window(limit, 7 * 24 * 60)
    })
}

/// 5-hour session window from `limits[]` (`kind == "session"`).
///
/// Prefer this over legacy `five_hour.utilization` when Anthropic migrates
/// the session total into the limits array (avoids phantom 100% from a stale
/// field right after a window rollover — same bug class as #210/#279).
pub(super) fn session_window(limits: &[ScopedWeeklyLimit]) -> Option<RateWindow> {
    limits.iter().find_map(|limit| {
        if limit.kind.as_deref()? != "session" {
            return None;
        }
        if limit.group.as_deref().is_some_and(|g| g != "session") {
            return None;
        }
        limit_window(limit, 5 * 60)
    })
}

/// A finite `percent`, clamped, as a window of `minutes`.
fn limit_window(limit: &ScopedWeeklyLimit, minutes: u32) -> Option<RateWindow> {
    let percent = limit.percent.filter(|value| value.is_finite())?;
    Some(RateWindow::with_details(
        percent.clamp(0.0, 100.0),
        Some(minutes),
        limit_resets_at(limit),
        None,
    ))
}

fn limit_resets_at(limit: &ScopedWeeklyLimit) -> Option<DateTime<Utc>> {
    limit
        .resets_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_valid_limits_and_deduplicates_stable_model_ids() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":{"id":"claude/fable.5:promo","display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"weekly","percent":8,"scope":{"model":{"id":"claude/fable.5:promo","display_name":"Renamed"}}}
            ]"#,
        )
        .unwrap();

        let windows = scoped_weekly_windows(&limits);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "claude-weekly-scoped-claude-fable-5-promo");
        assert_eq!(windows[0].title, "Fable only");
    }

    #[test]
    fn ignores_unrelated_malformed_and_unnamed_limits() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"session","group":"weekly","percent":7,"scope":{"model":{"display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"monthly","percent":7,"scope":{"model":{"display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":null}},
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":{"display_name":" "}}}
            ]"#,
        )
        .unwrap();

        assert!(scoped_weekly_windows(&limits).is_empty());
    }

    #[test]
    fn weekly_all_prefers_limits_percent_over_stale_seven_day() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"weekly_all","group":"weekly","percent":1,"resets_at":"2026-07-26T22:59:59Z"},
                {"kind":"weekly_scoped","group":"weekly","percent":2,"scope":{"model":{"display_name":"Fable"}}}
            ]"#,
        )
        .unwrap();

        let weekly = weekly_all_window(&limits).expect("weekly_all");
        assert!((weekly.used_percent - 1.0).abs() < f64::EPSILON);
        assert_eq!(weekly.window_minutes, Some(7 * 24 * 60));
        assert!(weekly.resets_at.is_some());

        let scoped = scoped_weekly_windows(&limits);
        assert_eq!(scoped.len(), 1);
        assert!((scoped[0].window.used_percent - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn session_window_prefers_limits_percent_over_stale_five_hour() {
        // Mirrors issue #279: right after a 5h window rollover the legacy
        // five_hour.utilization can transiently report 1.0 (would normalize
        // to 100%), while the limits[] entry already carries the fresh 5%.
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"session","group":"session","percent":5,"resets_at":"2026-08-13T12:49:59Z"},
                {"kind":"weekly_all","group":"weekly","percent":1,"resets_at":"2026-07-26T22:59:59Z"}
            ]"#,
        )
        .unwrap();

        let session = session_window(&limits).expect("session window");
        assert!((session.used_percent - 5.0).abs() < f64::EPSILON);
        assert_eq!(session.window_minutes, Some(5 * 60));
        assert!(session.resets_at.is_some());

        // weekly_all is unaffected.
        let weekly = weekly_all_window(&limits).expect("weekly_all");
        assert!((weekly.used_percent - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn session_window_ignores_mismatched_group_and_null_percent() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"session","group":"weekly","percent":42},
                {"kind":"weekly_scoped","group":"session","percent":7,"scope":{"model":{"display_name":"Fable"}}},
                {"kind":"session","group":"session","percent":null}
            ]"#,
        )
        .unwrap();

        // No qualifying session/session entry with a finite percent -> None.
        assert!(session_window(&limits).is_none());

        // A valid session/session entry wins.
        let valid: Vec<ScopedWeeklyLimit> =
            serde_json::from_str(r#"[{"kind":"session","group":"session","percent":12}]"#).unwrap();
        let session = session_window(&valid).expect("session window");
        assert!((session.used_percent - 12.0).abs() < f64::EPSILON);
    }
}
