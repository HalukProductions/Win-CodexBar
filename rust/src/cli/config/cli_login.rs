//! `config login` for the Grok and Kiro CLIs.
//!
//! Each flow runs the provider's own sign-in command and waits for it to
//! finish. Grok's credential is saved to the CodexBar account store, the same
//! way the desktop sign-in saves it. Kiro keeps its credential inside
//! `kiro-cli`, so for Kiro "saved" means the CLI reported a successful
//! sign-in. Neither flow prints a token or the CLI's own output.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::login::{
    ConfigLoginError, ConfigLoginEvent, LoginFailure, cancelled, ctrl_c, report_outcome,
    write_event,
};
use crate::core::ProviderId;
use crate::login::{LoginOutcome, LoginPhase, run_kiro_login_with};
use crate::providers::grok::accounts as grok_accounts;

/// How long a stopped CLI gets to exit before the Ctrl-C path returns.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// How long a Kiro sign-in may wait for the browser before it is stopped. The
/// desktop sign-in uses the same 120 seconds.
const KIRO_TIMEOUT_SECS: u64 = 120;

/// Runs `config login --provider grok`. The provider is turned on only after
/// the credential is saved.
pub async fn login_grok(json: bool, out: &mut impl Write) -> Result<(), ConfigLoginError> {
    let result = grok_sign_in(json, out).await;
    let result = result
        .and_then(|()| super::enable_provider(ProviderId::Grok).map_err(ConfigLoginError::other));
    report_outcome(json, out, result)
}

/// Runs `config login --provider kiro`. The provider is turned on only after
/// `kiro-cli` reports a successful sign-in.
pub async fn login_kiro(json: bool, out: &mut impl Write) -> Result<(), ConfigLoginError> {
    let result = kiro_sign_in(json, out).await;
    let result = result
        .and_then(|()| super::enable_provider(ProviderId::Kiro).map_err(ConfigLoginError::other));
    report_outcome(json, out, result)
}

async fn grok_sign_in(json: bool, out: &mut impl Write) -> Result<(), ConfigLoginError> {
    write_event(out, json, &ConfigLoginEvent::pending_browser())?;
    grok_accounts::begin_login();
    // Folders left by an earlier crashed login are removed. A failure here
    // does not stop this login.
    let _cleanup = grok_accounts::cleanup_abandoned_logins();

    let mut task = tokio::task::spawn_blocking(grok_accounts::login);
    let joined = tokio::select! {
        biased;
        () = ctrl_c() => None,
        joined = &mut task => Some(joined),
    };
    let Some(joined) = joined else {
        grok_accounts::cancel_login();
        // The blocking poll loop sees the flag within about 150 ms and stops
        // the CLI. The wait is bounded so a stuck thread cannot hang Ctrl-C.
        let _stopped = tokio::time::timeout(STOP_WAIT, task).await;
        return Err(cancelled());
    };
    let saved = joined
        .map_err(ConfigLoginError::other)?
        .map_err(grok_login_error)?;
    grok_accounts::save_login(saved)
        .await
        .map_err(ConfigLoginError::other)
}

async fn kiro_sign_in(json: bool, out: &mut impl Write) -> Result<(), ConfigLoginError> {
    write_event(out, json, &ConfigLoginEvent::pending_browser())?;

    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    // The sign-in link is printed as soon as the CLI shows it. The callback
    // runs on the login thread, so it writes to stdout itself.
    let on_link = move |link: &str| {
        let mut stdout = io::stdout();
        if let Err(error) = write_event(&mut stdout, json, &ConfigLoginEvent::pending_url(link)) {
            tracing::warn!("Could not write the Kiro sign-in link: {error}");
        }
    };
    let mut task = tokio::task::spawn_blocking(move || {
        run_kiro_login_with(
            KIRO_TIMEOUT_SECS,
            worker_cancel,
            |_: LoginPhase| {},
            on_link,
        )
    });
    let joined = tokio::select! {
        biased;
        () = ctrl_c() => None,
        joined = &mut task => Some(joined),
    };
    let Some(joined) = joined else {
        cancel.store(true, Ordering::SeqCst);
        // The watchdog stops the CLI within about 100 ms. The wait is bounded
        // so a stuck pipe cannot hang Ctrl-C.
        let _stopped = tokio::time::timeout(STOP_WAIT, task).await;
        return Err(cancelled());
    };
    let result = joined.map_err(ConfigLoginError::other)?;
    kiro_outcome(result.outcome)
}

