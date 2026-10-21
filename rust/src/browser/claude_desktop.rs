//! Location of the Claude desktop app's Electron cookie store.
//!
//! The classic installer keeps the store in `%APPDATA%\Claude`. The MSIX
//! (Microsoft Store) package virtualizes Roaming AppData under
//! `%LOCALAPPDATA%\Packages\Claude_<publisher-hash>\LocalCache\Roaming\Claude`.
//! Discovery only lists directory names and checks that a cookie database
//! exists; it never opens or reads the store.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::detection::BrowserProfile;

const APP_DIR: &str = "Claude";
const MSIX_PACKAGE_PREFIX: &str = "claude_";

/// The Claude desktop store directory that holds a cookie database. When both
/// the classic and an MSIX store exist, the one whose cookie database changed
/// most recently wins, since a reinstall can leave the other one stale.
/// Without any cookie database the classic directory is returned when it
/// exists, so detection can still report the app as present.
pub(super) fn store_dir(local: &Path, roaming: Option<&Path>) -> Option<PathBuf> {
    let classic = roaming.map(|root| root.join(APP_DIR));
    let mut candidates: Vec<PathBuf> = classic.iter().cloned().collect();
    candidates.extend(msix_store_dirs(local));

    candidates
        .iter()
        .filter_map(|dir| cookie_db_modified(dir).map(|modified| (modified, dir)))
        // `max_by_key` keeps the last maximum; reverse so ties keep the classic store.
        .rev()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, dir)| dir.clone())
        .or_else(|| classic.filter(|dir| dir.is_dir()))
}

/// The Electron app has a single session, so its user data directory is the
/// only profile. No profile is reported without a cookie database.
pub(super) fn profiles(user_data_dir: &Path) -> Vec<BrowserProfile> {
    if cookie_db_modified(user_data_dir).is_none() {
        return Vec::new();
    }
    vec![BrowserProfile {
        name: "Default".to_string(),
        path: user_data_dir.to_path_buf(),
        is_default: true,
    }]
}

/// Microsoft Store (MSIX) Claude package stores under `local`, sorted. The
/// directories are not checked for existence.
pub(crate) fn msix_store_dirs(local: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(local.join("Packages")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.to_ascii_lowercase().starts_with(MSIX_PACKAGE_PREFIX))
        })
        .map(|entry| {
            entry
                .path()
                .join("LocalCache")
                .join("Roaming")
                .join(APP_DIR)
        })
        .collect();
    dirs.sort();
    dirs
}

fn cookie_db_modified(dir: &Path) -> Option<SystemTime> {
    [dir.join("Network").join("Cookies"), dir.join("Cookies")]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .find(std::fs::Metadata::is_file)
        .map(|metadata| metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::detection::{BrowserDetector, BrowserType};

    fn write_cookie_db(dir: &Path, network: bool) -> PathBuf {
        let db_dir = if network {
            dir.join("Network")
        } else {
            dir.to_path_buf()
        };
        std::fs::create_dir_all(&db_dir).unwrap();
        let db = db_dir.join("Cookies");
        std::fs::write(&db, b"synthetic").unwrap();
        db
    }

    fn set_modified(path: &Path, seconds: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn classic_roaming_store_is_detected() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let roaming = temp.path().join("Roaming");
        write_cookie_db(&roaming.join("Claude"), true);

        let detected =
            BrowserDetector::detect_in_roots(BrowserType::ClaudeDesktop, &local, Some(&roaming))
                .expect("classic store");

        assert_eq!(detected.user_data_dir, roaming.join("Claude"));
        assert_eq!(detected.profiles.len(), 1);
        assert_eq!(detected.profiles[0].path, roaming.join("Claude"));
        assert_eq!(
            detected.profiles[0].cookies_db_path(),
            roaming.join("Claude").join("Network").join("Cookies")
        );
        assert_eq!(
            detected.profiles[0].local_state_path(&detected.user_data_dir),
            roaming.join("Claude").join("Local State")
        );
    }

    #[test]
    fn msix_package_store_is_detected_and_legacy_cookie_path_is_used() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let store = local
            .join("Packages")
            .join("Claude_pzs8sxrjxfjjc")
            .join("LocalCache")
            .join("Roaming")
            .join("Claude");
        std::fs::create_dir_all(local.join("Packages").join("ClaudeNotAnApp")).unwrap();
        write_cookie_db(&store, false);

        let detected = BrowserDetector::detect_in_roots(BrowserType::ClaudeDesktop, &local, None)
            .expect("msix store");

        assert_eq!(detected.user_data_dir, store);
        assert_eq!(
            detected.profiles[0].cookies_db_path(),
            store.join("Cookies")
        );
    }

    #[test]
    fn newest_cookie_database_wins_between_classic_and_msix() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let roaming = temp.path().join("Roaming");
        let classic = roaming.join("Claude");
        let msix = local
            .join("Packages")
            .join("Claude_abc123")
            .join("LocalCache")
            .join("Roaming")
            .join("Claude");
        let classic_db = write_cookie_db(&classic, true);
        let msix_db = write_cookie_db(&msix, true);

        set_modified(&classic_db, 1_000);
        set_modified(&msix_db, 2_000);
        assert_eq!(store_dir(&local, Some(&roaming)), Some(msix.clone()));

        set_modified(&classic_db, 3_000);
        assert_eq!(store_dir(&local, Some(&roaming)), Some(classic.clone()));

        set_modified(&msix_db, 3_000);
        assert_eq!(
            store_dir(&local, Some(&roaming)),
            Some(classic),
            "ties keep the classic store"
        );
    }

    #[test]
    fn missing_store_or_cookie_database_is_not_detected() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let roaming = temp.path().join("Roaming");
        assert!(
            BrowserDetector::detect_in_roots(BrowserType::ClaudeDesktop, &local, Some(&roaming))
                .is_none()
        );

        std::fs::create_dir_all(roaming.join("Claude")).unwrap();
        assert!(
            BrowserDetector::detect_in_roots(BrowserType::ClaudeDesktop, &local, Some(&roaming))
                .is_none(),
            "an app directory without a cookie database has no session to read"
        );
    }

    #[test]
    fn generic_browser_scans_never_include_the_desktop_store() {
        assert!(!BrowserType::all().contains(&BrowserType::ClaudeDesktop));
        assert_eq!(BrowserType::ClaudeDesktop.key(), "claude-desktop");
        assert_eq!(
            BrowserType::ClaudeDesktop.display_name(),
            "Claude desktop app"
        );
        assert!(BrowserType::ClaudeDesktop.is_chromium_based());
    }
}
