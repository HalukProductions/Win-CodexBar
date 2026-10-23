//! Auto-mode order after the Code API key: the Kimi Code CLI credential, then
//! web auth (upstream `KimiCLICredentialFetchStrategy` followed by
//! `KimiWebFetchStrategy` in the Kimi fetch plan).
//!
//! Upstream reports the error of the last strategy that was available. Web
//! auth is available only when it has a token to send, so a stale or rejected
//! CLI credential surfaces as renewal guidance only when web auth had nothing
//! to try. A rejected web token keeps its own web error.

use super::code_api::{KimiCliCredential, kimi_cli_credential_error};
use super::web::WebFetchFailure;
use crate::core::{ProviderError, ProviderFetchResult};

pub(super) async fn fetch_cli_then_web<C, CF, W, WF>(
    cli_credential: KimiCliCredential,
    fetch_cli: C,
    fetch_web: W,
) -> Result<ProviderFetchResult, ProviderError>
where
    C: FnOnce(String) -> CF,
    CF: Future<Output = Result<ProviderFetchResult, ProviderError>>,
    W: FnOnce() -> WF,
    WF: Future<Output = Result<ProviderFetchResult, WebFetchFailure>>,
{
    let cli_failure = match cli_credential {
        KimiCliCredential::Unavailable => None,
        KimiCliCredential::Stale => {
            tracing::debug!("Kimi Code CLI credential is stale; trying web auth");
            Some(kimi_cli_credential_error())
        }
        KimiCliCredential::Fresh(token) => match fetch_cli(token).await {
            Ok(fetched) => return Ok(labeled(fetched, "code-cli")),
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Kimi Code CLI credential fetch failed; trying web auth"
                );
                Some(cli_attempt_error(error))
            }
        },
    };

    match fetch_web().await {
        Ok(fetched) => Ok(labeled(fetched, "web")),
        Err(failure) => match cli_failure {
            Some(cli_error) if !failure.had_token => Err(cli_error),
            _ => Err(failure.error),
        },
    }
}

/// Stamps the Auto source that produced `fetched`. The usage and its display
/// rows are kept exactly as the source returned them.
fn labeled(mut fetched: ProviderFetchResult, source_label: &str) -> ProviderFetchResult {
    fetched.source_label = source_label.to_string();
    fetched
}

