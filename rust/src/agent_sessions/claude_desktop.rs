//! Windows Claude Desktop project-root discovery.
//!
//! Claude Desktop stores local-agent session work below its Electron
//! application-data directory. The layout has varied by release, so discovery
//! walks only the two known session roots to a shallow fixed depth and returns
//! nested `.claude/projects` directories. Every directory/type/path operation
//! shares the caller's metadata scan budget.
#![cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "the traversal stays platform-neutral so tests cover it, but only Windows calls it"
    )
)]

use super::pi_family::DirectoryScanBudget;
use crate::browser::claude_desktop::msix_store_dirs;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SESSION_DIRECTORY_NAMES: [&str; 2] = ["local-agent-mode-sessions", "claude-code-sessions"];
const MAX_DEPTH: usize = 4;
/// Cowork (local agent mode) transcript file written beside a session's work.
const AUDIT_FILE_NAME: &str = "audit.jsonl";
/// Entry and wall-clock caps for one cost-history discovery pass.
const COST_SCAN_MAX_ENTRIES: usize = 16_384;
const COST_SCAN_TIME_BUDGET: Duration = Duration::from_secs(5);

const SKIPPED_DIRECTORY_NAMES: [&str; 7] = [
    ".build",
    ".git",
    "build",
    "DerivedData",
    "node_modules",
    "outputs",
    "target",
];

pub(crate) struct ClaudeDesktopProjectsLocator;

/// Claude Desktop roots for cost history: project stores and Cowork audit
/// transcripts, in one sorted, de-duplicated list.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClaudeDesktopCostRoots {
    /// Canonical `.claude/projects` directories and `audit.jsonl` files.
    pub(crate) paths: Vec<PathBuf>,
    /// False when the walk ran out of its entry or time budget, so `paths`
    /// may be missing roots that exist.
    pub(crate) complete: bool,
}

/// Result of one walk below one application-data root.
struct Discovery {
    roots: Vec<PathBuf>,
    complete: bool,
}

/// Budget for one cost-history discovery pass. Discovery never shares the
/// live-session budget, so a slow Desktop tree cannot starve session listing.
pub(crate) fn cost_scan_budget() -> DirectoryScanBudget {
    DirectoryScanBudget::new(COST_SCAN_MAX_ENTRIES, MAX_DEPTH, COST_SCAN_TIME_BUDGET)
}

impl ClaudeDesktopProjectsLocator {
    /// Locate Claude Desktop roots in the current Windows user's AppData.
    ///
    /// macOS/Linux builds deliberately return no roots: their Claude Desktop
    /// layouts are not Windows paths and are handled by their native ports.
    #[cfg(windows)]
    pub(crate) fn roots(budget: &mut DirectoryScanBudget) -> Vec<PathBuf> {
        let Some(data_directory) = dirs::data_dir() else {
            return Vec::new();
        };
        Self::roots_under(&data_directory.join("Claude"), budget)
    }

    #[cfg(not(windows))]
    pub(crate) fn roots(_budget: &mut DirectoryScanBudget) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Cost-history roots from the Windows Claude Desktop folders: the classic
    /// `%APPDATA%\Claude` and each Microsoft Store package store, because MSIX
    /// installs redirect Roaming AppData into the package.
    #[cfg(windows)]
    pub(crate) fn cost_roots(budget: &mut DirectoryScanBudget) -> ClaudeDesktopCostRoots {
        let mut application_data = Vec::new();
        if let Some(data_directory) = dirs::data_dir() {
            application_data.push(data_directory.join("Claude"));
        }
        if let Some(local_directory) = dirs::data_local_dir() {
            application_data.extend(msix_store_dirs(&local_directory));
        }
        Self::cost_roots_under(&application_data, budget)
    }

    #[cfg(not(windows))]
    pub(crate) fn cost_roots(_budget: &mut DirectoryScanBudget) -> ClaudeDesktopCostRoots {
        ClaudeDesktopCostRoots {
            paths: Vec::new(),
            complete: true,
        }
    }

