//! Per-account session forecasts for external swap cards.
//!
//! Each fresh, successful measurement is recorded into the shared Claude
//! plan-utilization history under an account-specific key, and each card
//! reads back only its own history. The key is a digest of the normalized
//! email and the organization UUID, so a slot move keeps its history, the same
//! email in two organizations stays separate, and neither raw value is
//! persisted. Activation identity (`<source>:<slot>`) is unaffected.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    ClaudeSwapAccount, ClaudeSwapAccountList, ClaudeSwapAccountRow, ClaudeSwapUsageStatus,
};
use crate::core::{
    ProviderId, RateWindow, SESSION_WINDOW_MINUTES, SessionEquivalentForecast,
    WEEKLY_WINDOW_MINUTES,
};

/// Prefix that keeps swap history apart from the signed-in Claude account's
/// history, which is keyed by plain email or token-account id.
pub const HISTORY_ACCOUNT_KEY_PREFIX: &str = "__claude_swap__:";

/// Bridge shape of a card's forecast; field names match the provider card's
/// `SessionEquivalentForecastSnapshot` so the frontend formats both alike.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSwapSessionForecastDto {
    pub estimated_windows_to_exhaust_weekly: f64,
    pub windows_until_reset: i64,
    pub available_windows_until_reset: f64,
    pub sample_count: usize,
    pub weekly_resets_at: DateTime<Utc>,
    pub weekly_used_percent: f64,
}

impl From<&SessionEquivalentForecast> for ClaudeSwapSessionForecastDto {
    fn from(forecast: &SessionEquivalentForecast) -> Self {
        Self {
            estimated_windows_to_exhaust_weekly: forecast.estimated_windows_to_exhaust_weekly,
            windows_until_reset: forecast.windows_until_reset,
            available_windows_until_reset: forecast.available_windows_until_reset,
            sample_count: forecast.sample_count,
            weekly_resets_at: forecast.weekly_resets_at,
            weekly_used_percent: forecast.weekly_used_percent,
        }
    }
}

/// One recordable measurement for a swap row.
#[derive(Debug, Clone)]
pub struct ClaudeSwapForecastSample {
    pub history_key: String,
    pub session: RateWindow,
    pub weekly: RateWindow,
    pub captured_at: DateTime<Utc>,
}

/// Opaque history scope for a row, or `None` when the row has no email.
pub fn history_account_key(row: &ClaudeSwapAccountRow) -> Option<String> {
    let email = row.email.trim().to_lowercase();
    if email.is_empty() {
        return None;
    }
    let organization = row.organization_uuid.trim().to_lowercase();
    let pair = serde_json::to_string(&[email, organization]).ok()?;
    Some(format!(
        "{HISTORY_ACCOUNT_KEY_PREFIX}{}",
        crate::core::sha256_hex(pair.as_bytes())
    ))
}

/// The measurement to record for `row`, when it is a successful live reading
/// with both a session and a weekly window. Source-reported last-good usage is
/// never recorded.
pub fn forecast_sample(
    row: &ClaudeSwapAccountRow,
    now: DateTime<Utc>,
) -> Option<ClaudeSwapForecastSample> {
    if row.usage_status != ClaudeSwapUsageStatus::Ok {
        return None;
    }
    let history_key = history_account_key(row)?;
    let five_hour = row.usage.five_hour.as_ref()?;
    let seven_day = row.usage.seven_day.as_ref()?;
    let window = |used_percent: f64, minutes: u32, resets_at| RateWindow {
        window_minutes: Some(minutes),
        resets_at,
        ..RateWindow::new(used_percent)
    };
    Some(ClaudeSwapForecastSample {
        history_key,
        session: window(
            five_hour.used_percent,
            SESSION_WINDOW_MINUTES,
            five_hour.resets_at,
        ),
        weekly: window(
            seven_day.used_percent,
            WEEKLY_WINDOW_MINUTES,
            seven_day.resets_at,
        ),
        captured_at: row.usage_fetched_at.unwrap_or(now),
    })
}

/// Fill each projected card's forecast from its own row. `forecast` records
/// the sample and returns the forecast for its history key; it is injected so
/// tests never touch the persisted history store.
pub fn attach_session_forecasts(
    list: &ClaudeSwapAccountList,
    accounts: &mut [ClaudeSwapAccount],
    now: DateTime<Utc>,
    mut forecast: impl FnMut(&ClaudeSwapForecastSample) -> Option<SessionEquivalentForecast>,
) {
    for account in accounts {
        account.session_equivalent_forecast = list
            .accounts
            .iter()
            .find(|row| row.number == account.slot)
            .and_then(|row| forecast_sample(row, now))
            .and_then(|sample| forecast(&sample))
            .map(|forecast| ClaudeSwapSessionForecastDto::from(&forecast));
    }
}

