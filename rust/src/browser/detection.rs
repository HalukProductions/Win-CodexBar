//! Browser detection for Windows and WSL
//!
//! On native Windows, uses standard AppData paths.
//! On WSL, resolves browser paths via /mnt/c/ to access Windows browser data.

use std::path::{Path, PathBuf};

use crate::wsl;

/// Supported browser types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserType {
    Chrome,
    ChromeBeta,
    ChromeDev,
    ChromeCanary,
    ChromeForTesting,
    Edge,
    Brave,
    Arc,
    Firefox,
    Chromium,
    /// Opera Stable. Its profile sits directly under the Roaming root, not in
    /// a `Default` subfolder, so the root is treated as the profile.
    Opera,
    /// Opera GX Stable, which uses the same layout as [`BrowserType::Opera`].
    OperaGx,
    /// Opera Neon Stable, which uses the same layout as [`BrowserType::Opera`].
    OperaNeon,
    /// Aside, a Chromium-based browser with the standard `User Data` layout.
    Aside,
    /// The Claude desktop app's Electron cookie store. Deliberately absent
    /// from [`BrowserType::all`]: only Claude reads it, and only when the
    /// user picks it, so generic browser scans never touch it.
    ClaudeDesktop,
}

impl BrowserType {
    /// Every web browser that generic cookie scans may read. App cookie
    /// stores such as [`BrowserType::ClaudeDesktop`] are excluded.
    pub fn all() -> &'static [BrowserType] {
        &[
            BrowserType::Chrome,
            BrowserType::ChromeBeta,
            BrowserType::ChromeDev,
            BrowserType::ChromeCanary,
            BrowserType::ChromeForTesting,
            BrowserType::Edge,
            BrowserType::Brave,
            BrowserType::Arc,
            BrowserType::Firefox,
            BrowserType::Chromium,
            BrowserType::Opera,
            BrowserType::OperaGx,
            BrowserType::OperaNeon,
            BrowserType::Aside,
        ]
    }

    /// Check if this is a Chromium-based browser
    pub fn is_chromium_based(&self) -> bool {
        !matches!(self, BrowserType::Firefox)
    }

    /// Get the display name
    pub fn display_name(&self) -> &'static str {
        match self {
            BrowserType::Chrome => "Google Chrome",
            BrowserType::ChromeBeta => "Google Chrome Beta",
            BrowserType::ChromeDev => "Google Chrome Dev",
            BrowserType::ChromeCanary => "Google Chrome Canary",
            BrowserType::ChromeForTesting => "Chrome for Testing",
            BrowserType::Edge => "Microsoft Edge",
            BrowserType::Brave => "Brave",
            BrowserType::Arc => "Arc",
            BrowserType::Firefox => "Firefox",
            BrowserType::Chromium => "Chromium",
            BrowserType::Opera => "Opera",
            BrowserType::OperaGx => "Opera GX",
            BrowserType::OperaNeon => "Opera Neon",
            BrowserType::Aside => "Aside",
            BrowserType::ClaudeDesktop => "Claude desktop app",
        }
    }

    /// Stable browser identifier shared by the core and desktop IPC bridge.
    pub fn key(&self) -> &'static str {
        match self {
            BrowserType::Chrome => "chrome",
            BrowserType::ChromeBeta => "chrome-beta",
            BrowserType::ChromeDev => "chrome-dev",
            BrowserType::ChromeCanary => "chrome-canary",
            BrowserType::ChromeForTesting => "chrome-for-testing",
            BrowserType::Edge => "edge",
            BrowserType::Brave => "brave",
            BrowserType::Arc => "arc",
            BrowserType::Firefox => "firefox",
            BrowserType::Chromium => "chromium",
            BrowserType::Opera => "opera",
            BrowserType::OperaGx => "opera-gx",
            BrowserType::OperaNeon => "opera-neon",
            BrowserType::Aside => "aside",
            BrowserType::ClaudeDesktop => "claude-desktop",
        }
    }

    /// Resolve a profile root under Windows AppData. Firefox and Opera require
    /// Roaming; local-root browsers remain resolvable when Roaming is unavailable.
    pub fn user_data_dir(&self, local: &Path, roaming: Option<&Path>) -> Option<PathBuf> {
        match self {
            BrowserType::Opera => roaming.map(|root| root.join("Opera Software/Opera Stable")),
            BrowserType::OperaGx => roaming.map(|root| root.join("Opera Software/Opera GX Stable")),
            BrowserType::OperaNeon => {
                roaming.map(|root| root.join("Opera Software/Opera Neon Stable"))
            }
            BrowserType::Aside => Some(local.join("Aside/User Data")),
            BrowserType::Chrome => Some(local.join("Google/Chrome/User Data")),
            BrowserType::ChromeBeta => Some(local.join("Google/Chrome Beta/User Data")),
            BrowserType::ChromeDev => Some(local.join("Google/Chrome Dev/User Data")),
            BrowserType::ChromeCanary => Some(local.join("Google/Chrome SxS/User Data")),
            BrowserType::ChromeForTesting => {
                Some(local.join("Google/Chrome for Testing/User Data"))
            }
            BrowserType::Edge => Some(local.join("Microsoft/Edge/User Data")),
            BrowserType::Brave => Some(local.join("BraveSoftware/Brave-Browser/User Data")),
            BrowserType::Arc => Some(local.join("Arc/User Data")),
            BrowserType::Firefox => roaming.map(|root| root.join("Mozilla/Firefox/Profiles")),
            BrowserType::Chromium => Some(local.join("Chromium/User Data")),
            BrowserType::ClaudeDesktop => super::claude_desktop::store_dir(local, roaming),
        }
    }
}

