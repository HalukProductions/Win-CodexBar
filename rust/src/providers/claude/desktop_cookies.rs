//! claude.ai session from the Claude desktop app's cookie store (upstream
//! #4539).
//!
//! Opt-in through `claude_desktop_cookies_enabled` with the Automatic cookie
//! source. The store is read fresh on every refresh and never cached, so
//! switching accounts in the desktop app is followed on the next refresh, and
//! browsers are never used as a fallback for this explicit account choice.

use crate::browser::cookies::{
    CLAUDE_DESKTOP_APP_BOUND_MESSAGE, Cookie, CookieError, CookieExtractor,
};
use crate::browser::detection::{BrowserDetector, BrowserType, DetectedBrowser};
use crate::core::ProviderError;
use crate::settings::Settings;

const SESSION_COOKIE: &str = "sessionKey";
const COOKIE_DOMAIN: &str = "claude.ai";

pub(super) const NO_DESKTOP_SESSION_MESSAGE: &str = "No readable Claude desktop app session found. \
     Sign in to the Claude desktop app, or turn off \"Use Claude desktop app cookies\" in Settings > Providers > Claude.";

/// Whether this refresh should read the desktop app instead of browsers.
pub(super) fn selected(settings: &Settings) -> bool {
    settings.claude_desktop_cookies_enabled()
        && matches!(
            settings
                .claude_cookie_source()
                .to_ascii_lowercase()
                .as_str(),
            "auto" | "browser" | "web"
        )
}

/// The `Cookie` header for the desktop app's claude.ai session.
pub(super) fn session_cookie_header() -> Result<String, ProviderError> {
    session_cookie_header_from(
        BrowserDetector::detect(BrowserType::ClaudeDesktop),
        CookieExtractor::extract_for_domain,
    )
}

fn session_cookie_header_from(
    store: Option<DetectedBrowser>,
    read: impl FnOnce(&DetectedBrowser, &str) -> Result<Vec<Cookie>, CookieError>,
) -> Result<String, ProviderError> {
    let store = store.ok_or_else(no_session)?;
    let cookies = read(&store, COOKIE_DOMAIN).map_err(|error| match error {
        CookieError::AppBoundEncryption => {
            ProviderError::Other(CLAUDE_DESKTOP_APP_BOUND_MESSAGE.to_string())
        }
        error => {
            tracing::debug!(error = %error, "Claude desktop app cookie store could not be read");
            no_session()
        }
    })?;
    let has_session = cookies
        .iter()
        .any(|cookie| cookie.name == SESSION_COOKIE && !cookie.value.trim().is_empty());
    if !has_session {
        return Err(no_session());
    }
    Ok(CookieExtractor::build_cookie_header(&cookies))
}