/// Upstream `normalizedCodeAPIError`: a CLI token the Code API rejects (401)
/// is reported as the renewal guidance; any other failure keeps its error.
fn cli_attempt_error(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::AuthRequired => kimi_cli_credential_error(),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use crate::core::{RateWindow, UsageSnapshot};

    const OFF_SOURCE_ERROR: &str =
        "Kimi cookie source is Off; provide a manual cookie header or enable browser import.";

    fn usage(percent: f64) -> ProviderFetchResult {
        ProviderFetchResult::new(UsageSnapshot::new(RateWindow::new(percent)), "test")
    }

    fn web_failure(
        error: ProviderError,
        had_token: bool,
    ) -> Result<ProviderFetchResult, WebFetchFailure> {
        Err(WebFetchFailure { error, had_token })
    }

    fn web_without_token() -> Result<ProviderFetchResult, WebFetchFailure> {
        web_failure(ProviderError::Other(OFF_SOURCE_ERROR.into()), false)
    }

    fn assert_renewal_guidance(error: &ProviderError) {
        let message = error.to_string();
        for expected in [
            "Run kimi",
            "Settings > Providers > Kimi",
            "KIMI_CODE_API_KEY",
            "does not refresh",
        ] {
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert!(!message.contains("synthetic-"), "{message:?} leaks a token");
    }

    struct Outcome {
        result: Result<ProviderFetchResult, ProviderError>,
        cli_tokens: Vec<String>,
        web_ran: bool,
    }

    /// Runs the Auto CLI-then-web order with scripted sources.
    async fn run(
        credential: KimiCliCredential,
        cli: Result<ProviderFetchResult, ProviderError>,
        web: Result<ProviderFetchResult, WebFetchFailure>,
    ) -> Outcome {
        let cli_tokens = RefCell::new(Vec::new());
        let web_ran = Cell::new(false);
        let result = fetch_cli_then_web(
            credential,
            |token| {
                cli_tokens.borrow_mut().push(token);
                async move { cli }
            },
            || {
                web_ran.set(true);
                async move { web }
            },
        )
        .await;
        Outcome {
            result,
            cli_tokens: cli_tokens.into_inner(),
            web_ran: web_ran.get(),
        }
    }

    // Upstream `KimiCLICredentialLifecycleTests`: stale or rejected CLI
    // credentials fall back to configured web auth without renewal.
    #[tokio::test]
    async fn stale_or_rejected_cli_credentials_fall_back_to_configured_web_auth() {
        for rejected_by_server in [false, true] {
            let credential = if rejected_by_server {
                KimiCliCredential::Fresh("api-bad".into())
            } else {
                KimiCliCredential::Stale
            };
            let outcome = run(
                credential,
                Err(ProviderError::AuthRequired),
                Ok(usage(25.0)),
            )
            .await;

            let result = outcome.result.expect("web auth takes over");
            assert_eq!(result.source_label, "web");
            assert_eq!(result.usage.primary.used_percent, 25.0);
            let expected_cli_tokens: &[&str] = if rejected_by_server {
                &["api-bad"]
            } else {
                &[]
            };
            assert_eq!(outcome.cli_tokens, expected_cli_tokens);
            assert!(outcome.web_ran);
        }
        assert_renewal_guidance(&cli_attempt_error(ProviderError::AuthRequired));
    }

    // Upstream: CLI-only Auto mode (cookie source Off) explains renewal and
    // the API key setting.
    #[tokio::test]
    async fn cli_only_auto_mode_explains_renewal_and_the_api_key_setting() {
        for (credential, cli_tokens) in [
            (KimiCliCredential::Stale, Vec::<String>::new()),
            (
                KimiCliCredential::Fresh("synthetic-rejected".into()),
                vec!["synthetic-rejected".to_string()],
            ),
        ] {
            let outcome = run(
                credential,
                Err(ProviderError::AuthRequired),
                web_without_token(),
            )
            .await;
            assert_renewal_guidance(&outcome.result.expect_err("no source works"));
            assert_eq!(outcome.cli_tokens, cli_tokens);
        }
    }

    #[tokio::test]
    async fn rejected_web_token_keeps_the_web_error() {
        let outcome = run(
            KimiCliCredential::Stale,
            Err(ProviderError::AuthRequired),
            web_failure(ProviderError::AuthRequired, true),
        )
        .await;
        assert!(matches!(outcome.result, Err(ProviderError::AuthRequired)));

        let outcome = run(
            KimiCliCredential::Fresh("api-bad".into()),
            Err(ProviderError::AuthRequired),
            web_failure(ProviderError::Other("API error: 500".into()), true),
        )
        .await;
        assert!(matches!(
            outcome.result,
            Err(ProviderError::Other(message)) if message == "API error: 500"
        ));
    }

    #[tokio::test]
    async fn other_cli_failures_are_reported_when_web_auth_has_no_token() {
        let outcome = run(
            KimiCliCredential::Fresh("cli-token".into()),
            Err(ProviderError::Timeout),
            web_without_token(),
        )
        .await;
        assert!(matches!(outcome.result, Err(ProviderError::Timeout)));

        let denied = "Kimi Code API returned status 403 Forbidden (permission or quota denied)";
        let outcome = run(
            KimiCliCredential::Fresh("cli-token".into()),
            Err(ProviderError::Other(denied.into())),
            web_without_token(),
        )
        .await;
        assert!(matches!(
            outcome.result,
            Err(ProviderError::Other(message)) if message == denied
        ));
    }

    #[tokio::test]
    async fn unavailable_cli_credential_reports_the_web_error() {
        let outcome = run(
            KimiCliCredential::Unavailable,
            Ok(usage(10.0)),
            web_without_token(),
        )
        .await;
        assert!(matches!(
            outcome.result,
            Err(ProviderError::Other(message)) if message == OFF_SOURCE_ERROR
        ));
        assert!(outcome.cli_tokens.is_empty());
        assert!(outcome.web_ran);
    }

    // Upstream: the next fetch recovers once the CLI replaces its credential
    // (the file side is covered in `code_api`).
    #[tokio::test]
    async fn fresh_cli_credential_is_used_before_web_auth() {
        let outcome = run(
            KimiCliCredential::Fresh("cli-ok".into()),
            Ok(usage(25.0)),
            web_without_token(),
        )
        .await;
        let result = outcome.result.expect("CLI credential accepted");
        assert_eq!(result.source_label, "code-cli");
        assert_eq!(result.usage.primary.used_percent, 25.0);
        assert_eq!(outcome.cli_tokens, ["cli-ok"]);
        assert!(!outcome.web_ran);
    }
}
