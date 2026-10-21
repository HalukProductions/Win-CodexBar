//! Machine-local configuration for the app-managed HTTP usage server
//! (upstream 0.77.0 #4553 `HTTPServerConfiguration`).
//!
//! Stored in `http_server.json` beside the settings file, written through
//! [`crate::secure_file`] (DPAPI on Windows) because it holds the dashboard
//! token. It is deliberately not part of `settings.json`, so it never enters
//! the portable preferences export.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::serve::ServeArgs;

pub const HTTP_SERVER_CONFIG_FILE: &str = "http_server.json";

pub const HTTP_SERVER_IDENTITY_REDACTED: &str = "redacted";
pub const HTTP_SERVER_IDENTITY_FULL: &str = "full";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HttpServerConfig {
    pub enabled: bool,
    pub host: String,
    /// Wider than `u16` so an out-of-range value reaches validation instead
    /// of failing to deserialize.
    pub port: u32,
    pub token: String,
    pub refresh_interval_secs: f64,
    pub request_timeout_secs: f64,
    pub identity: String,
    pub allow_plain_http: bool,
}

// The token is a bearer secret; Debug must never print it.
impl std::fmt::Debug for HttpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServerConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("refresh_interval_secs", &self.refresh_interval_secs)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("identity", &self.identity)
            .field("allow_plain_http", &self.allow_plain_http)
            .finish()
    }
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".to_string(),
            port: 8080,
            token: String::new(),
            refresh_interval_secs: 60.0,
            request_timeout_secs: 30.0,
            identity: HTTP_SERVER_IDENTITY_REDACTED.to_string(),
            allow_plain_http: false,
        }
    }
}

/// Why a configuration cannot be started. Serialized as a stable code the UI
/// maps to a localized message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HttpServerValidationError {
    InvalidHost,
    InvalidPort,
    InvalidTiming,
    InvalidIdentity,
    InvalidToken,
    PlainHttpNotAccepted,
}

impl HttpServerConfig {
    pub fn default_path() -> Option<PathBuf> {
        super::config_store_dir().map(|dir| dir.join(HTTP_SERVER_CONFIG_FILE))
    }

    pub fn bind_host(&self) -> String {
        let trimmed = self.host.trim();
        if trimmed.eq_ignore_ascii_case("localhost") {
            "127.0.0.1".to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub fn is_loopback(&self) -> bool {
        self.bind_host().starts_with("127.")
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}/", self.bind_host(), self.port)
    }

    pub fn validation_error(&self) -> Option<HttpServerValidationError> {
        if !is_canonical_ipv4(&self.bind_host()) {
            return Some(HttpServerValidationError::InvalidHost);
        }
        if !(1..=65_535).contains(&self.port) {
            return Some(HttpServerValidationError::InvalidPort);
        }
        let timing_ok = |secs: f64| secs.is_finite() && secs >= 0.0;
        if !timing_ok(self.refresh_interval_secs) || !timing_ok(self.request_timeout_secs) {
            return Some(HttpServerValidationError::InvalidTiming);
        }
        if self.identity != HTTP_SERVER_IDENTITY_REDACTED
            && self.identity != HTTP_SERVER_IDENTITY_FULL
        {
            return Some(HttpServerValidationError::InvalidIdentity);
        }
        if self.token.is_empty() || !self.token.bytes().all(|b| b > 32 && b < 127) {
            return Some(HttpServerValidationError::InvalidToken);
        }
        if !self.is_loopback() && !self.allow_plain_http {
            return Some(HttpServerValidationError::PlainHttpNotAccepted);
        }
        None
    }

    /// Arguments for the in-process server. The token travels in memory only;
    /// it never appears on a command line or in the environment.
    pub fn serve_args(&self) -> ServeArgs {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "validated finite and nonnegative; float-to-int casts saturate"
        )]
        let refresh_interval = self.refresh_interval_secs.round() as u64;
        ServeArgs {
            port: u16::try_from(self.port).unwrap_or(0),
            host: self.bind_host(),
            refresh_interval,
            request_timeout: self.request_timeout_secs,
            dashboard_token: Some(self.token.clone()),
            metrics: false,
            allow_plain_http: self.allow_plain_http,
            identity: Some(self.identity.clone()),
            all_accounts: false,
        }
    }

    /// A missing file means defaults; an unreadable or corrupt file is an
    /// error and is left untouched.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = crate::secure_file::read_string(path)?;
        serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        crate::secure_file::write_string(path, &json)
    }
}

fn is_canonical_ipv4(host: &str) -> bool {
    let parts: Vec<_> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u8>().is_ok_and(|v| v.to_string() == *part)
        })
}

#[cfg(test)]
mod tests;
