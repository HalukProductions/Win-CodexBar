//! Best-effort coding-agent activity detection for Adaptive refresh (Windows).
//!
//! Two signals:
//! - well-known agent process names (no command lines or env are inspected, so no
//!   secret leakage);
//! - Codex rollout file modification times from today's and yesterday's session
//!   directories (upstream #4156). Only metadata is read; no rollout contents are opened.
//!
//! Consent is the Settings Adaptive toggle itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use chrono::{Datelike, Local, NaiveDate};
use codexbar::core::JsonlScanner;

/// Process base names (lowercase, without `.exe`) that count as coding activity.
const AGENT_PROCESS_NAMES: &[&str] = &[
    "claude",
    "codex",
    "cursor",
    "cursor-agent",
    "gemini",
    "opencode",
    "aider",
    "continue",
    "windsurf",
    "codeium",
    "copilot",
];

/// Returns true when at least one known coding-agent process is running.
pub fn coding_agent_process_active() -> bool {
    #[cfg(windows)]
    {
        windows_agent_running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_agent_running() -> bool {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: *mut std::ffi::c_void = -1isize as *mut std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> *mut std::ffi::c_void;
        fn Process32FirstW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut std::ffi::c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry = ProcessEntry32W {
            dw_size: std::mem::size_of::<ProcessEntry32W>() as u32,
            cnt_usage: 0,
            th32_process_id: 0,
            th32_default_heap_id: 0,
            th32_module_id: 0,
            cnt_threads: 0,
            th32_parent_process_id: 0,
            pc_pri_class_base: 0,
            dw_flags: 0,
            sz_exe_file: [0; 260],
        };
        let mut found = false;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let len = entry
                    .sz_exe_file
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.sz_exe_file.len());
                let name = OsString::from_wide(&entry.sz_exe_file[..len])
                    .to_string_lossy()
                    .to_ascii_lowercase();
                let stem = name.strip_suffix(".exe").unwrap_or(&name);
                if AGENT_PROCESS_NAMES.contains(&stem) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Clamp anchor for Codex rollouts whose mtime is in the future. Set on first sighting and kept
/// for the process lifetime (upstream `FutureModificationDateClamp`), so an unchanged
/// future-dated file keeps the same timestamp and cannot manufacture newer activity.
static FUTURE_MTIME_CLAMP: Mutex<Option<SystemTime>> = Mutex::new(None);

/// Directory entries listed per day directory. Today and yesterday each get their own budget,
/// so a full today folder cannot starve yesterday. The budget applies to listing, not to
/// file reads: rollout metadata comes from the directory enumeration on Windows.
const ROLLOUT_ENTRY_BUDGET_PER_DAY: usize = 256;
/// Newest rollout candidates kept per check (upstream `maxCodexRolloutCount`).
const ROLLOUT_CANDIDATE_CAP: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RolloutCandidate {
    path: PathBuf,
    modified: SystemTime,
}

/// Age of the newest Codex rollout modification time among today's and yesterday's
/// `YYYY/MM/DD` session directories, if that age is under `window`.
///
/// Reads directory entries and file mtimes only. Returns `None` when the sessions root is
/// unknown or no rollout falls inside `window`.
pub(crate) fn codex_rollout_recently_modified(window: Duration) -> Option<Duration> {
    let root = JsonlScanner::default_codex_sessions_root()?;
    let today = Local::now().date_naive();
    let mut clamp = FUTURE_MTIME_CLAMP.lock().unwrap_or_else(|e| e.into_inner());
    rollout_activity_age_in(&root, today, SystemTime::now(), window, &mut clamp)
}

fn rollout_activity_age_in(
    sessions_root: &Path,
    today: NaiveDate,
    now: SystemTime,
    window: Duration,
    clamp: &mut Option<SystemTime>,
) -> Option<Duration> {
    // Newest first, so the first candidate is the activity time.
    let newest = rollout_candidates_in(sessions_root, today, now, clamp)
        .first()?
        .modified;
    let age = now.duration_since(newest).unwrap_or(Duration::ZERO);
    (age < window).then_some(age)
}

/// Rollout files in today's and yesterday's day directories, newest first, at most
/// `ROLLOUT_CANDIDATE_CAP`. Every matching file in each listed directory is collected before
/// sorting, so the cap keeps the newest files, not the first ones in listing order.
///
/// A day directory with more than `ROLLOUT_ENTRY_BUDGET_PER_DAY` entries is cut short, and on
/// NTFS the listing is in name order. Rollout names start with a timestamp, so the newest files
/// are the last ones listed and may fall past that budget. A day that large loses its newest
/// files from this signal only; the process check and the normal Adaptive cadence still apply.
fn rollout_candidates_in(
    sessions_root: &Path,
    today: NaiveDate,
    now: SystemTime,
    clamp: &mut Option<SystemTime>,
) -> Vec<RolloutCandidate> {
    let Some(yesterday) = today.pred_opt() else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for day in [today, yesterday] {
        let Ok(entries) = fs::read_dir(day_directory(sessions_root, day)) else {
            continue;
        };
        for entry in entries.take(ROLLOUT_ENTRY_BUDGET_PER_DAY) {
            let Ok(entry) = entry else { continue };
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            candidates.push(RolloutCandidate {
                path: entry.path(),
                modified: clamp_future_mtime(modified, now, clamp),
            });
        }
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified));
    candidates.truncate(ROLLOUT_CANDIDATE_CAP);
    candidates
}

/// `<root>/YYYY/MM/DD`, the layout Codex uses for rollout files.
fn day_directory(sessions_root: &Path, day: NaiveDate) -> std::path::PathBuf {
    sessions_root
        .join(format!("{:04}", day.year()))
        .join(format!("{:02}", day.month()))
        .join(format!("{:02}", day.day()))
}

/// Future mtimes are pinned to the first clamp value seen, capped at `now`.
fn clamp_future_mtime(
    modified: SystemTime,
    now: SystemTime,
    clamp: &mut Option<SystemTime>,
) -> SystemTime {
    if modified <= now {
        return modified;
    }
    match *clamp {
        Some(anchor) => anchor.min(now),
        None => {
            *clamp = Some(now);
            now
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    /// Per-test directory under the system temp dir, unique per process run (pid and
    /// nanoseconds), so runs never reuse each other's files. Nothing is deleted, per the
    /// repo's no-deletion policy; old run directories stay under
    /// `%TEMP%\codexbar-rollout-activity-tests`.
    fn test_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir()
            .join("codexbar-rollout-activity-tests")
            .join(format!("{}-{nanos}", std::process::id()))
            .join(name);
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn write_rollout(root: &Path, day: NaiveDate, name: &str, modified: SystemTime) {
        let dir = day_directory(root, day);
        fs::create_dir_all(&dir).expect("create day dir");
        let path = dir.join(name);
        fs::write(&path, b"{}").expect("write rollout");
        File::options()
            .write(true)
            .open(&path)
            .expect("reopen rollout")
            .set_modified(modified)
            .expect("set mtime");
    }

    const WINDOW: Duration = Duration::from_secs(5 * 60);

    #[test]
    fn fresh_rollout_today_counts_with_its_age() {
        let root = test_root("fresh-today");
        let today = Local::now().date_naive();
        let now = SystemTime::now();
        write_rollout(
            &root,
            today,
            "rollout-fresh.jsonl",
            now - Duration::from_secs(30),
        );

        let age = rollout_activity_age_in(&root, today, now, WINDOW, &mut None);
        assert_eq!(age, Some(Duration::from_secs(30)));
    }

    #[test]
    fn fresh_rollout_yesterday_counts() {
        let root = test_root("fresh-yesterday");
        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().expect("yesterday");
        let now = SystemTime::now();
        write_rollout(
            &root,
            yesterday,
            "rollout-y.jsonl",
            now - Duration::from_secs(60),
        );

        let age = rollout_activity_age_in(&root, today, now, WINDOW, &mut None);
        assert_eq!(age, Some(Duration::from_secs(60)));
    }

    #[test]
    fn stale_or_unrelated_rollouts_do_not_count() {
        let root = test_root("stale-and-unrelated");
        let today = Local::now().date_naive();
        let two_days_ago = today
            .pred_opt()
            .and_then(|d| d.pred_opt())
            .expect("two days");
        let now = SystemTime::now();
        // Stale file inside the window dates.
        write_rollout(
            &root,
            today,
            "rollout-stale.jsonl",
            now - Duration::from_secs(10 * 60),
        );
        // Fresh files that do not match the rollout naming, and a file two days back.
        write_rollout(&root, today, "other.jsonl", now);
        write_rollout(&root, today, "rollout-ignore.txt", now);
        write_rollout(&root, two_days_ago, "rollout-old-day.jsonl", now);

        let age = rollout_activity_age_in(&root, today, now, WINDOW, &mut None);
        assert_eq!(age, None);
    }

    #[test]
    fn future_mtime_is_clamped_and_cannot_grow_when_unchanged() {
        let root = test_root("future-clamped");
        let today = Local::now().date_naive();
        let t0 = SystemTime::now();
        write_rollout(
            &root,
            today,
            "rollout-future.jsonl",
            t0 + Duration::from_secs(3600),
        );
        let mut clamp = None;

        // First sighting: pinned to the scan time, so age is zero, never negative or future.
        let first = rollout_activity_age_in(&root, today, t0, WINDOW, &mut clamp);
        assert_eq!(first, Some(Duration::ZERO));

        // Later scans of the unchanged file age from the first sighting instead of resetting.
        let later = t0 + Duration::from_secs(120);
        let second = rollout_activity_age_in(&root, today, later, WINDOW, &mut clamp);
        assert_eq!(second, Some(Duration::from_secs(120)));

        let expired = t0 + Duration::from_secs(6 * 60);
        assert_eq!(
            rollout_activity_age_in(&root, today, expired, WINDOW, &mut clamp),
            None
        );
    }

    #[test]
    fn missing_day_directories_are_not_an_error() {
        let root = test_root("missing-dirs");
        let today = Local::now().date_naive();
        assert_eq!(
            rollout_activity_age_in(&root, today, SystemTime::now(), WINDOW, &mut None),
            None
        );
    }

    #[test]
    fn candidate_cap_keeps_the_freshest_even_when_listed_last() {
        let root = test_root("cap-freshest-last");
        let today = Local::now().date_naive();
        let now = SystemTime::now();
        // 150 files, more than the 128 cap. Names are zero-padded, so the freshest file
        // (highest mtime) is the last one in name order.
        for i in 0..150u64 {
            write_rollout(
                &root,
                today,
                &format!("rollout-{i:04}.jsonl"),
                now - Duration::from_secs(150 - i),
            );
        }

        let candidates = rollout_candidates_in(&root, today, now, &mut None);
        assert_eq!(candidates.len(), ROLLOUT_CANDIDATE_CAP);
        assert_eq!(
            candidates[0].path.file_name().and_then(|n| n.to_str()),
            Some("rollout-0149.jsonl")
        );
        assert!(
            candidates
                .windows(2)
                .all(|pair| pair[0].modified >= pair[1].modified)
        );
        assert_eq!(
            rollout_activity_age_in(&root, today, now, WINDOW, &mut None),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn full_today_directory_does_not_starve_yesterday() {
        let root = test_root("full-today-yesterday");
        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().expect("yesterday");
        let now = SystemTime::now();
        // More stale files than today's per-day budget.
        for i in 0..(ROLLOUT_ENTRY_BUDGET_PER_DAY + 44) {
            write_rollout(
                &root,
                today,
                &format!("rollout-{i:04}.jsonl"),
                now - Duration::from_secs(60 * 60),
            );
        }
        write_rollout(
            &root,
            yesterday,
            "rollout-yesterday-fresh.jsonl",
            now - Duration::from_secs(60),
        );

        assert_eq!(
            rollout_activity_age_in(&root, today, now, WINDOW, &mut None),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn agent_name_list_is_nonempty() {
        assert!(!AGENT_PROCESS_NAMES.is_empty());
        assert!(AGENT_PROCESS_NAMES.contains(&"claude"));
        assert!(AGENT_PROCESS_NAMES.contains(&"codex"));
    }

    #[test]
    fn detection_does_not_panic() {
        let _ = coding_agent_process_active();
    }
}