/// A detected browser installation
#[derive(Debug, Clone)]
pub struct DetectedBrowser {
    pub browser_type: BrowserType,
    pub user_data_dir: PathBuf,
    pub profiles: Vec<BrowserProfile>,
}

/// A browser profile
#[derive(Debug, Clone)]
pub struct BrowserProfile {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

impl BrowserProfile {
    /// Get the cookies database path for Chromium browsers. Profiles written
    /// by older Chromium/Electron builds keep `Cookies` at the profile root;
    /// that file is used only when `Network/Cookies` does not exist.
    pub fn cookies_db_path(&self) -> PathBuf {
        let network = self.path.join("Network").join("Cookies");
        let legacy = self.path.join("Cookies");
        if !network.is_file() && legacy.is_file() {
            return legacy;
        }
        network
    }

    /// Get the Local State file path (contains encryption key)
    pub fn local_state_path(&self, user_data_dir: &Path) -> PathBuf {
        user_data_dir.join("Local State")
    }
}

/// Browser detector for Windows and WSL
pub struct BrowserDetector;

impl BrowserDetector {
    /// Detect all installed browsers.
    ///
    /// On native Windows, scans standard AppData directories.
    /// On WSL, also scans Windows browser paths via /mnt/c/.
    pub fn detect_all() -> Vec<DetectedBrowser> {
        let mut browsers = Vec::new();

        for browser_type in BrowserType::all() {
            if let Some(browser) = Self::detect(*browser_type) {
                browsers.push(browser);
            }
        }

        if wsl::is_wsl() {
            let wsl_browsers = super::wsl_paths::WslBrowserDetector::detect_all();
            for wsl_browser in wsl_browsers {
                let already_found = browsers
                    .iter()
                    .any(|b| b.browser_type == wsl_browser.browser_type);
                if !already_found {
                    browsers.push(wsl_browser);
                }
            }
        }

        browsers
    }

    /// Detect a specific browser
    pub fn detect(browser_type: BrowserType) -> Option<DetectedBrowser> {
        let user_data_dir = Self::get_user_data_dir(browser_type)?;
        Self::detect_at_path(browser_type, user_data_dir)
    }

    /// Detect from explicit AppData roots, including WSL-mounted Windows roots.
    pub(super) fn detect_in_roots(
        browser_type: BrowserType,
        local: &Path,
        roaming: Option<&Path>,
    ) -> Option<DetectedBrowser> {
        let user_data_dir = browser_type.user_data_dir(local, roaming)?;
        Self::detect_at_path(browser_type, user_data_dir)
    }

    fn detect_at_path(
        browser_type: BrowserType,
        user_data_dir: PathBuf,
    ) -> Option<DetectedBrowser> {
        if !user_data_dir.exists() {
            return None;
        }

        let profiles = Self::detect_profiles(browser_type, &user_data_dir);

        if profiles.is_empty() {
            return None;
        }

        Some(DetectedBrowser {
            browser_type,
            user_data_dir,
            profiles,
        })
    }

