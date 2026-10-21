//! Claude transcript roots for the local cost scan.
//!
//! The Claude profile root (`CLAUDE_CONFIG_DIR` or `~/.claude`) is always
//! scanned. claude-swap (`cswap run`) additionally keeps one Claude profile per
//! session home below `~/.claude-swap-backup/sessions/<N>-<label>/projects`,
//! which Claude Code writes to when a swapped session is active. Those homes
//! are added so the aggregate totals include them (upstream 0.65.0,
//! `ClaudeConfigPaths.costProjectsRoots`). Only transcript directories are
//! read; claude-swap credentials and private metadata are never touched.
//!
//! claude-swap uses the legacy `~/.claude-swap-backup` root on Windows and
//! macOS. The XDG data root applies to Linux/WSL only and is not needed here.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub(super) struct ClaudeProjectsRoots {
    pub(super) paths: Vec<PathBuf>,
    pub(super) read_failures: u32,
    /// The roots come from a token account's declared config directory, so
    /// ambient, claude-swap and Pi history are excluded.
    pub(super) account_mapped: bool,
}

impl ClaudeProjectsRoots {
    pub(super) fn has_possible_roots(&self) -> bool {
        !self.paths.is_empty() || self.read_failures > 0
    }

    /// Adds Claude Desktop cost roots (project stores and Cowork audit files),
    /// de-duplicated by resolved path against the roots already present. An
    /// incomplete discovery walk counts as a read failure, so the estimate
    /// stays unavailable rather than silently missing Desktop history.
    pub(super) fn add_desktop_roots(&mut self, desktop: Vec<PathBuf>, complete: bool) {
        if !complete {
            self.read_failures = self.read_failures.saturating_add(1);
        }
        let mut seen: HashSet<PathBuf> = self.paths.iter().map(|root| resolved(root)).collect();
        for root in desktop {
            if seen.insert(resolved(&root)) {
                self.paths.push(root);
            }
        }
    }
}

/// True when `CLAUDE_CONFIG_DIR` is unset or blank. Only the default history
/// takes Claude Desktop roots; a configured directory keeps them out.
pub(super) fn config_dir_is_unset(config_dir: Option<&str>) -> bool {
    config_dir.map(str::trim).is_none_or(str::is_empty)
}

fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Existing Claude transcript roots, de-duplicated by resolved path.
///
/// `config_dir` is the raw `CLAUDE_CONFIG_DIR` value: one literal directory
/// (never split on separators), with a blank value meaning "unset". A root
/// that does not exist is omitted, which scans the same rows as upstream's
/// empty missing root; failures that prevent discovering existing roots are
/// retained for scan coverage.
pub(super) fn claude_projects_roots(
    config_dir: Option<&str>,
    home: Option<&Path>,
) -> ClaudeProjectsRoots {
    let mut result = ClaudeProjectsRoots {
        paths: vec![base_projects_dir(config_dir, home)],
        ..ClaudeProjectsRoots::default()
    };
    if let Some(home) = home {
        let swaps = swap_projects_roots(home);
        result.paths.extend(swaps.paths);
        result.read_failures = result.read_failures.saturating_add(swaps.read_failures);
    }

    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for root in result.paths.drain(..) {
        match fs::metadata(&root) {
            Ok(_) => {}
            Err(error) if is_missing_path(&error) => continue,
            Err(_) => {
                result.read_failures = result.read_failures.saturating_add(1);
                continue;
            }
        }

        // Shared-history symlinks or junctions resolve to the same directory
        // as the profile they point at; scan that directory once.
        let resolved = match fs::canonicalize(&root) {
            Ok(resolved) => resolved,
            Err(_) => {
                result.read_failures = result.read_failures.saturating_add(1);
                root.clone()
            }
        };
        if seen.insert(resolved) {
            paths.push(root);
        }
    }
    result.paths = paths;
    result
}

/// The single `projects` root of a config directory mapped to the active
/// Claude token account (upstream 0.77.0 #4556). The ambient profile root and
/// claude-swap homes are never added. A missing or unreadable history
/// directory counts as a read failure, so the estimate stays unavailable
/// instead of becoming a known zero.
pub(super) fn claude_account_projects_roots(config_dir: &Path) -> ClaudeProjectsRoots {
    let projects = config_dir.join("projects");
    let mut result = ClaudeProjectsRoots {
        account_mapped: true,
        ..ClaudeProjectsRoots::default()
    };
    match fs::metadata(&projects) {
        Ok(metadata) if metadata.is_dir() => result.paths.push(projects),
        _ => result.read_failures = 1,
    }
    result
}

fn base_projects_dir(config_dir: Option<&str>, home: Option<&Path>) -> PathBuf {
    if let Some(trimmed) = config_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(trimmed).join("projects");
    }

    let home = home.unwrap_or_else(|| Path::new("."));
    let claude_dir = home.join(".claude").join("projects");
    if claude_dir.exists() {
        return claude_dir;
    }
    home.join(".config").join("claude").join("projects")
}

