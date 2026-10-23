//! Usage Pace Prediction
//!
//! Calculates whether the user is On Track, Ahead, or Behind their usage quota
//! based on elapsed time and consumption rate.

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};

use super::RateWindow;

/// Usage pace stage indicating consumption rate relative to time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceStage {
    /// Within 2% of expected usage
    OnTrack,
    /// 2-6% ahead of expected
    SlightlyAhead,
    /// 6-12% ahead of expected
    Ahead,
    /// More than 12% ahead of expected
    FarAhead,
    /// 2-6% behind expected
    SlightlyBehind,
    /// 6-12% behind expected
    Behind,
    /// More than 12% behind expected
    FarBehind,
}

impl PaceStage {
    /// Get a short label for the stage
    pub fn label(&self) -> &'static str {
        match self {
            PaceStage::OnTrack => "On Track",
            PaceStage::SlightlyAhead => "Slightly Ahead",
            PaceStage::Ahead => "Ahead",
            PaceStage::FarAhead => "Far Ahead",
            PaceStage::SlightlyBehind => "Slightly Behind",
            PaceStage::Behind => "Behind",
            PaceStage::FarBehind => "Far Behind",
        }
    }

    /// Get an emoji indicator for the stage
    pub fn emoji(&self) -> &'static str {
        match self {
            PaceStage::OnTrack => "✓",
            PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead => "⚡",
            PaceStage::SlightlyBehind | PaceStage::Behind | PaceStage::FarBehind => "🐢",
        }
    }
}

/// Usage pace prediction result
#[derive(Debug, Clone)]
pub struct UsagePace {
    /// The pace stage
    pub stage: PaceStage,
    /// Delta between actual and expected usage (positive = ahead)
    pub delta_percent: f64,
    /// Expected usage percent based on elapsed time
    pub expected_used_percent: f64,
    /// Actual usage percent
    pub actual_used_percent: f64,
    /// Estimated time until quota is exhausted (if ahead of pace)
    pub eta_seconds: Option<f64>,
    /// Whether current pace will last until reset
    pub will_last_to_reset: bool,
}

/// Weekly window length in minutes; the only window workday pace applies to.
const WEEKLY_WINDOW_MINUTES: u32 = 10080;

/// Elapsed / remaining work time inside a weekly window, sliced at local day
/// boundaries (upstream `UsagePace.WorkdayProgress`).
#[derive(Debug, Clone, Copy)]
struct WorkdayProgress {
    work_days: u8,
    total_seconds: f64,
    elapsed_seconds: f64,
    remaining_seconds: f64,
}

impl WorkdayProgress {
    fn expected_used_percent(&self) -> f64 {
        ((self.elapsed_seconds / self.total_seconds) * 100.0).clamp(0.0, 100.0)
    }
}

impl UsagePace {
    /// Calculate weekly usage pace in the machine's local time zone.
    ///
    /// # Arguments
    /// * `window` - The rate window to analyze
    /// * `now` - Current time (defaults to Utc::now())
    /// * `default_window_minutes` - Default window duration if not specified (10080 = 7 days)
    /// * `work_days` - `Settings::weekly_progress_work_days`. Values 2..=6 on a
    ///   10080-minute window count only Monday through that weekday as elapsed
    ///   time; `None`, 1 and 7 keep plain elapsed-time pace (upstream
    ///   `UsagePace.weekly(window:now:defaultWindowMinutes:workDays:calendar:)`).
    pub fn weekly(
        window: &RateWindow,
        now: Option<DateTime<Utc>>,
        default_window_minutes: u32,
        work_days: Option<u8>,
    ) -> Option<Self> {
        Self::weekly_in(window, now, default_window_minutes, work_days, &Local)
    }

