use super::tests::billing_response_with_percent;
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

fn missing_login() -> ProviderError {
    ProviderError::NotInstalled("Grok auth.json not found. Run `grok login`.".to_string())
}

fn expired_login() -> ProviderError {
    ProviderError::AuthRequired
}

#[test]
fn expired_cli_login_in_auth_json_is_auth_required() {
    // The real expired-login error must qualify for the ACP fallback.
    let text = r#"{"https://accounts.x.ai/sign-in": {"key": "cli-token", "auth_mode": "session", "expires_at": "2000-01-01T00:00:00Z"}}"#;
    let error = GrokCredentials::parse_for_kind(text, GrokAuthKind::Cli).unwrap_err();

    assert!(matches!(error, ProviderError::AuthRequired), "{error}");
    assert!(is_missing_or_expired_login(&error));
}

#[tokio::test]
async fn missing_cli_login_is_answered_by_the_cli() {
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let result = provider
        .fetch_cli_acp_without_login(missing_login(), &FetchContext::default())
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.usage.primary.used_percent, 64.0);
    assert_eq!(result.source_label, "grok-cli");
    assert_eq!(result.usage.account_email, None);
    assert_eq!(
        result.usage.login_method.as_deref(),
        Some("SuperGrok Heavy")
    );
}

#[tokio::test]
async fn expired_cli_login_is_answered_by_the_cli() {
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let result = provider
        .fetch_cli_acp_without_login(expired_login(), &FetchContext::default())
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.usage.primary.used_percent, 64.0);
}

#[tokio::test]
async fn auto_mode_uses_the_cli_when_the_login_is_missing_or_expired() {
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let missing = provider
        .try_ambient_loaded(
            GrokAuthKind::Cli,
            Err(missing_login()),
            &FetchContext::default(),
        )
        .await
        .expect("auto step answers")
        .unwrap();
    let expired = provider
        .try_ambient_loaded(
            GrokAuthKind::Cli,
            Err(expired_login()),
            &FetchContext::default(),
        )
        .await
        .expect("auto step answers")
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(missing.usage.primary.used_percent, 64.0);
    assert_eq!(expired.usage.primary.used_percent, 64.0);
}

#[tokio::test]
async fn failing_cli_acp_keeps_the_original_missing_login_error() {
    // The CLI's own auth error must not replace the friendly login error.
    let (calls, run) = counting_cli(|| Err(ProviderError::AuthRequired));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let error = provider
        .fetch_cli_acp_without_login(missing_login(), &FetchContext::default())
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(&error, ProviderError::NotInstalled(message) if message.contains("grok login")),
        "{error}"
    );
}

#[tokio::test]
async fn failing_cli_acp_in_auto_mode_moves_to_the_next_step() {
    let (calls, run) = counting_cli(|| Err(ProviderError::AuthRequired));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let step = provider
        .try_ambient_loaded(
            GrokAuthKind::Cli,
            Err(missing_login()),
            &FetchContext::default(),
        )
        .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(step.is_none());
}

#[tokio::test]
async fn isolated_token_account_never_reaches_the_cli_without_a_login() {
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);
    let ctx = FetchContext {
        token_account_isolated: true,
        ..FetchContext::default()
    };

    let error = provider
        .fetch_cli_acp_without_login(missing_login(), &ctx)
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(error, ProviderError::NotInstalled(_)), "{error}");
}

#[tokio::test]
async fn malformed_cli_login_never_reaches_the_cli() {
    // A parse failure is a broken file, not a missing or expired login.
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let error = provider
        .fetch_cli_acp_without_login(
            ProviderError::Parse("bad json".to_string()),
            &FetchContext::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(error, ProviderError::Parse(_)), "{error}");
}

#[tokio::test]
async fn oauth_login_missing_never_reaches_the_cli() {
    let (calls, run) = counting_cli(|| Ok(cli_answer()));
    let provider = GrokProvider::new().with_cli_billing_for_tests(run);

    let step = provider
        .try_ambient_loaded(
            GrokAuthKind::OAuth,
            Err(missing_login()),
            &FetchContext::default(),
        )
        .await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(step.is_none());
}