/// `<home>/.claude-swap-backup/sessions/<N>-<label>/projects` directories,
/// sorted by path. Only immediate children of `sessions` qualify; `<N>` must
/// be a positive integer and `projects` must be a directory (a dangling shared
/// history link is skipped without affecting the other homes). Discovery
/// failures are retained so callers do not report incomplete history as a
/// complete zero.
fn swap_projects_roots(home: &Path) -> ClaudeProjectsRoots {
    let sessions = home.join(".claude-swap-backup").join("sessions");
    let entries = match fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ClaudeProjectsRoots::default();
        }
        Err(_) => {
            return ClaudeProjectsRoots {
                read_failures: 1,
                ..ClaudeProjectsRoots::default()
            };
        }
    };

    let mut result = ClaudeProjectsRoots::default();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.read_failures = result.read_failures.saturating_add(1);
                continue;
            }
        };
        if !is_swap_slot_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let projects = entry.path().join("projects");
        match fs::metadata(&projects) {
            Ok(metadata) if metadata.is_dir() => result.paths.push(projects),
            Ok(_) => {}
            Err(error) if is_missing_path(&error) => {}
            Err(_) => result.read_failures = result.read_failures.saturating_add(1),
        }
    }

    result.paths.sort();
    result
}

fn is_missing_path(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// `<N>-<label>` with `<N>` a positive integer. The label may be empty, as in
/// upstream's `split(separator: "-", maxSplits: 1, omittingEmptySubsequences:
/// false)` check; hidden entries are skipped like `.skipsHiddenFiles`.
fn is_swap_slot_name(name: &str) -> bool {
    if name.starts_with('.') {
        return false;
    }
    name.split_once('-')
        .and_then(|(number, _label)| number.parse::<i64>().ok())
        .is_some_and(|number| number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_projects(root: &Path) -> PathBuf {
        let projects = root.join("projects");
        fs::create_dir_all(&projects).unwrap();
        projects
    }

    fn swap_home(home: &Path, slot: &str) -> PathBuf {
        make_projects(&home.join(".claude-swap-backup").join("sessions").join(slot))
    }

    #[test]
    fn slot_names_need_a_positive_integer_and_a_dash() {
        for accepted in ["1-first", "12-a-b", "+5-plus", "3-"] {
            assert!(is_swap_slot_name(accepted), "{accepted}");
        }
        for rejected in [
            "0-foo",
            "-1-foo",
            "unrelated",
            "abc-1",
            "7",
            "",
            ".1-hidden",
            " 1-x",
            "99999999999999999999-overflow",
        ] {
            assert!(!is_swap_slot_name(rejected), "{rejected}");
        }
    }

    #[test]
    fn swap_homes_are_added_after_the_profile_root_sorted_by_path() {
        let home = tempfile::tempdir().unwrap();
        let base = make_projects(&home.path().join(".claude"));
        let second = swap_home(home.path(), "2-second");
        let first = swap_home(home.path(), "1-first");
        swap_home(home.path(), "unrelated");
        swap_home(home.path(), "0-zero");

        let roots = claude_projects_roots(None, Some(home.path()));
        assert_eq!(roots.paths, vec![base, first, second]);
    }

    #[test]
    fn literal_config_dir_stays_beside_swap_homes() {
        let home = tempfile::tempdir().unwrap();
        let configured = tempfile::tempdir().unwrap();
        let configured_projects = make_projects(configured.path());
        make_projects(&home.path().join(".claude"));
        let swap = swap_home(home.path(), "2-second");

        let roots = claude_projects_roots(configured.path().to_str(), Some(home.path()));
        assert_eq!(roots.paths, vec![configured_projects, swap]);
    }

    #[test]
    fn empty_config_dir_falls_back_to_the_default_root() {
        let home = tempfile::tempdir().unwrap();
        let default = make_projects(&home.path().join(".config").join("claude"));
        assert_eq!(
            claude_projects_roots(Some("  "), Some(home.path())).paths,
            vec![default]
        );
    }

    #[test]
    fn missing_profile_root_does_not_stop_swap_homes() {
        let home = tempfile::tempdir().unwrap();
        let swap = swap_home(home.path(), "1-first");
        // A slot without a projects directory and a file named like a slot.
        fs::create_dir_all(
            home.path()
                .join(".claude-swap-backup")
                .join("sessions")
                .join("2-empty"),
        )
        .unwrap();
        fs::write(
            home.path()
                .join(".claude-swap-backup")
                .join("sessions")
                .join("3-file"),
            b"x",
        )
        .unwrap();

        assert_eq!(
            claude_projects_roots(None, Some(home.path())).paths,
            vec![swap]
        );
        assert!(
            claude_projects_roots(None, Some(&home.path().join("nowhere")))
                .paths
                .is_empty()
        );
    }

    #[test]
    fn a_profile_root_matching_a_swap_home_by_resolved_path_is_listed_once() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join(".claude-swap-backup").join("sessions");
        let first = swap_home(home.path(), "1-first");
        let respelled = sessions.join("..").join("sessions").join("1-first");

        let roots = claude_projects_roots(respelled.to_str(), Some(home.path()));
        assert_eq!(roots.paths.len(), 1);
        assert_eq!(
            fs::canonicalize(&roots.paths[0]).unwrap(),
            fs::canonicalize(first).unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn shared_history_symlink_is_scanned_once() {
        let home = tempfile::tempdir().unwrap();
        let first = swap_home(home.path(), "1-first");
        let shared = home
            .path()
            .join(".claude-swap-backup")
            .join("sessions")
            .join("3-shared");
        fs::create_dir_all(&shared).unwrap();
        // Creating a symlink needs a privilege on Windows; skip without it.
        if std::os::windows::fs::symlink_dir(&first, shared.join("projects")).is_err() {
            return;
        }

        assert_eq!(
            claude_projects_roots(None, Some(home.path())).paths,
            vec![first]
        );
    }

    /// Junctions need no symlink privilege, so this covers the resolved-path
    /// de-duplication of a shared-history link on every Windows host.
    #[cfg(windows)]
    #[test]
    fn shared_history_junction_is_scanned_once() {
        use std::os::windows::process::CommandExt;
        let home = tempfile::tempdir().unwrap();
        let first = swap_home(home.path(), "1-first");
        let second = swap_home(home.path(), "2-second");
        let shared = home
            .path()
            .join(".claude-swap-backup")
            .join("sessions")
            .join("3-shared");
        fs::create_dir_all(&shared).unwrap();
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "New-Item -ItemType Junction -Path $env:CODEXBAR_TEST_LINK -Target $env:CODEXBAR_TEST_TARGET | Out-Null",
            ])
            .env("CODEXBAR_TEST_LINK", shared.join("projects"))
            .env("CODEXBAR_TEST_TARGET", &first)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Failed to create the shared-history junction fixture."
        );
        assert!(shared.join("projects").is_dir());

        let roots = claude_projects_roots(None, Some(home.path()));
        assert_eq!(roots.paths, vec![first, second]);
        assert_eq!(roots.read_failures, 0);
    }

    #[test]
    fn account_roots_exclude_configured_ambient_and_swap_roots() {
        let home = tempfile::tempdir().unwrap();
        let swap = swap_home(home.path(), "1-personal");
        let ambient = home.path().join("ambient");
        let ambient_projects = make_projects(&ambient);
        let account = home.path().join("account, directory");
        let account_projects = make_projects(&account);

        let mapped = claude_account_projects_roots(&account);
        assert_eq!(mapped.paths, vec![account_projects]);
        assert_eq!(mapped.read_failures, 0);
        assert!(mapped.account_mapped);

        let combined = claude_projects_roots(ambient.to_str(), Some(home.path()));
        assert_eq!(combined.paths, vec![ambient_projects, swap]);
        assert!(!combined.account_mapped);
    }

    #[test]
    fn a_missing_account_history_directory_is_unavailable_not_empty() {
        let home = tempfile::tempdir().unwrap();
        let missing = claude_account_projects_roots(&home.path().join("deleted"));
        assert!(missing.paths.is_empty());
        assert_eq!(missing.read_failures, 1);
        assert!(missing.has_possible_roots());

        fs::write(home.path().join("projects"), b"not a directory").unwrap();
        let not_a_dir = claude_account_projects_roots(home.path());
        assert!(not_a_dir.paths.is_empty());
        assert_eq!(not_a_dir.read_failures, 1);
    }

    #[test]
    fn session_enumeration_errors_are_reported() {
        let home = tempfile::tempdir().unwrap();
        let backup = home.path().join(".claude-swap-backup");
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("sessions"), b"not a directory").unwrap();

        let roots = claude_projects_roots(None, Some(home.path()));
        assert!(roots.paths.is_empty());
        assert_eq!(roots.read_failures, 1);
        assert!(roots.has_possible_roots());
    }

    #[test]
    fn desktop_roots_join_once_and_an_incomplete_walk_is_a_read_failure() {
        let home = tempfile::tempdir().unwrap();
        let projects = make_projects(&home.path().join(".claude"));
        let audit = home.path().join("audit.jsonl");
        fs::write(&audit, b"{}\n").unwrap();

        let mut roots = claude_projects_roots(None, Some(home.path()));
        // The projects directory found again by Desktop discovery is not added twice.
        roots.add_desktop_roots(vec![projects.clone(), audit.clone()], true);
        assert_eq!(roots.paths, vec![projects.clone(), audit.clone()]);
        assert_eq!(roots.read_failures, 0);

        roots.add_desktop_roots(Vec::new(), false);
        assert_eq!(roots.read_failures, 1);
        assert!(roots.has_possible_roots());
    }

    #[test]
    fn only_an_unset_or_blank_config_dir_takes_desktop_roots() {
        assert!(config_dir_is_unset(None));
        assert!(config_dir_is_unset(Some("")));
        assert!(config_dir_is_unset(Some("   ")));
        assert!(!config_dir_is_unset(Some("D:\\claude-profile")));
    }
}