/// Record `sample` into the shared Claude history (in process and on disk)
/// and forecast from that account's history alone.
pub fn record_and_forecast(
    sample: &ClaudeSwapForecastSample,
    now: DateTime<Utc>,
    work_days: Option<u8>,
) -> Option<SessionEquivalentForecast> {
    let provider_id = ProviderId::Claude.cli_name();
    let key = Some(sample.history_key.as_str());
    // Adopt the persisted history once per scope before recording, so a
    // restart does not reset the card's forecast to "not enough samples".
    crate::core::load_persisted_history(provider_id, key);
    crate::core::record_provider_windows(
        provider_id,
        key,
        &sample.session,
        Some(&sample.weekly),
        sample.captured_at,
    );
    crate::core::forecast_for_provider(
        provider_id,
        key,
        &sample.session,
        &sample.weekly,
        now,
        work_days,
    )
}

#[cfg(test)]
mod tests {
    use super::super::{
        ClaudeSwapAccountSource, ClaudeSwapUsageMeasurement, ClaudeSwapUsageWindow,
    };
    use super::*;
    use crate::core::{
        ForecastScope, SessionEquivalentBurnEstimate, SessionEquivalentBurnEstimator,
        SessionEquivalentHistoryStore,
    };
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn row(slot: u32, email: &str, org: &str) -> ClaudeSwapAccountRow {
        ClaudeSwapAccountRow {
            number: slot,
            email: email.to_string(),
            organization_uuid: org.to_string(),
            organization_name: String::new(),
            alias: None,
            is_active: false,
            usage_status: ClaudeSwapUsageStatus::Ok,
            usage: ClaudeSwapUsageMeasurement {
                five_hour: None,
                seven_day: None,
                scoped: Vec::new(),
                spend: None,
            },
            usage_fetched_at: None,
            is_disabled: false,
            historical_usage: None,
        }
    }

    fn measured(
        mut row: ClaudeSwapAccountRow,
        session: (f64, i64),
        weekly: (f64, i64),
        fetched_at: i64,
    ) -> ClaudeSwapAccountRow {
        row.usage.five_hour = Some(ClaudeSwapUsageWindow {
            used_percent: session.0,
            resets_at: Some(ts(session.1)),
        });
        row.usage.seven_day = Some(ClaudeSwapUsageWindow {
            used_percent: weekly.0,
            resets_at: Some(ts(weekly.1)),
        });
        row.usage_fetched_at = Some(ts(fetched_at));
        row
    }

    fn list(rows: Vec<ClaudeSwapAccountRow>) -> ClaudeSwapAccountList {
        ClaudeSwapAccountList {
            active_account_number: None,
            accounts: rows,
            supports_account_switching: true,
        }
    }