    /// Get the user data directory for a browser
    fn get_user_data_dir(browser_type: BrowserType) -> Option<PathBuf> {
        // In WSL, prefer Windows AppData paths when available
        if wsl::is_wsl()
            && let Some(appdata_local) = wsl::windows_appdata_local()
        {
            let roaming = wsl::windows_appdata_roaming();
            if let Some(path) = browser_type.user_data_dir(&appdata_local, roaming.as_deref())
                && path.exists()
            {
                return Some(path);
            }
        }

        let local_app_data = dirs::data_local_dir()?;
        let app_data = dirs::data_dir();
        browser_type.user_data_dir(&local_app_data, app_data.as_deref())
    }

    /// Detect profiles within a browser's user data directory
    fn detect_profiles(browser_type: BrowserType, user_data_dir: &PathBuf) -> Vec<BrowserProfile> {
        match browser_type {
            BrowserType::Firefox => Self::detect_firefox_profiles(user_data_dir),
            BrowserType::ClaudeDesktop => super::claude_desktop::profiles(user_data_dir),
            BrowserType::Opera | BrowserType::OperaGx | BrowserType::OperaNeon => {
                Self::detect_root_profile(user_data_dir)
            }
            _ => Self::detect_chromium_profiles(user_data_dir),
        }
    }

    /// Opera keeps a single profile at its user data root, with `Local State`
    /// beside `Network/Cookies`, so the root itself is the default profile.
    /// Without `Local State` there is no cookie key to read, so no profile is
    /// reported.
    fn detect_root_profile(user_data_dir: &Path) -> Vec<BrowserProfile> {
        if !user_data_dir.join("Local State").is_file() {
            return Vec::new();
        }
        vec![BrowserProfile {
            name: "Default".to_string(),
            path: user_data_dir.to_path_buf(),
            is_default: true,
        }]
    }