fn no_session() -> ProviderError {
    ProviderError::Other(NO_DESKTOP_SESSION_MESSAGE.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::browser::detection::BrowserProfile;

    fn cookie(name: &str, value: &str) -> Cookie {
        Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain: ".claude.ai".to_string(),
            path: "/".to_string(),
            expires: None,
            is_secure: true,
            is_http_only: true,
        }
    }

    fn fake_store(dir: &Path) -> DetectedBrowser {
        DetectedBrowser {
            browser_type: BrowserType::ClaudeDesktop,
            user_data_dir: dir.to_path_buf(),
            profiles: vec![BrowserProfile {
                name: "Default".into(),
                path: dir.to_path_buf(),
                is_default: true,
            }],
        }
    }

    fn settings_with(enabled: bool, cookie_source: &str) -> Settings {
        let mut settings = Settings::default();
        settings.set_claude_desktop_cookies_enabled(enabled);
        settings.set_cookie_source(crate::core::ProviderId::Claude, cookie_source);
        settings
    }

    #[test]
    fn desktop_cookies_apply_only_to_the_automatic_cookie_source() {
        assert!(selected(&settings_with(true, "auto")));
        assert!(selected(&settings_with(true, "browser")));
        assert!(!selected(&settings_with(true, "manual")));
        assert!(!selected(&settings_with(true, "off")));
        assert!(!selected(&settings_with(false, "auto")));
        assert!(!selected(&Settings::default()));
    }

    #[test]
    fn session_header_includes_every_claude_cookie() {
        let header = session_cookie_header_from(Some(fake_store(Path::new("x"))), |_, domain| {
            assert_eq!(domain, "claude.ai");
            Ok(vec![
                cookie("sessionKey", "sk-ant-sid01-test"),
                cookie("lastActiveOrg", "org"),
            ])
        })
        .unwrap();
        assert_eq!(header, "sessionKey=sk-ant-sid01-test; lastActiveOrg=org");
    }

    #[test]
    fn missing_store_or_session_cookie_reports_the_desktop_sign_in() {
        for result in [
            session_cookie_header_from(None, |_, _| unreachable!("no store to read")),
            session_cookie_header_from(Some(fake_store(Path::new("x"))), |_, _| {
                Ok(vec![cookie("lastActiveOrg", "org")])
            }),
            session_cookie_header_from(Some(fake_store(Path::new("x"))), |_, _| {
                Ok(vec![cookie("sessionKey", "  ")])
            }),
            session_cookie_header_from(Some(fake_store(Path::new("x"))), |_, _| {
                Err(CookieError::NotFound("claude.ai".into()))
            }),
        ] {
            let message = result.unwrap_err().to_string();
            assert_eq!(message, NO_DESKTOP_SESSION_MESSAGE);
        }
    }

    #[test]
    fn app_bound_encryption_gets_a_desktop_specific_message() {
        let error = session_cookie_header_from(Some(fake_store(Path::new("x"))), |_, _| {
            Err(CookieError::AppBoundEncryption)
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("Claude desktop app"), "{error}");
        assert!(error.contains("App-Bound Encryption"), "{error}");
        assert!(!error.contains("Firefox"), "{error}");
    }

    /// End to end through the real extractor: a synthetic Electron store with
    /// a DPAPI-protected Local State key, a `v10` session cookie and a
    /// cookie for an unrelated host.
    #[cfg(windows)]
    #[test]
    fn synthetic_desktop_store_decrypts_the_session_cookie() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("Claude");
        let key = [5_u8; 32];
        write_local_state(&store, &key);
        let db = create_cookie_db(&store);
        insert_cookie(
            &db,
            ".claude.ai",
            "sessionKey",
            &encrypt_v10(&key, b"sk-ant-sid01-synthetic"),
        );
        insert_cookie(
            &db,
            "example.com",
            "sessionKey",
            &encrypt_v10(&key, b"other-site"),
        );
        drop(db);

        let header = session_cookie_header_from(
            Some(fake_store(&store)),
            CookieExtractor::extract_for_domain,
        )
        .unwrap();
        assert_eq!(header, "sessionKey=sk-ant-sid01-synthetic");
    }

    #[cfg(windows)]
    #[test]
    fn synthetic_v20_desktop_store_reports_app_bound_encryption() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("Claude");
        write_local_state(&store, &[5_u8; 32]);
        let db = create_cookie_db(&store);
        let mut v20 = b"v20".to_vec();
        v20.extend_from_slice(&[0x42; 48]);
        insert_cookie(&db, ".claude.ai", "sessionKey", &v20);
        drop(db);

        let error = session_cookie_header_from(
            Some(fake_store(&store)),
            CookieExtractor::extract_for_domain,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), CLAUDE_DESKTOP_APP_BOUND_MESSAGE);
    }

    #[cfg(windows)]
    fn write_local_state(store: &Path, key: &[u8]) {
        use base64::Engine;

        std::fs::create_dir_all(store).unwrap();
        let mut wrapped = b"DPAPI".to_vec();
        wrapped.extend(CookieExtractor::dpapi_protect_for_tests(key));
        let encoded = base64::engine::general_purpose::STANDARD.encode(wrapped);
        std::fs::write(
            store.join("Local State"),
            format!(r#"{{"os_crypt":{{"encrypted_key":"{encoded}"}}}}"#),
        )
        .unwrap();
    }

    #[cfg(windows)]
    fn create_cookie_db(store: &Path) -> rusqlite::Connection {
        let network = store.join("Network");
        std::fs::create_dir_all(&network).unwrap();
        let path: PathBuf = network.join("Cookies");
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE cookies (
                host_key TEXT NOT NULL,
                name TEXT NOT NULL,
                encrypted_value BLOB NOT NULL,
                path TEXT NOT NULL,
                expires_utc INTEGER NOT NULL,
                is_secure INTEGER NOT NULL,
                is_httponly INTEGER NOT NULL
            );",
        )
        .unwrap();
        db
    }

    #[cfg(windows)]
    fn insert_cookie(db: &rusqlite::Connection, host: &str, name: &str, encrypted: &[u8]) {
        db.execute(
            "INSERT INTO cookies VALUES (?1, ?2, ?3, '/', 0, 1, 1)",
            rusqlite::params![host, name, encrypted],
        )
        .unwrap();
    }

    #[cfg(windows)]
    fn encrypt_v10(key: &[u8], plain: &[u8]) -> Vec<u8> {
        use aes_gcm::aead::{Aead, KeyInit};

        let nonce = [3_u8; 12];
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(key).unwrap();
        let mut encrypted = b"v10".to_vec();
        encrypted.extend_from_slice(&nonce);
        encrypted.extend(
            cipher
                .encrypt(aes_gcm::Nonce::from_slice(&nonce), plain)
                .unwrap(),
        );
        encrypted
    }
}