    /// Cost-history discovery below injected application-data directories.
    /// Cowork `audit.jsonl` files are admitted beside the project stores. The
    /// budget is shared across the roots, so a later root starts with whatever
    /// remains and reports incomplete once it is spent.
    pub(crate) fn cost_roots_under(
        application_data_roots: &[PathBuf],
        budget: &mut DirectoryScanBudget,
    ) -> ClaudeDesktopCostRoots {
        let mut paths = Vec::new();
        let mut complete = true;
        for application_data_root in application_data_roots {
            let found = Self::discover(application_data_root, budget, true);
            paths.extend(found.roots);
            complete &= found.complete;
        }
        paths.sort();
        paths.dedup();
        ClaudeDesktopCostRoots { paths, complete }
    }

    /// Discover live-session project roots below an injected application-data
    /// directory.
    ///
    /// This platform-neutral helper keeps the traversal deterministic and
    /// makes the Windows deadline contract testable without touching a real
    /// user profile.
    pub(crate) fn roots_under(
        application_data_root: &Path,
        budget: &mut DirectoryScanBudget,
    ) -> Vec<PathBuf> {
        Self::discover(application_data_root, budget, false).roots
    }

    fn discover(
        application_data_root: &Path,
        budget: &mut DirectoryScanBudget,
        include_audit_files: bool,
    ) -> Discovery {
        if !budget.has_time_remaining() {
            return Discovery {
                roots: Vec::new(),
                complete: false,
            };
        }

        let session_roots = SESSION_DIRECTORY_NAMES
            .into_iter()
            .map(|name| application_data_root.join(name))
            .collect::<Vec<_>>();
        let mut queue = session_roots
            .iter()
            .cloned()
            .map(|path| (path, 0_usize))
            .collect::<Vec<_>>();
        let mut visited = session_roots
            .iter()
            .map(|path| visited_key(path))
            .collect::<HashSet<_>>();
        let mut roots = Vec::new();
        let mut next_index = 0;

        while next_index < queue.len() && budget.has_time_remaining() {
            let (current, depth) = {
                let (path, depth) = &queue[next_index];
                (path.clone(), *depth)
            };
            next_index += 1;

            let projects = current.join(".claude").join("projects");
            if budget.has_time_remaining()
                && projects.is_dir()
                && let Some(canonical) = budget.canonicalize_if_time_remaining(&projects)
            {
                roots.push(canonical);
            }

            // Audit transcripts are cost inputs, not project roots. A symbolic
            // link is never followed, matching the project-store rule.
            let audit = current.join(AUDIT_FILE_NAME);
            if include_audit_files
                && budget.has_time_remaining()
                && is_regular_file(&audit)
                && let Some(canonical) = budget.canonicalize_if_time_remaining(&audit)
            {
                roots.push(canonical);
            }

            if depth >= MAX_DEPTH || depth >= budget.max_depth() || !budget.has_time_remaining() {
                continue;
            }

            let entries = budget.child_directories(&current);
            let children = budget.compact_map_while_time_remaining(entries, |entry| {
                let name = entry.file_name();
                if is_skipped_directory_name(&name.to_string_lossy()) {
                    return None;
                }
                let canonical = budget.canonicalize_if_time_remaining(&entry.path())?;
                let key = visited_key(&canonical);
                visited.insert(key).then_some(canonical)
            });
            queue.extend(children.into_iter().map(|path| (path, depth + 1)));
        }

        roots.sort();
        roots.dedup();
        // Every queued directory was visited, and neither the entry cap nor
        // the clock stopped a listing or a check along the way.
        let complete = next_index >= queue.len()
            && !budget.entry_limit_reached()
            && budget.has_time_remaining();
        Discovery { roots, complete }
    }
}

/// A regular file that is not a symbolic link.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn is_skipped_directory_name(name: &str) -> bool {
    SKIPPED_DIRECTORY_NAMES
        .iter()
        .any(|skipped| skipped.eq_ignore_ascii_case(name))
}

fn visited_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('/', "\\");
    #[cfg(windows)]
    {
        value.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}