/// A wallet-only ACP answer: an active weekly period and prepaid balance, no percent.
fn wallet_only_answer(now: DateTime<Utc>) -> BearerBilling {
    BearerBilling {
        billing: GrokBillingSnapshot {
            used_percent: None,
            used_percent_is_wire_published: false,
            used_percent_is_implicit_zero: false,
            resets_at: Some(now + chrono::Duration::days(6)),
            window_minutes: Some(10_080),
            product_usage: Vec::new(),
        },
        subscription_tier: Some("SuperGrok Heavy".to_string()),
        prepaid_balance_usd: Some(12.5),
    }
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            encoded.push(byte);
            return encoded;
        }
        encoded.push(byte | 0x80);
    }
}

fn length_field(field: u64, contents: &[u8]) -> Vec<u8> {
    let mut encoded = varint((field << 3) | 2);
    encoded.extend(varint(contents.len() as u64));
    encoded.extend(contents);
    encoded
}

fn timestamp_message(seconds: i64) -> Vec<u8> {
    let mut message = vec![0x08];
    message.extend(varint(seconds as u64));
    message
}

/// A gRPC-web response for an active weekly period whose usage is omitted, so
/// the zero is implied. Timestamps are relative to `now` to keep the period active.
fn implicit_zero_weekly_response(now: DateTime<Utc>) -> Vec<u8> {
    let mut period = vec![0x08, 0x02];
    period.extend(length_field(
        2,
        &timestamp_message((now - chrono::Duration::days(1)).timestamp()),
    ));
    period.extend(length_field(
        3,
        &timestamp_message((now + chrono::Duration::days(6)).timestamp()),
    ));
    let payload = length_field(1, &length_field(8, &period));
    let mut response = vec![0x00];
    response.extend(
        u32::try_from(payload.len())
            .expect("test payload length fits u32")
            .to_be_bytes(),
    );
    response.extend(payload);
    response
}

#[tokio::test]
async fn wallet_only_cli_answer_adopts_a_validated_zero_week() {
    let now = Utc::now();
    let mut server = mockito::Server::new_async().await;
    let grpc = server
        .mock("POST", "/billing")
        .with_status(200)
        .with_body(implicit_zero_weekly_response(now))
        .expect(1)
        .create_async()
        .await;
    let provider =
        GrokProvider::new().with_billing_endpoint_for_tests(format!("{}/billing", server.url()));

    let answer = provider
        .resolve_cli_unknown_usage(
            wallet_only_answer(now),
            &GrokCredentials::from_bearer("token"),
        )
        .await;

    grpc.assert_async().await;
    assert_eq!(answer.billing.used_percent, Some(0.0));
    assert!(answer.billing.used_percent_is_implicit_zero);
    assert_eq!(answer.billing.window_minutes, Some(10_080));
    assert_eq!(answer.prepaid_balance_usd, Some(12.5));
    assert_eq!(answer.subscription_tier.as_deref(), Some("SuperGrok Heavy"));
    let result = result_from_billing(answer.billing, "grok-cli", None, None, None);
    assert_eq!(result.usage.primary.used_percent, 0.0);
}

#[tokio::test]
async fn wallet_only_cli_answer_adopts_a_wire_published_percent() {
    let now = Utc::now();
    let mut server = mockito::Server::new_async().await;
    let grpc = server
        .mock("POST", "/billing")
        .with_status(200)
        .with_body(billing_response_with_percent(37.0))
        .expect(1)
        .create_async()
        .await;
    let provider =
        GrokProvider::new().with_billing_endpoint_for_tests(format!("{}/billing", server.url()));

    let answer = provider
        .resolve_cli_unknown_usage(
            wallet_only_answer(now),
            &GrokCredentials::from_bearer("token"),
        )
        .await;

    grpc.assert_async().await;
    assert_eq!(answer.billing.used_percent, Some(37.0));
    assert!(answer.billing.used_percent_is_wire_published);
    assert_eq!(answer.prepaid_balance_usd, Some(12.5));
    assert_eq!(answer.billing.window_minutes, Some(10_080));
}