    /// Per-day share of `remaining` until `resets_at` in the machine's local
    /// time zone. See [`UsagePace::daily_target_in`].
    pub fn daily_target(
        remaining: f64,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Option<f64> {
        Self::daily_target_in(remaining, resets_at, now, &Local)
    }

    /// Upstream `UsagePace.dailyTarget`: the remaining allowance divided by the
    /// local calendar days that are left, counting today and any partial reset
    /// day. A reset exactly at local midnight adds no partial day, and today
    /// always counts, so the target is at least `remaining`. It never estimates
    /// today's consumption. Returns `None` for a negative or non-finite
    /// remainder, or a reset that is missing or not after `now`.
    pub fn daily_target_in<Tz: TimeZone>(
        remaining: f64,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        tz: &Tz,
    ) -> Option<f64> {
        if !remaining.is_finite() || remaining < 0.0 {
            return None;
        }
        let resets_at = resets_at.filter(|at| *at > now)?;
        let reset_date = resets_at.with_timezone(tz).date_naive();
        let reset_day_start = local_midnight_on(reset_date, tz)?;
        let today = now.with_timezone(tz).date_naive();
        let whole_days = (reset_date - today).num_days();
        let partial_day = i64::from(resets_at > reset_day_start);
        let day_count = u32::try_from((whole_days + partial_day).max(1)).ok()?;
        Some(remaining / f64::from(day_count))
    }

    /// [`UsagePace::weekly`] with an explicit time zone for local day
    /// boundaries (upstream passes `Calendar.current`; tests inject one).
    pub fn weekly_in<Tz: TimeZone>(
        window: &RateWindow,
        now: Option<DateTime<Utc>>,
        default_window_minutes: u32,
        work_days: Option<u8>,
        tz: &Tz,
    ) -> Option<Self> {
        let now = now.unwrap_or_else(Utc::now);
        let resets_at = window.resets_at?;
        let minutes = window.window_minutes.unwrap_or(default_window_minutes);

        if minutes == 0 {
            return None;
        }

        let duration_secs = f64::from(minutes) * 60.0;
        let time_until_reset = seconds_between(now, resets_at);

        // Must be before reset and within the window duration
        if time_until_reset <= 0.0 || time_until_reset > duration_secs {
            return None;
        }

        let elapsed = Self::clamp(duration_secs - time_until_reset, 0.0, duration_secs);
        let workday_progress = match work_days {
            Some(days) if (2..7).contains(&days) && minutes == WEEKLY_WINDOW_MINUTES => {
                Self::workday_progress(now, duration_secs, resets_at, days, tz)
            }
            _ => None,
        };
        let expected = workday_progress.map_or_else(
            || Self::clamp((elapsed / duration_secs) * 100.0, 0.0, 100.0),
            |progress| progress.expected_used_percent(),
        );
        let actual = Self::clamp(window.used_percent, 0.0, 100.0);

        // If no time has elapsed but there's usage, something's wrong
        if elapsed == 0.0 && actual > 0.0 {
            return None;
        }

        let delta = actual - expected;
        let stage = Self::stage_for_delta(delta);

        let mut eta_seconds = None;
        let mut will_last_to_reset = false;

        let pace_elapsed = workday_progress.map_or(elapsed, |p| p.elapsed_seconds);
        let effective_time_until_reset =
            workday_progress.map_or(time_until_reset, |p| p.remaining_seconds);

        if actual >= 100.0 {
            eta_seconds = Some(0.0);
        } else if pace_elapsed > 0.0 && actual > 0.0 {
            let rate = actual / pace_elapsed; // percent per (work) second
            if rate > 0.0 {
                let remaining = 100.0 - actual;
                let candidate = remaining / rate;
                if candidate >= effective_time_until_reset {
                    will_last_to_reset = true;
                } else if let Some(progress) = workday_progress {
                    eta_seconds = Self::wall_clock_interval(
                        now,
                        resets_at,
                        candidate,
                        progress.work_days,
                        tz,
                    );
                } else {
                    eta_seconds = Some(candidate);
                }
            }
        } else if pace_elapsed > 0.0 && actual == 0.0 {
            // No usage yet, will definitely last
            will_last_to_reset = true;
        }

        Some(UsagePace {
            stage,
            delta_percent: delta,
            expected_used_percent: expected,
            actual_used_percent: actual,
            eta_seconds,
            will_last_to_reset,
        })
    }

    /// Splits the weekly window at local day boundaries so reset offsets do
    /// not shift weekday classification (upstream `workdayProgress`).
    fn workday_progress<Tz: TimeZone>(
        now: DateTime<Utc>,
        duration_secs: f64,
        resets_at: DateTime<Utc>,
        work_days: u8,
        tz: &Tz,
    ) -> Option<WorkdayProgress> {
        let window_start = resets_at - Duration::milliseconds(seconds_to_millis(duration_secs)?);

        let mut total_seconds = 0.0;
        let mut elapsed_seconds = 0.0;
        let mut remaining_seconds = 0.0;

        let mut cursor = window_start;
        while cursor < resets_at {
            let next_day = next_day_boundary(cursor, tz).filter(|next| *next > cursor)?;
            let slice_end = next_day.min(resets_at);

            if is_workday(cursor, tz, work_days) {
                total_seconds += seconds_between(cursor, slice_end);
                if now > cursor {
                    elapsed_seconds += seconds_between(cursor, now.min(slice_end));
                }
                if now < slice_end {
                    remaining_seconds += seconds_between(now.max(cursor), slice_end);
                }
            }
            cursor = slice_end;
        }

        if total_seconds <= 0.0 {
            return None;
        }
        Some(WorkdayProgress {
            work_days,
            total_seconds,
            elapsed_seconds,
            remaining_seconds,
        })
    }

    /// Maps `required_work_seconds` of workday time from `now` onto wall-clock
    /// seconds, skipping non-workdays (upstream `wallClockInterval`).
    fn wall_clock_interval<Tz: TimeZone>(
        now: DateTime<Utc>,
        resets_at: DateTime<Utc>,
        required_work_seconds: f64,
        work_days: u8,
        tz: &Tz,
    ) -> Option<f64> {
        if required_work_seconds <= 0.0 {
            return Some(0.0);
        }

        let mut remaining = required_work_seconds;
        let mut cursor = now;
        while cursor < resets_at {
            let next_day = next_day_boundary(cursor, tz).filter(|next| *next > cursor)?;
            let slice_end = next_day.min(resets_at);
            if is_workday(cursor, tz, work_days) {
                let available = seconds_between(cursor, slice_end);
                if remaining <= available {
                    return Some(seconds_between(now, cursor) + remaining);
                }
                remaining -= available;
            }
            cursor = slice_end;
        }
        None
    }

    /// Calculate the stage for a given delta percentage
    fn stage_for_delta(delta: f64) -> PaceStage {
        let abs_delta = delta.abs();

        if abs_delta <= 2.0 {
            PaceStage::OnTrack
        } else if abs_delta <= 6.0 {
            if delta >= 0.0 {
                PaceStage::SlightlyAhead
            } else {
                PaceStage::SlightlyBehind
            }
        } else if abs_delta <= 12.0 {
            if delta >= 0.0 {
                PaceStage::Ahead
            } else {
                PaceStage::Behind
            }
        } else if delta >= 0.0 {
            PaceStage::FarAhead
        } else {
            PaceStage::FarBehind
        }
    }

    fn clamp(value: f64, lower: f64, upper: f64) -> f64 {
        value.clamp(lower, upper)
    }

    /// Format the ETA as a human-readable string
    pub fn format_eta(&self) -> Option<String> {
        let secs = self.eta_seconds?;
        // Display-only ETA breakdown; whole hours/minutes by design.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "display-only ETA; whole hours/minutes by design"
        )]
        let hours = (secs / 3600.0) as i64;
        #[allow(
            clippy::cast_possible_truncation,
            reason = "display-only ETA; whole hours/minutes by design"
        )]
        let minutes = ((secs % 3600.0) / 60.0) as i64;

        if hours > 24 {
            let days = hours / 24;
            Some(format!("{}d {}h", days, hours % 24))
        } else if hours > 0 {
            Some(format!("{}h {}m", hours, minutes))
        } else {
            Some(format!("{}m", minutes))
        }
    }

    /// Format the pace as a status line
    pub fn format_status(&self) -> String {
        let stage_text = self.stage.label();

        if self.will_last_to_reset {
            format!("{} - will last to reset", stage_text)
        } else if let Some(eta) = self.format_eta() {
            format!("{} - exhausted in {}", stage_text, eta)
        } else {
            stage_text.to_string()
        }
    }
}