/// Maps a Grok sign-in error to a `config login` failure. A missing CLI has
/// its own exit code, and a sign-in that ran out of time counts as expired.
fn grok_login_error(error: io::Error) -> ConfigLoginError {
    let failure = match error.kind() {
        io::ErrorKind::NotFound => LoginFailure::MissingBinary,
        io::ErrorKind::TimedOut => LoginFailure::Expired,
        _ => LoginFailure::Other,
    };
    ConfigLoginError::new(failure, error.to_string())
}

/// Maps the Kiro CLI outcome to a `config login` result. Only a successful
/// sign-in is `Ok`.
fn kiro_outcome(outcome: LoginOutcome) -> Result<(), ConfigLoginError> {
    match outcome {
        LoginOutcome::Success => Ok(()),
        LoginOutcome::MissingBinary => Err(ConfigLoginError::new(
            LoginFailure::MissingBinary,
            "Kiro CLI (kiro-cli) was not found. Install Kiro CLI, then try again.",
        )),
        LoginOutcome::TimedOut => Err(ConfigLoginError::new(
            LoginFailure::Expired,
            "Kiro sign-in timed out. Please try again.",
        )),
        LoginOutcome::Cancelled => Err(cancelled()),
        LoginOutcome::Failed { status } => Err(ConfigLoginError::other(format!(
            "Kiro sign-in did not complete (exit code {status})."
        ))),
        LoginOutcome::LaunchFailed(error) => Err(ConfigLoginError::other(format!(
            "Could not start kiro-cli: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_missing_binary_maps_to_missing_binary() {
        let error = grok_login_error(io::Error::new(
            io::ErrorKind::NotFound,
            "Grok CLI was not found.",
        ));
        assert_eq!(error.failure(), LoginFailure::MissingBinary);
        assert_eq!(error.to_string(), "Grok CLI was not found.");
    }

    #[test]
    fn grok_timeout_maps_to_expired() {
        let error = grok_login_error(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        assert_eq!(error.failure(), LoginFailure::Expired);
    }

    #[test]
    fn other_grok_errors_are_generic_failures() {
        let error = grok_login_error(io::Error::other("disk full"));
        assert_eq!(error.failure(), LoginFailure::Other);
    }

    #[test]
    fn kiro_success_is_ok() {
        assert!(kiro_outcome(LoginOutcome::Success).is_ok());
    }

    #[test]
    fn kiro_missing_binary_maps_to_missing_binary() {
        let error = kiro_outcome(LoginOutcome::MissingBinary).expect_err("missing");
        assert_eq!(error.failure(), LoginFailure::MissingBinary);
    }

    #[test]
    fn kiro_timeout_maps_to_expired() {
        let error = kiro_outcome(LoginOutcome::TimedOut).expect_err("timed out");
        assert_eq!(error.failure(), LoginFailure::Expired);
    }

    #[test]
    fn kiro_cancel_maps_to_cancelled() {
        let error = kiro_outcome(LoginOutcome::Cancelled).expect_err("cancelled");
        assert_eq!(error.failure(), LoginFailure::Cancelled);
    }

    #[test]
    fn kiro_nonzero_exit_is_a_generic_failure() {
        let error = kiro_outcome(LoginOutcome::Failed { status: 3 }).expect_err("failed");
        assert_eq!(error.failure(), LoginFailure::Other);
        assert!(error.to_string().contains("exit code 3"));
    }

    #[test]
    fn kiro_launch_failure_is_a_generic_failure() {
        let error =
            kiro_outcome(LoginOutcome::LaunchFailed("denied".to_string())).expect_err("launch");
        assert_eq!(error.failure(), LoginFailure::Other);
    }
}