#[tokio::test]
async fn cli_answer_with_a_percent_is_never_enriched() {
    let mut server = mockito::Server::new_async().await;
    let grpc = server
        .mock("POST", "/billing")
        .with_status(200)
        .with_body(billing_response_with_percent(90.0))
        .expect(0)
        .create_async()
        .await;
    let provider =
        GrokProvider::new().with_billing_endpoint_for_tests(format!("{}/billing", server.url()));

    let answer = provider
        .resolve_cli_unknown_usage(cli_answer(), &GrokCredentials::from_bearer("token"))
        .await;

    grpc.assert_async().await;
    assert_eq!(answer.billing.used_percent, Some(64.0));
}

#[tokio::test]
async fn failed_percent_enrichment_keeps_the_wallet_with_unknown_usage() {
    let now = Utc::now();
    let mut server = mockito::Server::new_async().await;
    let grpc = server
        .mock("POST", "/billing")
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let provider =
        GrokProvider::new().with_billing_endpoint_for_tests(format!("{}/billing", server.url()));

    let answer = provider
        .resolve_cli_unknown_usage(
            wallet_only_answer(now),
            &GrokCredentials::from_bearer("token"),
        )
        .await;

    grpc.assert_async().await;
    assert_eq!(answer.billing.used_percent, None);
    assert_eq!(answer.prepaid_balance_usd, Some(12.5));
    assert_eq!(answer.billing.window_minutes, Some(10_080));
}

#[tokio::test]
async fn expired_credentials_skip_percent_enrichment() {
    let now = Utc::now();
    let mut server = mockito::Server::new_async().await;
    let grpc = server
        .mock("POST", "/billing")
        .with_status(200)
        .with_body(implicit_zero_weekly_response(now))
        .expect(0)
        .create_async()
        .await;
    let provider =
        GrokProvider::new().with_billing_endpoint_for_tests(format!("{}/billing", server.url()));
    let mut credentials = GrokCredentials::from_bearer("stale-token");
    credentials.expires_at = Some(now - chrono::Duration::minutes(1));

    let answer = provider
        .resolve_cli_unknown_usage(wallet_only_answer(now), &credentials)
        .await;

    grpc.assert_async().await;
    assert_eq!(answer.billing.used_percent, None);
}

#[tokio::test]
async fn acp_wallet_answer_is_enriched_on_the_fallback_path() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/credits")
        .with_status(500)
        .create_async()
        .await;
    // The bearer phase and the enrichment each ask grok.com once; both fail here.
    let grpc = server
        .mock("POST", "/billing")
        .with_status(500)
        .expect(2)
        .create_async()
        .await;
    let (calls, run) = counting_cli(|| Ok(wallet_only_answer(Utc::now())));
    let provider = GrokProvider::new()
        .with_billing_endpoint_for_tests(format!("{}/billing", server.url()))
        .with_credits_proxy_endpoint_for_tests(format!("{}/credits", server.url()))
        .with_cli_billing_for_tests(run);

    let answer = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap();

    grpc.assert_async().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(answer.billing.used_percent, None);
    assert_eq!(answer.prepaid_balance_usd, Some(12.5));
}

#[tokio::test]
async fn acp_wallet_answer_adopts_the_enriched_percent_on_the_fallback_path() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/credits")
        .with_status(500)
        .create_async()
        .await;
    // mockito serves the first matching mock that still expects hits, so the
    // bearer phase gets the failure and the enrichment gets the percent.
    let bearer_billing = server
        .mock("POST", "/billing")
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let enrichment = server
        .mock("POST", "/billing")
        .with_status(200)
        .with_body(billing_response_with_percent(37.0))
        .expect(1)
        .create_async()
        .await;
    let (calls, run) = counting_cli(|| Ok(wallet_only_answer(Utc::now())));
    let provider = GrokProvider::new()
        .with_billing_endpoint_for_tests(format!("{}/billing", server.url()))
        .with_credits_proxy_endpoint_for_tests(format!("{}/credits", server.url()))
        .with_cli_billing_for_tests(run);

    let answer = provider
        .fetch_bearer_or_cli_billing(
            &GrokCredentials::from_bearer("token"),
            GrokAuthKind::Cli,
            &FetchContext::default(),
        )
        .await
        .unwrap();

    bearer_billing.assert_async().await;
    enrichment.assert_async().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(answer.billing.used_percent, Some(37.0));
    assert!(answer.billing.used_percent_is_wire_published);
    assert_eq!(answer.prepaid_balance_usd, Some(12.5));
}