/// Fractional seconds from `from` to `to` (Foundation `timeIntervalSince`).
fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    let delta = to - from;
    delta.num_microseconds().map_or_else(
        || delta.num_milliseconds() as f64 / 1_000.0,
        |micros| micros as f64 / 1_000_000.0,
    )
}

fn seconds_to_millis(seconds: f64) -> Option<i64> {
    let millis = (seconds * 1_000.0).round();
    if !millis.is_finite() || millis.abs() >= 9.0e15 {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "finite, rounded and bounded well inside i64 above"
    )]
    Some(millis as i64)
}

/// Start of the local calendar day after `instant` (upstream
/// `calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for:))`).
/// When local midnight does not exist (a DST gap), the first existing local
/// quarter hour after it is used, as Foundation's `startOfDay` does.
fn next_day_boundary<Tz: TimeZone>(instant: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
    let next_date = instant.with_timezone(tz).date_naive().succ_opt()?;
    local_midnight_on(next_date, tz)
}

/// Start of local calendar day `date`, using the first existing local quarter
/// hour when midnight is skipped (see [`next_day_boundary`]).
fn local_midnight_on<Tz: TimeZone>(date: NaiveDate, tz: &Tz) -> Option<DateTime<Utc>> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    (0..=180).step_by(15).find_map(|offset_minutes| {
        let candidate = midnight + Duration::minutes(offset_minutes);
        match tz.from_local_datetime(&candidate) {
            LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at.with_timezone(&Utc)),
            LocalResult::None => None,
        }
    })
}