    /// Records into a local store and forecasts like `record_and_forecast`,
    /// without the process-global or persisted history.
    fn local_forecaster(
        store: &mut SessionEquivalentHistoryStore,
        now: DateTime<Utc>,
    ) -> impl FnMut(&ClaudeSwapForecastSample) -> Option<SessionEquivalentForecast> + '_ {
        move |sample| {
            let scope = ForecastScope::new("claude", Some(&sample.history_key));
            let entry = |window: &RateWindow| crate::core::PlanUtilizationHistoryEntry {
                captured_at: sample.captured_at,
                used_percent: window.used_percent,
                resets_at: window.resets_at,
            };
            store.record(
                &scope,
                Some(entry(&sample.session)),
                Some(entry(&sample.weekly)),
                SessionEquivalentBurnEstimator::DEFAULT_SAMPLE_LIMIT,
            );
            let burn: SessionEquivalentBurnEstimate = SessionEquivalentBurnEstimator::estimate(
                &store.histories(&scope),
                sample.session.resets_at,
                now,
                SessionEquivalentBurnEstimator::DEFAULT_SAMPLE_LIMIT,
            )?;
            SessionEquivalentForecast::make(&sample.session, &sample.weekly, &burn, None, now, None)
        }
    }

    #[test]
    fn history_key_follows_email_and_organization_not_slot() {
        let key = history_account_key(&row(1, "Dev@Example.com ", "ORG-A")).unwrap();
        assert!(key.starts_with(HISTORY_ACCOUNT_KEY_PREFIX));
        assert!(!key.to_lowercase().contains("example"));
        assert!(!key.to_lowercase().contains("org-a"));
        assert_eq!(
            Some(key.clone()),
            history_account_key(&row(7, "dev@example.com", " org-a"))
        );
        assert_ne!(
            Some(key.clone()),
            history_account_key(&row(1, "dev@example.com", "org-b"))
        );
        assert_ne!(
            Some(key),
            history_account_key(&row(1, "dev@example.com", ""))
        );
        assert_eq!(history_account_key(&row(1, "  ", "org-a")), None);
    }

    #[test]
    fn only_successful_live_readings_with_both_windows_are_recorded() {
        let now = ts(1_700_000_000);
        let full = measured(
            row(1, "dev@example.com", "org"),
            (20.0, 1_700_010_000),
            (30.0, 1_700_500_000),
            1_699_999_000,
        );
        let sample = forecast_sample(&full, now).expect("sample");
        assert_eq!(sample.captured_at, ts(1_699_999_000));
        assert_eq!(sample.session.window_minutes, Some(SESSION_WINDOW_MINUTES));
        assert_eq!(sample.weekly.window_minutes, Some(WEEKLY_WINDOW_MINUTES));
        assert_eq!(sample.weekly.used_percent, 30.0);

        let mut expired = full.clone();
        expired.usage_status = ClaudeSwapUsageStatus::TokenExpired;
        assert!(forecast_sample(&expired, now).is_none());

        let mut session_only = full.clone();
        session_only.usage.seven_day = None;
        assert!(forecast_sample(&session_only, now).is_none());

        let mut anonymous = full;
        anonymous.email.clear();
        assert!(forecast_sample(&anonymous, now).is_none());
    }

    #[test]
    fn each_card_forecasts_from_its_own_history_across_slot_moves() {
        let session = i64::from(SESSION_WINDOW_MINUTES) * 60;
        let base = 1_700_000_000_i64;
        let weekly_reset = base + i64::from(WEEKLY_WINDOW_MINUTES) * 60;
        let current_start = base + 3 * session;
        let now = ts(current_start + 30 * 60);
        let mut store = SessionEquivalentHistoryStore::default();

        // Three completed full sessions for the work organization only.
        for i in 0..3 {
            let start = base + i * session;
            let end = start + session;
            let step = i as f64 * 5.0;
            for (captured, session_used, weekly_used) in [
                (start + 60, 40.0, 12.0 + step),
                (end - 60, 100.0, 15.0 + step),
            ] {
                let observed = list(vec![measured(
                    row(1, "dev@example.com", "org-work"),
                    (session_used, end),
                    (weekly_used, weekly_reset),
                    captured,
                )]);
                let mut cards = project(&observed);
                attach_session_forecasts(
                    &observed,
                    &mut cards,
                    now,
                    local_forecaster(&mut store, now),
                );
            }
        }

        // The account moved to slot 4; the same email in another organization
        // sits in slot 1 with no history of its own.
        let current = list(vec![
            measured(
                row(1, "dev@example.com", "org-personal"),
                (20.0, current_start + session),
                (30.0, weekly_reset),
                current_start + 30 * 60,
            ),
            measured(
                row(4, "dev@example.com", "org-work"),
                (20.0, current_start + session),
                (30.0, weekly_reset),
                current_start + 30 * 60,
            ),
        ]);
        let mut cards = project(&current);
        attach_session_forecasts(&current, &mut cards, now, local_forecaster(&mut store, now));

        let by_slot = |slot: u32| {
            cards
                .iter()
                .find(|card| card.slot == slot)
                .unwrap()
                .session_equivalent_forecast
                .clone()
        };
        let moved = by_slot(4).expect("work account keeps its history after the slot move");
        assert_eq!(moved.sample_count, 3);
        assert_eq!(moved.weekly_used_percent, 30.0);
        assert!(
            by_slot(1).is_none(),
            "another organization must not borrow the work account's history"
        );
    }

    #[test]
    fn forecasts_attach_by_slot_and_survive_hidden_personal_info() {
        let now = ts(1_700_000_000);
        let observed = list(vec![
            measured(
                row(2, "a@example.com", "org"),
                (10.0, 1_700_010_000),
                (20.0, 1_700_500_000),
                1_700_000_000,
            ),
            row(3, "b@example.com", "org"),
        ]);
        let mut cards =
            super::super::project_accounts(&observed, ClaudeSwapAccountSource::ClaudeSwap, true);
        let mut seen = Vec::new();
        attach_session_forecasts(&observed, &mut cards, now, |sample| {
            seen.push(sample.history_key.clone());
            Some(SessionEquivalentForecast {
                estimated_windows_to_exhaust_weekly: 4.0,
                windows_until_reset: 6,
                available_windows_until_reset: 5.5,
                sample_count: 3,
                weekly_resets_at: ts(1_700_500_000),
                weekly_used_percent: 20.0,
                weekly_window_id: None,
            })
        });
        assert_eq!(
            seen,
            vec![history_account_key(&observed.accounts[0]).unwrap()]
        );
        let forecast_for = |slot: u32| {
            cards
                .iter()
                .find(|card| card.slot == slot)
                .unwrap()
                .session_equivalent_forecast
                .as_ref()
                .map(|forecast| forecast.estimated_windows_to_exhaust_weekly)
        };
        assert_eq!(forecast_for(2), Some(4.0));
        assert_eq!(forecast_for(3), None);
        let payload = serde_json::to_value(&cards[0]).unwrap();
        assert_eq!(
            payload["sessionEquivalentForecast"]["estimatedWindowsToExhaustWeekly"],
            4.0
        );
        assert!(!payload.to_string().contains("a@example.com"));
    }

    fn project(list: &ClaudeSwapAccountList) -> Vec<ClaudeSwapAccount> {
        super::super::project_accounts(list, ClaudeSwapAccountSource::ClaudeSwap, false)
    }
}