    /// Detect Chromium-based browser profiles
    pub(super) fn detect_chromium_profiles(user_data_dir: &Path) -> Vec<BrowserProfile> {
        let Ok(entries) = std::fs::read_dir(user_data_dir) else {
            return Vec::new();
        };

        let mut profiles: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let path = entry.path();
                let is_profile =
                    name == "Default" || name.starts_with("Profile ") || name.starts_with("user-");
                (is_profile && path.is_dir()).then(|| BrowserProfile {
                    is_default: name == "Default",
                    name,
                    path,
                })
            })
            .collect();
        profiles.sort_by(|left, right| left.name.cmp(&right.name));
        profiles
    }

    /// Detect Firefox profiles
    fn detect_firefox_profiles(profiles_dir: &PathBuf) -> Vec<BrowserProfile> {
        let mut profiles = Vec::new();

        if let Ok(entries) = std::fs::read_dir(profiles_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // Firefox profiles are named like "abcd1234.default" or "abcd1234.default-release"
                if path.is_dir() && name.contains('.') {
                    let is_default = name.contains("default");
                    profiles.push(BrowserProfile {
                        name,
                        path,
                        is_default,
                    });
                }
            }
        }

        profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_paths_and_keys_cover_every_type() {
        let local = Path::new("C:/Users/test/AppData/Local");
        let roaming = Path::new("C:/Users/test/AppData/Roaming");
        let expected = [
            (
                BrowserType::Chrome,
                "Google/Chrome/User Data",
                "Google Chrome",
                "chrome",
            ),
            (
                BrowserType::ChromeBeta,
                "Google/Chrome Beta/User Data",
                "Google Chrome Beta",
                "chrome-beta",
            ),
            (
                BrowserType::ChromeDev,
                "Google/Chrome Dev/User Data",
                "Google Chrome Dev",
                "chrome-dev",
            ),
            (
                BrowserType::ChromeCanary,
                "Google/Chrome SxS/User Data",
                "Google Chrome Canary",
                "chrome-canary",
            ),
            (
                BrowserType::ChromeForTesting,
                "Google/Chrome for Testing/User Data",
                "Chrome for Testing",
                "chrome-for-testing",
            ),
            (
                BrowserType::Edge,
                "Microsoft/Edge/User Data",
                "Microsoft Edge",
                "edge",
            ),
            (
                BrowserType::Brave,
                "BraveSoftware/Brave-Browser/User Data",
                "Brave",
                "brave",
            ),
            (BrowserType::Arc, "Arc/User Data", "Arc", "arc"),
            (
                BrowserType::Firefox,
                "Mozilla/Firefox/Profiles",
                "Firefox",
                "firefox",
            ),
            (
                BrowserType::Chromium,
                "Chromium/User Data",
                "Chromium",
                "chromium",
            ),
            (
                BrowserType::Opera,
                "Opera Software/Opera Stable",
                "Opera",
                "opera",
            ),
            (
                BrowserType::OperaGx,
                "Opera Software/Opera GX Stable",
                "Opera GX",
                "opera-gx",
            ),
            (
                BrowserType::OperaNeon,
                "Opera Software/Opera Neon Stable",
                "Opera Neon",
                "opera-neon",
            ),
            (BrowserType::Aside, "Aside/User Data", "Aside", "aside"),
        ];

        assert_eq!(BrowserType::all().len(), expected.len());
        let mut keys = std::collections::HashSet::new();
        for (browser, relative_path, display_name, key) in expected {
            let roaming_root = matches!(
                browser,
                BrowserType::Firefox
                    | BrowserType::Opera
                    | BrowserType::OperaGx
                    | BrowserType::OperaNeon
            );
            let root = if roaming_root { roaming } else { local };
            assert_eq!(
                browser.user_data_dir(local, Some(roaming)),
                Some(root.join(relative_path)),
                "wrong profile root for {display_name}"
            );
            assert_eq!(browser.display_name(), display_name);
            assert_eq!(browser.key(), key);
            assert!(keys.insert(browser.key()), "duplicate IPC key: {key}");
            assert!(BrowserType::all().contains(&browser));
            assert_eq!(
                browser.user_data_dir(local, None),
                (!roaming_root).then(|| local.join(relative_path))
            );
        }
    }

    /// Opera keeps its whole profile directly under the Roaming root instead
    /// of in a `Default` subfolder. The root must be the single default
    /// profile, and `Local State` (the key for cookie decryption) must resolve
    /// from that root.
    #[test]
    fn opera_profile_root_is_the_single_default_profile() {
        let temp = tempfile::tempdir().unwrap();
        let roaming = temp.path().join("Roaming");
        let root = roaming.join("Opera Software/Opera Stable");
        std::fs::create_dir_all(root.join("Network")).unwrap();
        std::fs::write(root.join("Local State"), "{}").unwrap();
        std::fs::write(root.join("Network").join("Cookies"), "").unwrap();

        let detected = BrowserDetector::detect_in_roots(
            BrowserType::Opera,
            temp.path(),
            Some(roaming.as_path()),
        )
        .unwrap();

        assert_eq!(detected.user_data_dir, root);
        assert_eq!(detected.profiles.len(), 1);
        let profile = &detected.profiles[0];
        assert_eq!(profile.path, root);
        assert!(profile.is_default);
        let local_state = profile.local_state_path(&detected.user_data_dir);
        assert_eq!(local_state, root.join("Local State"));
        assert!(local_state.is_file());
        assert_eq!(
            profile.cookies_db_path(),
            root.join("Network").join("Cookies")
        );
    }

    /// Opera GX uses the same root-as-profile layout. A root without
    /// `Local State` has no cookie key, so it is not reported as a browser.
    #[test]
    fn opera_gx_root_needs_local_state_to_be_detected() {
        let temp = tempfile::tempdir().unwrap();
        let roaming = temp.path().join("Roaming");
        let root = roaming.join("Opera Software/Opera GX Stable");
        std::fs::create_dir_all(root.join("Cache")).unwrap();

        assert!(
            BrowserDetector::detect_in_roots(
                BrowserType::OperaGx,
                temp.path(),
                Some(roaming.as_path()),
            )
            .is_none()
        );

        std::fs::write(root.join("Local State"), "{}").unwrap();
        let detected = BrowserDetector::detect_in_roots(
            BrowserType::OperaGx,
            temp.path(),
            Some(roaming.as_path()),
        )
        .unwrap();
        assert_eq!(detected.profiles.len(), 1);
        assert_eq!(detected.profiles[0].path, root);
    }

    /// Opera Neon shares the Opera root-as-profile layout under its own
    /// Roaming folder, so it never picks up Opera Stable's profile.
    #[test]
    fn opera_neon_root_is_its_own_default_profile() {
        let temp = tempfile::tempdir().unwrap();
        let roaming = temp.path().join("Roaming");
        let stable = roaming.join("Opera Software/Opera Stable");
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("Local State"), "{}").unwrap();
        assert!(
            BrowserDetector::detect_in_roots(
                BrowserType::OperaNeon,
                temp.path(),
                Some(roaming.as_path()),
            )
            .is_none()
        );

        let root = roaming.join("Opera Software/Opera Neon Stable");
        std::fs::create_dir_all(root.join("Network")).unwrap();
        std::fs::write(root.join("Local State"), "{}").unwrap();
        let detected = BrowserDetector::detect_in_roots(
            BrowserType::OperaNeon,
            temp.path(),
            Some(roaming.as_path()),
        )
        .unwrap();
        assert_eq!(detected.user_data_dir, root);
        assert_eq!(detected.profiles.len(), 1);
        let profile = &detected.profiles[0];
        assert_eq!(profile.path, root);
        assert!(profile.is_default);
        assert_eq!(
            profile.local_state_path(&detected.user_data_dir),
            root.join("Local State")
        );
        assert_eq!(
            profile.cookies_db_path(),
            root.join("Network").join("Cookies")
        );
    }

    /// Aside keeps the standard Chromium `User Data` layout under Local
    /// AppData: `Local State` at the root and `Default` / `Profile N`
    /// profile folders beneath it.
    #[test]
    fn aside_profiles_are_detected_under_local_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let user_data = temp.path().join("Aside/User Data");
        std::fs::create_dir_all(user_data.join("Default/Network")).unwrap();
        std::fs::create_dir_all(user_data.join("Profile 1")).unwrap();
        std::fs::create_dir_all(user_data.join("System Profile")).unwrap();
        std::fs::write(user_data.join("Local State"), "{}").unwrap();

        let detected =
            BrowserDetector::detect_in_roots(BrowserType::Aside, temp.path(), None).unwrap();

        assert_eq!(detected.user_data_dir, user_data);
        let names: Vec<_> = detected.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Default", "Profile 1"]);
        let default = &detected.profiles[0];
        assert!(default.is_default);
        assert!(!detected.profiles[1].is_default);
        assert_eq!(
            default.local_state_path(&detected.user_data_dir),
            user_data.join("Local State")
        );
        assert_eq!(
            default.cookies_db_path(),
            user_data.join("Default").join("Network").join("Cookies")
        );
    }

    /// Firefox Developer Edition keeps its profiles under the shared Roaming
    /// root with a `<salt>.dev-edition-default` name. It must be detected as
    /// the default profile, like `.default-release`, and not be dropped.
    #[test]
    fn firefox_developer_edition_profiles_are_detected_as_default() {
        let temp = tempfile::tempdir().unwrap();
        let roaming = temp.path().join("Roaming");
        let profiles = roaming.join("Mozilla/Firefox/Profiles");
        let dev_edition = profiles.join("k7x2m9ab.dev-edition-default");
        std::fs::create_dir_all(&dev_edition).unwrap();
        std::fs::create_dir_all(profiles.join("q3w8e1rt.default-release")).unwrap();
        std::fs::create_dir_all(profiles.join("z9y8x7wv.other")).unwrap();

        let detected = BrowserDetector::detect_in_roots(
            BrowserType::Firefox,
            temp.path(),
            Some(roaming.as_path()),
        )
        .unwrap();

        assert_eq!(detected.user_data_dir, profiles);
        assert_eq!(detected.profiles.len(), 3);
        let dev = detected
            .profiles
            .iter()
            .find(|profile| profile.name == "k7x2m9ab.dev-edition-default")
            .expect("Developer Edition profile should be detected");
        assert_eq!(dev.path, dev_edition);
        assert!(dev.is_default);
        assert!(
            detected
                .profiles
                .iter()
                .any(|profile| profile.name == "q3w8e1rt.default-release" && profile.is_default)
        );
        assert!(
            detected
                .profiles
                .iter()
                .any(|profile| profile.name == "z9y8x7wv.other" && !profile.is_default)
        );
    }

    #[test]
    fn test_browser_detection() {
        let browsers = BrowserDetector::detect_all();
        println!("Detected {} browsers", browsers.len());
        for browser in &browsers {
            println!(
                "  {} at {:?} ({} profiles)",
                browser.browser_type.display_name(),
                browser.user_data_dir,
                browser.profiles.len()
            );
        }
    }
}