/// Whether the ISO weekday (Mon = 1 ... Sun = 7) of `instant` in `tz` is one
/// of the first `work_days` days of the week (upstream `isWorkday`).
fn is_workday<Tz: TimeZone>(instant: DateTime<Utc>, tz: &Tz, work_days: u8) -> bool {
    instant.with_timezone(tz).weekday().number_from_monday() <= u32::from(work_days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_stage_for_delta() {
        assert_eq!(UsagePace::stage_for_delta(0.0), PaceStage::OnTrack);
        assert_eq!(UsagePace::stage_for_delta(1.5), PaceStage::OnTrack);
        assert_eq!(UsagePace::stage_for_delta(-1.5), PaceStage::OnTrack);

        assert_eq!(UsagePace::stage_for_delta(4.0), PaceStage::SlightlyAhead);
        assert_eq!(UsagePace::stage_for_delta(-4.0), PaceStage::SlightlyBehind);

        assert_eq!(UsagePace::stage_for_delta(10.0), PaceStage::Ahead);
        assert_eq!(UsagePace::stage_for_delta(-10.0), PaceStage::Behind);

        assert_eq!(UsagePace::stage_for_delta(20.0), PaceStage::FarAhead);
        assert_eq!(UsagePace::stage_for_delta(-20.0), PaceStage::FarBehind);
    }

    #[test]
    fn test_pace_calculation() {
        let now = Utc::now();
        // Window resets in 3.5 days (halfway through a 7-day window)
        let resets_at = now + Duration::days(3) + Duration::hours(12);

        // User has used 50% - exactly on track
        let window = RateWindow::with_details(50.0, Some(10080), Some(resets_at), None);
        let pace = UsagePace::weekly(&window, Some(now), 10080, None).unwrap();

        assert_eq!(pace.stage, PaceStage::OnTrack);
        assert!(pace.delta_percent.abs() < 2.0);
    }

    #[test]
    fn test_pace_ahead() {
        let now = Utc::now();
        // Window resets in 3.5 days (halfway through a 7-day window)
        let resets_at = now + Duration::days(3) + Duration::hours(12);

        // User has used 80% - way ahead of schedule
        let window = RateWindow::with_details(80.0, Some(10080), Some(resets_at), None);
        let pace = UsagePace::weekly(&window, Some(now), 10080, None).unwrap();

        assert!(matches!(
            pace.stage,
            PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead
        ));
        assert!(pace.delta_percent > 0.0);
    }

    /// Upstream `MonthlyPacingTargetTests` daily-target cases, with the local
    /// calendar injected so the results do not depend on the machine.
    mod daily_target {
        use super::super::*;
        use chrono::{FixedOffset, TimeZone, Utc};

        fn utc() -> FixedOffset {
            FixedOffset::east_opt(0).unwrap()
        }

        fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
                .unwrap()
        }

        /// 2026-02-15 12:00 UTC: 14 whole days remain until the Mar 1 reset.
        fn now() -> DateTime<Utc> {
            at(2026, 2, 15, 12, 0)
        }

        #[test]
        fn midnight_reset_counts_only_the_whole_days_before_it() {
            let target =
                UsagePace::daily_target_in(60.0, Some(at(2026, 3, 1, 0, 0)), now(), &utc());
            assert!(target.is_some_and(|value| (value - 60.0 / 14.0).abs() < 1e-9));
        }

        #[test]
        fn partial_reset_day_counts_toward_the_target() {
            let target =
                UsagePace::daily_target_in(60.0, Some(at(2026, 3, 1, 12, 0)), now(), &utc());
            assert!(target.is_some_and(|value| (value - 4.0).abs() < 1e-9));
        }

        #[test]
        fn today_counts_when_the_reset_is_later_today_or_tomorrow_midnight() {
            let later_today =
                UsagePace::daily_target_in(42.0, Some(at(2026, 2, 15, 18, 0)), now(), &utc());
            assert_eq!(later_today, Some(42.0));
            let tomorrow_midnight =
                UsagePace::daily_target_in(42.0, Some(at(2026, 2, 16, 0, 0)), now(), &utc());
            assert_eq!(tomorrow_midnight, Some(42.0));
        }

        #[test]
        fn missing_expired_or_invalid_inputs_have_no_target() {
            assert_eq!(UsagePace::daily_target_in(10.0, None, now(), &utc()), None);
            assert_eq!(
                UsagePace::daily_target_in(10.0, Some(now()), now(), &utc()),
                None
            );
            let past = now() - chrono::Duration::hours(1);
            assert_eq!(
                UsagePace::daily_target_in(10.0, Some(past), now(), &utc()),
                None
            );
            let future = Some(at(2026, 3, 1, 0, 0));
            assert_eq!(
                UsagePace::daily_target_in(-1.0, future, now(), &utc()),
                None
            );
            assert_eq!(
                UsagePace::daily_target_in(f64::NAN, future, now(), &utc()),
                None
            );
            assert_eq!(
                UsagePace::daily_target_in(f64::INFINITY, future, now(), &utc()),
                None
            );
        }

        #[test]
        fn exhausted_allowance_has_a_zero_target() {
            let target = UsagePace::daily_target_in(0.0, Some(at(2026, 3, 1, 0, 0)), now(), &utc());
            assert_eq!(target, Some(0.0));
        }

        #[test]
        fn local_calendar_days_span_a_daylight_saving_change() {
            let los_angeles = chrono_tz::America::Los_Angeles;
            // Mar 7 23:30 PST to Mar 9 00:30 PDT: three local dates, one day of hours.
            let now = at(2026, 3, 8, 7, 30);
            let reset = at(2026, 3, 9, 7, 30);
            let target = UsagePace::daily_target_in(30.0, Some(reset), now, &los_angeles);
            assert!(target.is_some_and(|value| (value - 10.0).abs() < 1e-9));
            // Local midnight on Mar 9 is 07:00 UTC: Mar 7 and Mar 8 remain, no partial day.
            let midnight = at(2026, 3, 9, 7, 0);
            let target = UsagePace::daily_target_in(30.0, Some(midnight), now, &los_angeles);
            assert!(target.is_some_and(|value| (value - 15.0).abs() < 1e-9));
        }
    }

    // Ported from upstream v0.70.0 Tests/CodexBarTests/UsagePaceTests.swift.
    // Literal expected values are the oracle. `speedMultiplierToReset` and
    // `runOutProbability` assertions are omitted: Windows has neither field.
    mod upstream_usage_pace_tests {
        use super::super::*;
        use chrono::{Duration, FixedOffset, TimeZone, Utc};

        const DAY: i64 = 24 * 3600;

        fn epoch() -> DateTime<Utc> {
            Utc.timestamp_opt(0, 0).unwrap()
        }

        fn window(used: f64, minutes: u32, resets_at: Option<DateTime<Utc>>) -> RateWindow {
            RateWindow::with_details(used, Some(minutes), resets_at, None)
        }

        fn utc(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
            Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
        }

        fn utc_calendar() -> FixedOffset {
            FixedOffset::east_opt(0).unwrap()
        }

        fn close(actual: f64, expected: f64, tolerance: f64) -> bool {
            (actual - expected).abs() < tolerance
        }

        fn weekly_utc(
            window: &RateWindow,
            now: DateTime<Utc>,
            work_days: Option<u8>,
        ) -> Option<UsagePace> {
            UsagePace::weekly_in(window, Some(now), 10080, work_days, &utc_calendar())
        }

        #[test]
        fn weekly_pace_computes_delta_and_eta() {
            let now = epoch();
            let w = window(50.0, 10080, Some(now + Duration::seconds(4 * DAY)));
            let pace = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            assert!(close(pace.expected_used_percent, 42.857, 0.01));
            assert!(close(pace.delta_percent, 7.143, 0.01));
            assert_eq!(pace.stage, PaceStage::Ahead);
            assert!(!pace.will_last_to_reset);
            assert!(close(pace.eta_seconds.unwrap(), (3 * DAY) as f64, 1.0));
        }

        #[test]
        fn weekly_pace_marks_lasts_to_reset_when_usage_is_low() {
            let now = epoch();
            let w = window(5.0, 10080, Some(now + Duration::seconds(4 * DAY)));
            let pace = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            assert!(pace.will_last_to_reset);
            assert_eq!(pace.eta_seconds, None);
            assert_eq!(pace.stage, PaceStage::FarBehind);
        }

        #[test]
        fn weekly_pace_speed_headroom_window_expected_90() {
            let now = epoch();
            let w = window(70.0, 10080, Some(now + Duration::seconds(7 * DAY / 10)));
            let pace = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            assert!(close(pace.expected_used_percent, 90.0, 0.01));
            assert!(pace.will_last_to_reset);
        }

        #[test]
        fn weekly_pace_hides_when_reset_missing_or_outside_window() {
            let now = epoch();
            let missing = window(10.0, 10080, None);
            let too_far = window(10.0, 10080, Some(now + Duration::seconds(9 * DAY)));
            assert!(UsagePace::weekly(&missing, Some(now), 10080, None).is_none());
            assert!(UsagePace::weekly(&too_far, Some(now), 10080, None).is_none());
        }

        #[test]
        fn weekly_pace_hides_when_usage_exists_but_no_elapsed() {
            let now = epoch();
            let w = window(12.0, 10080, Some(now + Duration::seconds(7 * DAY)));
            assert!(UsagePace::weekly(&w, Some(now), 10080, None).is_none());
        }

        #[test]
        fn workday_aware_pace_shows_on_track_for_five_day_user_on_friday() {
            // Window Sun Jun 7 00:00 -> Sun Jun 14 00:00; now Fri Jun 12 18:00.
            let resets_at = utc(2026, 6, 14, 0);
            let now = resets_at - Duration::hours(30);
            let w = window(100.0, 10080, Some(resets_at));
            let pace7 = weekly_utc(&w, now, None).unwrap();
            let pace5 = weekly_utc(&w, now, Some(5)).unwrap();
            assert!(pace7.delta_percent > 15.0);
            assert!(close(pace5.expected_used_percent, 95.0, 0.01));
            assert!(pace5.delta_percent.abs() <= 5.0);
        }

        #[test]
        fn workday_aware_pace_shows_on_track_midweek() {
            let resets_at = utc(2026, 6, 14, 0);
            let now = resets_at - Duration::hours(72);
            let w = window(60.0, 10080, Some(resets_at));
            let pace5 = weekly_utc(&w, now, Some(5)).unwrap();
            assert!(close(pace5.expected_used_percent, 60.0, 0.01));
            assert!(pace5.delta_percent.abs() < 0.01);
        }

        #[test]
        fn workday_aware_exhausted_quota_does_not_last_through_weekend() {
            let w = window(100.0, 10080, Some(utc(2026, 6, 14, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 13, 12), Some(5)).unwrap();
            assert!(!pace.will_last_to_reset);
            assert_eq!(pace.eta_seconds, Some(0.0));
        }

        #[test]
        fn workday_aware_eta_excludes_non_workday_elapsed_time() {
            let w = window(20.0, 10080, Some(utc(2026, 6, 14, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 8, 12), Some(5)).unwrap();
            assert!(!pace.will_last_to_reset);
            assert!(close(pace.eta_seconds.unwrap(), 48.0 * 3600.0, 1.0));
        }

        #[test]
        fn workday_aware_eta_maps_work_time_across_a_weekend() {
            let w = window(60.0, 10080, Some(utc(2026, 6, 17, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 12, 12), Some(5)).unwrap();
            // 40 work hours remain: 12 h Friday, all Monday, then 4 h Tuesday.
            assert!(!pace.will_last_to_reset);
            assert!(close(pace.eta_seconds.unwrap(), 88.0 * 3600.0, 1.0));
        }

        #[test]
        fn workday_aware_pace_stays_flat_on_non_workdays() {
            let w = window(60.0, 10080, Some(utc(2026, 6, 17, 0)));
            let saturday = weekly_utc(&w, utc(2026, 6, 13, 12), Some(5)).unwrap();
            let sunday = weekly_utc(&w, utc(2026, 6, 14, 12), Some(5)).unwrap();
            assert!(close(saturday.expected_used_percent, 60.0, 0.01));
            assert_eq!(sunday.expected_used_percent, saturday.expected_used_percent);
        }

        #[test]
        fn six_work_days_counts_saturday_and_excludes_sunday() {
            // Window Sat Jun 6 00:00 -> Sat Jun 13 00:00; Mon-Sat are work days.
            let w = window(60.0, 10080, Some(utc(2026, 6, 13, 0)));
            // Saturday noon: half of the first of six work days has elapsed.
            let saturday = weekly_utc(&w, utc(2026, 6, 6, 12), Some(6)).unwrap();
            assert!(close(
                saturday.expected_used_percent,
                100.0 * 0.5 / 6.0,
                0.01
            ));
            // Sunday is not a work day, so the expected rate stays flat through it.
            let sunday = weekly_utc(&w, utc(2026, 6, 7, 12), Some(6)).unwrap();
            assert!(close(sunday.expected_used_percent, 100.0 / 6.0, 0.01));
            // Five work days excludes Saturday, so nothing has elapsed by Sunday noon.
            let five = weekly_utc(&w, utc(2026, 6, 7, 12), Some(5)).unwrap();
            assert!(close(five.expected_used_percent, 0.0, 0.01));
        }

        #[test]
        fn zero_usage_becomes_safe_only_after_the_first_configured_workday_begins() {
            let la = chrono_tz::America::Los_Angeles;
            let resets_at = la
                .with_ymd_and_hms(2026, 6, 14, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let first_workday = la
                .with_ymd_and_hms(2026, 6, 8, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let w = window(0.0, 10080, Some(resets_at));
            let at = |now: DateTime<Utc>| {
                UsagePace::weekly_in(&w, Some(now), 10080, Some(5), &la).unwrap()
            };
            let before = at(first_workday - Duration::seconds(1));
            let boundary = at(first_workday);
            let after = at(first_workday + Duration::seconds(3600));
            assert_eq!(before.expected_used_percent, 0.0);
            assert!(!before.will_last_to_reset);
            assert_eq!(boundary.expected_used_percent, 0.0);
            assert!(!boundary.will_last_to_reset);
            assert!(after.expected_used_percent > 0.0);
            assert!(after.will_last_to_reset);
        }

        #[test]
        fn workday_aware_pace_does_not_declare_zero_usage_safe_before_first_workday() {
            let w = window(0.0, 10080, Some(utc(2026, 6, 14, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 7, 12), Some(5)).unwrap();
            assert_eq!(pace.expected_used_percent, 0.0);
            assert!(!pace.will_last_to_reset);
            assert_eq!(pace.eta_seconds, None);
        }

        #[test]
        fn workday_aware_exhausted_quota_stays_exhausted_before_first_workday() {
            let w = window(100.0, 10080, Some(utc(2026, 6, 14, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 7, 12), Some(5)).unwrap();
            assert!(!pace.will_last_to_reset);
            assert_eq!(pace.eta_seconds, Some(0.0));
        }

        #[test]
        fn workday_aware_pace_splits_a_non_midnight_reset_at_local_day_boundaries() {
            // Window starts Sunday 20:00. Monday 00:00-12:00 is 12 of 120 work hours.
            let w = window(10.0, 10080, Some(utc(2026, 6, 14, 20)));
            let pace = weekly_utc(&w, utc(2026, 6, 8, 12), Some(5)).unwrap();
            assert!(close(pace.expected_used_percent, 10.0, 0.01));
            assert!(pace.delta_percent.abs() < 0.01);
        }

        #[test]
        fn workday_aware_pace_falls_back_to_linear_when_work_days_is_none_or_7() {
            let now = epoch();
            let w = window(50.0, 10080, Some(now + Duration::seconds(4 * DAY)));
            let pace_none = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            let pace7 = UsagePace::weekly(&w, Some(now), 10080, Some(7)).unwrap();
            assert!(close(pace_none.expected_used_percent, 300.0 / 7.0, 0.01));
            assert!(close(
                pace7.expected_used_percent,
                pace_none.expected_used_percent,
                0.01
            ));
        }

        #[test]
        fn work_days_off_linear_weekly_pace_keeps_deficit_sign() {
            let now = epoch();
            let w = window(
                88.0,
                10080,
                Some(now + Duration::seconds(2 * DAY + 19 * 3600)),
            );
            let pace = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            assert!(close(
                pace.expected_used_percent,
                101.0 / 168.0 * 100.0,
                0.01
            ));
            assert!(pace.delta_percent > 25.0);
            assert_eq!(pace.stage, PaceStage::FarAhead);
            assert!(!pace.will_last_to_reset);
        }

        #[test]
        fn workday_aware_pace_ignores_non_weekly_windows() {
            let now = epoch();
            let w = window(50.0, 300, Some(now + Duration::seconds(2 * 3600)));
            let no_work = UsagePace::weekly(&w, Some(now), 300, None).unwrap();
            let work5 = UsagePace::weekly(&w, Some(now), 300, Some(5)).unwrap();
            assert!(close(
                no_work.expected_used_percent,
                work5.expected_used_percent,
                0.01
            ));
        }

        #[test]
        fn session_pace_computes_delta_and_eta_for_five_hour_window() {
            let now = epoch();
            let w = window(50.0, 300, Some(now + Duration::seconds(2 * 3600)));
            let pace = UsagePace::weekly(&w, Some(now), 300, None).unwrap();
            assert!(close(pace.expected_used_percent, 60.0, 0.01));
            assert!(close(pace.delta_percent, -10.0, 0.01));
            assert_eq!(pace.stage, PaceStage::Behind);
            assert!(pace.will_last_to_reset);
        }

        #[test]
        fn one_work_day_falls_back_to_linear_pace() {
            let now = epoch();
            let w = window(50.0, 10080, Some(now + Duration::seconds(4 * DAY)));
            let one = UsagePace::weekly(&w, Some(now), 10080, Some(1)).unwrap();
            let none = UsagePace::weekly(&w, Some(now), 10080, None).unwrap();
            assert!(close(
                one.expected_used_percent,
                none.expected_used_percent,
                0.01
            ));
        }

        // Windows-only: day boundaries follow the injected zone, not UTC. In
        // UTC+14 (Kiribati) Sun Jun 7 12:00 UTC is already Mon Jun 8 02:00.
        #[test]
        fn workday_boundaries_follow_the_injected_time_zone() {
            let w = window(0.0, 10080, Some(utc(2026, 6, 14, 0)));
            let now = utc(2026, 6, 7, 12);
            let utc_pace = weekly_utc(&w, now, Some(5)).unwrap();
            let plus14 = FixedOffset::east_opt(14 * 3600).unwrap();
            let kiribati = UsagePace::weekly_in(&w, Some(now), 10080, Some(5), &plus14).unwrap();
            assert_eq!(utc_pace.expected_used_percent, 0.0);
            assert!(kiribati.expected_used_percent > 0.0);
        }

        // Windows-only edge: a spring-forward week. The window starts Thu
        // Mar 5 23:00 PST and resets Fri Mar 13 00:00 PDT; Sunday Mar 8 is
        // 23 hours long and not a workday. Work time is 1 h of Thursday plus
        // five full local days = 435 600 s; by Mon Mar 9 12:00 PDT, 1 h + Fri
        // + half of Monday = 133 200 s have elapsed.
        #[test]
        fn workday_pace_counts_local_days_across_a_spring_forward_week() {
            let la = chrono_tz::America::Los_Angeles;
            let resets_at = la
                .with_ymd_and_hms(2026, 3, 13, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let now = la
                .with_ymd_and_hms(2026, 3, 9, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let w = window(30.0, 10080, Some(resets_at));
            let pace = UsagePace::weekly_in(&w, Some(now), 10080, Some(5), &la).unwrap();
            assert_eq!(pace.expected_used_percent, 133_200.0 / 435_600.0 * 100.0);
            assert!(close(pace.expected_used_percent, 30.578_512, 1e-6));
            // 70% left at 30% per 133 200 work seconds needs 310 800 s, more
            // than the 302 400 work seconds left.
            assert!(pace.will_last_to_reset);
            assert_eq!(pace.eta_seconds, None);
        }

        // Windows-only edge: Chile skips local midnight on Sun Sep 6 2026
        // (00:00 -04 becomes 01:00 -03). Saturday still ends at the first
        // existing instant of Sunday, so with six work days (Mon-Sat) the
        // week holds exactly 6 x 86 400 s of work time.
        #[test]
        fn workday_pace_survives_a_skipped_local_midnight() {
            let santiago = chrono_tz::America::Santiago;
            assert!(matches!(
                santiago.with_ymd_and_hms(2026, 9, 6, 0, 0, 0),
                chrono::LocalResult::None
            ));
            let resets_at = santiago
                .with_ymd_and_hms(2026, 9, 7, 0, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let w = window(50.0, 10080, Some(resets_at));
            let saturday_noon = santiago
                .with_ymd_and_hms(2026, 9, 5, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let pace =
                UsagePace::weekly_in(&w, Some(saturday_noon), 10080, Some(6), &santiago).unwrap();
            assert_eq!(pace.expected_used_percent, 475_200.0 / 518_400.0 * 100.0);
            assert!(close(pace.expected_used_percent, 91.666_667, 1e-6));
            // Sunday after the gap: every work second is spent.
            let sunday_noon = santiago
                .with_ymd_and_hms(2026, 9, 6, 12, 0, 0)
                .unwrap()
                .with_timezone(&Utc);
            let sunday =
                UsagePace::weekly_in(&w, Some(sunday_noon), 10080, Some(6), &santiago).unwrap();
            assert_eq!(sunday.expected_used_percent, 100.0);
            assert_eq!(sunday.delta_percent, -50.0);
            assert!(sunday.will_last_to_reset);
        }

        // Edge: two work days and a reset on a Saturday. The window runs Sat
        // Jun 6 to Sat Jun 13; only Mon Jun 8 and Tue Jun 9 count (172 800 s).
        #[test]
        fn two_work_days_with_a_weekend_reset() {
            let w = window(30.0, 10080, Some(utc(2026, 6, 13, 0)));
            let pace = weekly_utc(&w, utc(2026, 6, 9, 12), Some(2)).unwrap();
            assert_eq!(pace.expected_used_percent, 75.0);
            assert_eq!(pace.delta_percent, -45.0);
            assert_eq!(pace.stage, PaceStage::FarBehind);
            assert!(pace.will_last_to_reset);
            // On Wednesday the work week is over.
            let later = weekly_utc(&w, utc(2026, 6, 10, 12), Some(2)).unwrap();
            assert_eq!(later.expected_used_percent, 100.0);
            // Six work days on the same Tuesday: the opening Saturday counts
            // too, so Sat + Mon + half of Tue of six days have elapsed.
            let six = weekly_utc(&w, utc(2026, 6, 9, 12), Some(6)).unwrap();
            assert_eq!(six.expected_used_percent, 216_000.0 / 518_400.0 * 100.0);
        }
    }

    #[test]
    fn test_pace_labels() {
        assert_eq!(PaceStage::OnTrack.label(), "On Track");
        assert_eq!(PaceStage::FarAhead.label(), "Far Ahead");
        assert_eq!(PaceStage::SlightlyBehind.emoji(), "🐢");
    }
}
