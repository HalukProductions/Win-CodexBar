//! Fixed-text explanation for why live Antigravity usage fell back to offline
//! conversation history.
//!
//! Port of upstream `AntigravityOfflineFetchStrategy.diagnostic(forPriorFailure:)`
//! (CodexBar v0.65.0, #3865). A failed live probe carries a typed
//! [`LiveFailureReason`] next to the [`ProviderError`] it surfaces when no
//! offline history exists. The reason is chosen where the failure happens and
//! the error's message is never read for it: messages can embed local paths,
//! URLs, response bodies or account details, so they must not reach a display
//! surface.

use std::fmt;

use crate::core::{ProviderDisplayDetail, ProviderError};

use super::AGY_NOT_FOUND_MESSAGE;
use super::cli_print_failure::CliPrintFailure;

const DETAIL_ID: &str = "antigravity-live-unavailable";
const DETAIL_TITLE: &str = "Live usage";
const DETAIL_PREFIX: &str = "Live Antigravity usage is unavailable; showing offline data.";
const GENERIC_HINT: &str = "check Diagnostics for per-source details";

/// Sanitized category of a failed live probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveFailureReason {
    /// Neither a desktop language server nor an `agy` CLI was found.
    NotRunning,
    TimedOut,
    /// The local language server rejected the request (HTTP 401/403).
    SessionExpired,
    HttpStatus(u16),
    /// The local language server refused or dropped the connection.
    CannotConnect,
    CliReport(CliPrintFailure),
    /// The Google OAuth remote quota fallback failed.
    Remote(RemoteFailureKind),
    /// Parse, port-detection and other transport failures, plus any error
    /// that was not classified where it happened.
    Unclassified,
}

/// Sanitized category of a failed remote OAuth fetch. Only sign-in guidance is
/// specific; API, parse and transport failures can carry response bodies, so
/// they reduce to one fixed hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteFailureKind {
    NotLoggedIn,
    SessionExpired,
    ReauthenticationRequired,
    RequestFailed,
}

impl LiveFailureReason {
    fn text(self) -> String {
        match self {
            Self::NotRunning => AGY_NOT_FOUND_MESSAGE.to_string(),
            Self::TimedOut => "Antigravity quota request timed out.".to_string(),
            Self::SessionExpired => {
                "Antigravity session expired. Restart Antigravity and retry.".to_string()
            }
            Self::HttpStatus(status) => format!("the usage request failed (HTTP {status})"),
            Self::CannotConnect => "Could not connect to the server.".to_string(),
            Self::CliReport(failure) => failure.description(),
            Self::Remote(kind) => match kind {
                RemoteFailureKind::NotLoggedIn => super::remote::NOT_LOGGED_IN_MESSAGE.to_string(),
                RemoteFailureKind::SessionExpired => {
                    super::remote::SESSION_EXPIRED_MESSAGE.to_string()
                }
                RemoteFailureKind::ReauthenticationRequired => {
                    super::remote::REAUTHENTICATION_REQUIRED_MESSAGE.to_string()
                }
                RemoteFailureKind::RequestFailed => {
                    "the Antigravity API request failed".to_string()
                }
            },
            Self::Unclassified => GENERIC_HINT.to_string(),
        }
    }
}

/// A failed live Antigravity probe: the error surfaced when no offline history
/// exists, plus the fixed-text reason shown next to offline data.
#[derive(Debug)]
pub(super) struct LiveFailure {
    error: ProviderError,
    reason: LiveFailureReason,
}

impl LiveFailure {
    fn new(error: ProviderError, reason: LiveFailureReason) -> Self {
        Self { error, reason }
    }

    /// No live runtime answered and no `agy` executable was found.
    pub(super) fn not_running() -> Self {
        Self::new(
            ProviderError::NotInstalled(AGY_NOT_FOUND_MESSAGE.into()),
            LiveFailureReason::NotRunning,
        )
    }

    /// The local language server answered with a non-success HTTP status.
    pub(super) fn http_status(status: u16, error: ProviderError) -> Self {
        let reason = match status {
            401 | 403 => LiveFailureReason::SessionExpired,
            _ => LiveFailureReason::HttpStatus(status),
        };
        Self::new(error, reason)
    }

    /// A local language-server request failed before an HTTP status arrived.
    ///
    /// Like upstream's `URLError(code)` reduction, only the transport category
    /// survives: the error text can carry the request URL. A timeout reuses
    /// the fixed quota-timeout text; a refused connection uses the text
    /// upstream shows for `URLError.cannotConnectToHost`.
    pub(super) fn request(context: &str, error: &reqwest::Error) -> Self {
        let reason = if error.is_timeout() {
            LiveFailureReason::TimedOut
        } else if error.is_connect() {
            LiveFailureReason::CannotConnect
        } else {
            LiveFailureReason::Unclassified
        };
        Self::new(ProviderError::Other(format!("{context}: {error}")), reason)
    }

    /// A classified `agy` print-usage failure. The error text is the same
    /// fixed description, so raw subprocess output never reaches it.
    pub(super) fn cli_report(failure: CliPrintFailure) -> Self {
        Self::new(
            ProviderError::Other(failure.description()),
            LiveFailureReason::CliReport(failure),
        )
    }

    /// A classified remote OAuth failure.
    pub(super) fn remote(error: ProviderError, kind: RemoteFailureKind) -> Self {
        Self::new(error, LiveFailureReason::Remote(kind))
    }

    pub(super) fn is_auth_required(&self) -> bool {
        matches!(self.error, ProviderError::AuthRequired)
    }

    pub(super) fn into_error(self) -> ProviderError {
        self.error
    }

    /// Detail row explaining the failure that led to the offline snapshot.
    ///
    /// `AuthRequired` is terminal and never reaches the offline snapshot, so it
    /// yields no row.
    pub(super) fn offline_detail(&self) -> Option<ProviderDisplayDetail> {
        if self.is_auth_required() {
            return None;
        }
        ProviderDisplayDetail::new(
            DETAIL_ID,
            DETAIL_TITLE,
            format!("{DETAIL_PREFIX} {}", self.reason.text()),
        )
    }

    #[cfg(test)]
    pub(super) fn reason(&self) -> LiveFailureReason {
        self.reason
    }
}

impl From<ProviderError> for LiveFailure {
    /// Only the typed timeout is recognizable after the fact; everything else
    /// reduces to the fixed Diagnostics hint.
    fn from(error: ProviderError) -> Self {
        let reason = match error {
            ProviderError::Timeout => LiveFailureReason::TimedOut,
            _ => LiveFailureReason::Unclassified,
        };
        Self::new(error, reason)
    }
}

impl fmt::Display for LiveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

#[cfg(test)]
#[path = "offline_reason_tests.rs"]
mod tests;
