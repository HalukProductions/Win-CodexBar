use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const CREDITS_OK_BODY: &str = r#"{"config":{"creditUsagePercent":12.5,"currentPeriod":{"start":"2026-01-01T00:00:00Z","end":"2099-02-01T00:00:00Z"}},"subscriptionTier":"SuperGrok"}"#;

fn cli_answer() -> BearerBilling {
    BearerBilling {
        billing: GrokBillingSnapshot {
            used_percent: Some(64.0),
            used_percent_is_wire_published: true,
            used_percent_is_implicit_zero: false,
            resets_at: None,
            window_minutes: None,
            product_usage: Vec::new(),
        },
        subscription_tier: Some("SuperGrok Heavy".to_string()),
        prepaid_balance_usd: None,
    }
}

/// A fake CLI runner that counts how often it is asked.
fn counting_cli(
    answer: fn() -> Result<BearerBilling, ProviderError>,
) -> (
    Arc<AtomicUsize>,
    impl Fn() -> Result<BearerBilling, ProviderError> + Send + Sync + 'static,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    (calls, move || {
        counter.fetch_add(1, Ordering::SeqCst);
        answer()
    })
}

async fn provider_with_bearer_status(
    server: &mut mockito::ServerGuard,
    status: usize,
) -> GrokProvider {
    server
        .mock("GET", "/credits")
        .with_status(status)
        .create_async()
        .await;
    server
        .mock("POST", "/billing")
        .with_status(status)
        .create_async()
        .await;
    GrokProvider::new()
        .with_billing_endpoint_for_tests(format!("{}/billing", server.url()))
        .with_credits_proxy_endpoint_for_tests(format!("{}/credits", server.url()))
}

#[tokio::test]
async fn successful_bearer_billing_never_asks_the_cli() {
    let mut server = mockito::Server::new_async().await;
    let credits = server
        .mock("GET", "/credits")
        .with_status(200)
        .with_body(CREDITS_OK_BODY)
        .create_async()
        .await;
    let grpc = server
        .mock("POST", "/billing")
        .expect(0)
        .create_async()
        .await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new()
        .with_billing_endpoint_for_tests(format!("{}/billing", server.url()))
        .with_credits_proxy_endpoint_for_tests(format!("{}/credits", server.url()))
        .with_cli_billing_for_tests(run);

    let bearer = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap();

    credits.assert_async().await;
    grpc.assert_async().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(bearer.billing.used_percent, Some(12.5));
}

#[tokio::test]
async fn failed_bearer_billing_falls_back_to_the_cli_once() {
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 500).await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = provider.with_cli_billing_for_tests(run);

    let bearer = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(bearer.billing.used_percent, Some(64.0));
    assert_eq!(bearer.subscription_tier.as_deref(), Some("SuperGrok Heavy"));
}

#[tokio::test]
async fn auth_required_on_the_bearer_path_also_falls_back_to_the_cli() {
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 401).await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = provider.with_cli_billing_for_tests(run);

    let bearer = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(bearer.billing.used_percent, Some(64.0));
}

#[tokio::test]
async fn failing_cli_fallback_keeps_the_bearer_error_for_auto_mode() {
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 500).await;
    // The CLI's own auth error must not replace the bearer error.
    let (calls, run) = counting_cli(|| Err(ProviderError::AuthRequired));
    let provider = provider.with_cli_billing_for_tests(run);

    let error = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(&error, ProviderError::Other(message) if message.contains("status 500")),
        "{error}"
    );
}

#[tokio::test]
async fn isolated_token_account_never_reaches_the_cli() {
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 500).await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = provider.with_cli_billing_for_tests(run);
    let ctx = FetchContext {
        token_account_isolated: true,
        ..FetchContext::default()
    };

    let error = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &ctx,
        )
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(error, ProviderError::Other(_)), "{error}");
}

#[tokio::test]
async fn oauth_principal_never_reaches_the_cli() {
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 401).await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = provider.with_cli_billing_for_tests(run);

    let error = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::OAuth,
            &FetchContext::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(error, ProviderError::AuthRequired), "{error}");
}

#[test]
fn saved_account_rows_are_isolated_from_the_ambient_cli_login() {
    assert!(saved_account_usage_context().token_account_isolated);
}

#[tokio::test]
async fn saved_account_row_failure_never_reaches_the_ambient_cli() {
    // A saved row's auth text may name another account than the one switched
    // into ~/.grok, so a failed row must not borrow the ambient login over ACP.
    let mut server = mockito::Server::new_async().await;
    let provider = provider_with_bearer_status(&mut server, 500).await;
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = provider.with_cli_billing_for_tests(run);
    let saved_row =
        r#"{"https://accounts.x.ai/sign-in": {"key": "saved-cli-token", "auth_mode": "session"}}"#;

    let error = provider
        .fetch_usage_from_auth_json(saved_row)
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(error, ProviderError::Other(_)), "{error}");
}
