//! Shared pieces of `codexbar config login`: the JSON event shape, the typed
//! failure with its exit code, and the reporter that writes the final event.
//!
//! Each provider's login writes its own progress events, then hands its
//! result to [`report_outcome`]. Tokens and secrets never reach these types.

use std::fmt::Display;
use std::io::Write;

use serde::Serialize;

use crate::cli::exit_codes;
use crate::providers::copilot::device_flow::DeviceFlowError;
use crate::providers::copilot::login::CopilotLoginError;

/// Why `config login` did not finish. Each variant has one exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFailure {
    /// Bad provider or an unsupported provider for `config login`.
    Usage,
    /// The user denied the request (`access_denied`).
    Denied,
    /// The device code or sign-in window expired before approval.
    Expired,
    /// Stopped with Ctrl-C before the credential was saved.
    Cancelled,
    /// The provider's CLI binary is not installed.
    MissingBinary,
    /// Any other failure (network, GitHub error, store or settings write).
    Other,
}

impl LoginFailure {
    /// Process exit code for this failure. Success is exit code 0.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Usage => exit_codes::USAGE_ERROR,
            Self::Denied => exit_codes::LOGIN_DENIED,
            Self::Expired => exit_codes::LOGIN_EXPIRED,
            Self::Cancelled => exit_codes::LOGIN_CANCELLED,
            Self::MissingBinary => exit_codes::LOGIN_MISSING_BINARY,
            Self::Other => exit_codes::UNEXPECTED_FAILURE,
        }
    }

    /// Value of the `reason` field on a `failed` event.
    fn reason(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::MissingBinary => "missing_binary",
            Self::Other => "error",
        }
    }
}

/// A `config login` failure. `main` reads [`ConfigLoginError::exit_code`] to
/// pick the process exit code.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ConfigLoginError {
    failure: LoginFailure,
    message: String,
}

impl ConfigLoginError {
    pub fn new(failure: LoginFailure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Display) -> Self {
        Self::new(LoginFailure::Usage, message.to_string())
    }

    pub fn other(error: impl Display) -> Self {
        Self::new(LoginFailure::Other, error.to_string())
    }

    pub fn failure(&self) -> LoginFailure {
        self.failure
    }

    pub fn exit_code(&self) -> i32 {
        self.failure.exit_code()
    }
}

impl From<CopilotLoginError> for ConfigLoginError {
    fn from(error: CopilotLoginError) -> Self {
        let failure = match &error {
            CopilotLoginError::Flow(DeviceFlowError::AccessDenied) => LoginFailure::Denied,
            CopilotLoginError::Flow(DeviceFlowError::ExpiredToken) => LoginFailure::Expired,
            _ => LoginFailure::Other,
        };
        Self::new(failure, error.to_string())
    }
}

/// One `config login` event (upstream `ConfigLoginEvent`). It carries the
/// verification URL and user code only; the token is never part of it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ConfigLoginEvent {
    pub status: &'static str,
    #[serde(rename = "verificationURI", skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
    #[serde(rename = "userCode", skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(rename = "expiresIn", skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ConfigLoginEvent {
    pub fn status(status: &'static str) -> Self {
        Self {
            status,
            verification_uri: None,
            user_code: None,
            expires_in: None,
            reason: None,
            error: None,
        }
    }

    /// The `authorization_pending` event that shows the URL and code.
    pub fn pending(verification_uri: &str, user_code: &str, expires_in: u32) -> Self {
        Self {
            verification_uri: Some(verification_uri.to_string()),
            user_code: Some(user_code.to_string()),
            expires_in: Some(expires_in),
            ..Self::status("authorization_pending")
        }
    }

    /// The `failed` event. It carries the failure reason and message only.
    pub fn failed(error: &ConfigLoginError) -> Self {
        Self {
            reason: Some(error.failure().reason()),
            error: Some(error.to_string()),
            ..Self::status("failed")
        }
    }
}

/// Writes one event. JSON mode prints one object per line. Text mode prints
/// the URL and code for `authorization_pending` and `Connected` for
/// `authorized`; other events print nothing and their error goes to stderr.
pub fn write_event(
    out: &mut impl Write,
    json: bool,
    event: &ConfigLoginEvent,
) -> Result<(), ConfigLoginError> {
    if json {
        let line = serde_json::to_string(event).map_err(ConfigLoginError::other)?;
        writeln!(out, "{line}").map_err(ConfigLoginError::other)?;
    } else if let (Some(url), Some(code)) = (&event.verification_uri, &event.user_code) {
        writeln!(out, "{url}\n{code}").map_err(ConfigLoginError::other)?;
    } else if event.status == "authorized" {
        writeln!(out, "Connected").map_err(ConfigLoginError::other)?;
    }
    Ok(())
}

/// Future that resolves on Ctrl-C. If the handler cannot be installed, it
/// never resolves, so a broken signal setup cannot cancel a login.
pub async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Writes the final event for `result` and returns the result unchanged.
///
/// A success writes `authorized` only after the credential is saved. A Ctrl-C
/// writes `cancelled`. Any other failure writes a `failed` event with its
/// reason in JSON mode; text mode prints the error on stderr from the caller.
/// A failed write to stdout is logged and does not change the result, so
/// exit code 0 still means the credential was saved.
pub fn report_outcome<T>(
    json: bool,
    out: &mut impl Write,
    result: Result<T, ConfigLoginError>,
) -> Result<T, ConfigLoginError> {
    let event = match &result {
        Ok(_) => Some(ConfigLoginEvent::status("authorized")),
        Err(error) if error.failure() == LoginFailure::Cancelled => {
            Some(ConfigLoginEvent::status("cancelled"))
        }
        Err(error) if json => Some(ConfigLoginEvent::failed(error)),
        Err(_) => None,
    };
    if let Some(event) = event
        && let Err(error) = write_event(out, json, &event)
    {
        tracing::warn!("Could not write the config login result: {error}");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_has_its_own_exit_code() {
        assert_eq!(LoginFailure::Usage.exit_code(), exit_codes::USAGE_ERROR);
        assert_eq!(LoginFailure::Denied.exit_code(), 5);
        assert_eq!(LoginFailure::Expired.exit_code(), 6);
        assert_eq!(LoginFailure::Cancelled.exit_code(), 7);
        assert_eq!(LoginFailure::MissingBinary.exit_code(), 8);
        assert_eq!(
            LoginFailure::Other.exit_code(),
            exit_codes::UNEXPECTED_FAILURE
        );
    }

    #[test]
    fn failed_event_carries_reason_and_message_only() {
        let error = ConfigLoginError::new(LoginFailure::Denied, "GitHub said no");
        let json = serde_json::to_value(ConfigLoginEvent::failed(&error)).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "status": "failed",
                "reason": "denied",
                "error": "GitHub said no",
            })
        );
    }

    #[test]
    fn report_outcome_writes_cancelled_for_ctrl_c_and_returns_the_error() {
        let error = ConfigLoginError::new(LoginFailure::Cancelled, "Login cancelled.");
        let mut out = Vec::new();
        let result: Result<(), _> = report_outcome(true, &mut out, Err(error));
        assert_eq!(result.expect_err("cancelled").exit_code(), 7);
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "{\"status\":\"cancelled\"}\n"
        );
    }

    #[test]
    fn report_outcome_text_failure_prints_nothing_on_stdout() {
        let error = ConfigLoginError::new(LoginFailure::Expired, "Device code expired");
        let mut out = Vec::new();
        let result: Result<(), _> = report_outcome(false, &mut out, Err(error));
        assert!(result.is_err());
        assert!(out.is_empty());
    }
}
