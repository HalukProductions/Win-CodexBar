//! Wire tests for the OAuth usage request's `cedar_ember` opt-in (upstream
//! `ClaudeOAuthResetCreditsTests`).

use super::{
    ClaudeOAuthCredentials, ClaudeOAuthFetcher, is_rate_limited_error, reset_credits_item,
};
use crate::core::ProviderError;
use chrono::Utc;
use mockito::{Matcher, Mock, Server, ServerGuard};

const USAGE_PATH: &str = "/api/oauth/usage";
const GATE_KEY: &str = "gate-key-fixture";

const ELIGIBLE_BLOCK: &str = r#"{"eligible":true,"grants":[
  {"id":"redemption-handle","label":"private grant","resets_left":2,"resets_total":2,
   "starts_at":"2020-01-01T00:00:00Z","ends_at":"2099-01-01T00:00:00Z",
   "paused":false,"usable_now":false},
  {"resets_left":1,"paused":true},
  {"resets_left":1,"paused":false,"ends_at":"2020-01-01T00:00:00Z"},
  {"resets_left":1,"paused":false,"starts_at":"2099-01-01T00:00:00Z"}
]}"#;

fn payload(block: &str) -> String {
    format!(
        r#"{{"five_hour":{{"utilization":12}},
         "extra_usage":{{"is_enabled":true,"monthly_limit":1000,"used_credits":300}},
         "cedar_ember":{block}}}"#
    )
}

fn credentials(token: &str) -> ClaudeOAuthCredentials {
    ClaudeOAuthCredentials {
        access_token: token.to_string(),
        refresh_token: None,
        expires_at: None,
        scopes: vec!["user:profile".to_string()],
        rate_limit_tier: None,
    }
}

async fn opted_in(server: &mut ServerGuard, status: usize, body: &str, calls: usize) -> Mock {
    server
        .mock("GET", USAGE_PATH)
        .match_query(Matcher::UrlEncoded("cedar_ember".into(), "1".into()))
        .match_header("authorization", "Bearer synthetic-reset-token")
        .match_header("anthropic-beta", "oauth-2025-04-20")
        .with_status(status)
        .with_header("retry-after", "300")
        .with_body(body)
        .expect(calls)
        .create_async()
        .await
}

async fn plain(server: &mut ServerGuard, status: usize, body: &str, calls: usize) -> Mock {
    server
        .mock("GET", USAGE_PATH)
        .match_query(Matcher::Missing)
        .match_header("authorization", "Bearer synthetic-reset-token")
        .match_header("anthropic-beta", "oauth-2025-04-20")
        .with_status(status)
        .with_header("retry-after", "300")
        .with_body(body)
        .expect(calls)
        .create_async()
        .await
}

#[tokio::test]
async fn opt_in_reads_reset_inventory_and_keeps_spend_without_a_retry() {
    let mut server = Server::new_async().await;
    let first = opted_in(&mut server, 200, &payload(ELIGIBLE_BLOCK), 1).await;
    let retry = plain(&mut server, 200, &payload("null"), 0).await;
    let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());

    let usage = fetcher
        .fetch_usage(&credentials("synthetic-reset-token"), GATE_KEY)
        .await
        .unwrap();

    assert_eq!(
        usage.extra_usage.as_ref().unwrap().used_credits,
        Some(300.0)
    );
    let item = reset_credits_item(&usage, Utc::now()).expect("reset inventory");
    assert_eq!(item.available_count, 2);
    first.assert_async().await;
    retry.assert_async().await;
}

#[tokio::test]
async fn rejected_opt_in_retries_once_without_it_and_keeps_ordinary_usage() {
    for status in [400, 403] {
        let mut server = Server::new_async().await;
        let first = opted_in(&mut server, status, "{}", 1).await;
        let retry = plain(&mut server, 200, &payload("null"), 1).await;
        let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());

        let usage = fetcher
            .fetch_usage(&credentials("synthetic-reset-token"), GATE_KEY)
            .await
            .unwrap();

        assert_eq!(
            usage.five_hour.as_ref().unwrap().utilization,
            Some(12.0),
            "status {status}"
        );
        assert!(usage.extra_usage.is_some(), "status {status}");
        assert!(reset_credits_item(&usage, Utc::now()).is_none());
        first.assert_async().await;
        retry.assert_async().await;
    }
}

#[tokio::test]
async fn authentication_scope_rate_limit_and_server_errors_do_not_retry() {
    let body = r#"{"error":"Missing required scope user:profile"}"#;
    for status in [401, 403, 429, 500] {
        let mut server = Server::new_async().await;
        let first = opted_in(&mut server, status, body, 1).await;
        let retry = plain(&mut server, 200, &payload("null"), 0).await;
        let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());

        let error = fetcher
            .fetch_usage(&credentials("synthetic-reset-token"), GATE_KEY)
            .await
            .err()
            .unwrap();

        match status {
            401 => assert!(matches!(error, ProviderError::OAuthExpired(_))),
            403 => assert!(
                matches!(&error, ProviderError::OAuth(m) if m.contains("scope requirement 'user:profile'"))
            ),
            429 => assert!(is_rate_limited_error(&error)),
            _ => assert!(matches!(&error, ProviderError::OAuth(m) if m.contains("500"))),
        }
        first.assert_async().await;
        retry.assert_async().await;
    }
}

#[tokio::test]
async fn ordinary_usage_rejection_ends_the_fallback_after_two_requests() {
    for status in [400, 403] {
        let mut server = Server::new_async().await;
        let first = opted_in(&mut server, status, "{}", 1).await;
        let retry = plain(&mut server, status, "{}", 1).await;
        let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());

        let error = fetcher
            .fetch_usage(&credentials("synthetic-reset-token"), GATE_KEY)
            .await
            .err()
            .unwrap();

        assert!(
            matches!(&error, ProviderError::OAuth(m) if m.contains(&status.to_string())),
            "status {status}: {error:?}"
        );
        first.assert_async().await;
        retry.assert_async().await;
    }
}

#[tokio::test]
async fn fallback_preserves_terminal_errors_and_the_rate_limit_block() {
    for status in [401, 429, 500] {
        let mut server = Server::new_async().await;
        let first = opted_in(&mut server, 400, "{}", 1).await;
        let retry = plain(&mut server, status, "{}", 1).await;
        let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());
        let creds = credentials("synthetic-reset-token");

        let error = fetcher.fetch_usage(&creds, GATE_KEY).await.err().unwrap();

        match status {
            401 => assert!(matches!(error, ProviderError::OAuthExpired(_))),
            429 => {
                assert!(is_rate_limited_error(&error));
                // The block holds: no further request goes out.
                let again = fetcher.fetch_usage(&creds, GATE_KEY).await.err().unwrap();
                assert!(is_rate_limited_error(&again));
            }
            _ => assert!(matches!(&error, ProviderError::OAuth(m) if m.contains("500"))),
        }
        first.assert_async().await;
        retry.assert_async().await;
    }
}

#[tokio::test]
async fn the_gate_keeps_reset_counts_but_never_redemption_handles() {
    let mut server = Server::new_async().await;
    let first = opted_in(&mut server, 200, &payload(ELIGIBLE_BLOCK), 1).await;
    let fetcher = ClaudeOAuthFetcher::for_test_server(server.url());
    let creds = credentials("synthetic-reset-token");
    fetcher.fetch_usage(&creds, GATE_KEY).await.unwrap();

    let decision = fetcher
        .usage_gate
        .lock()
        .unwrap()
        .check(super::usage_gate::now_ms(), GATE_KEY);
    let super::usage_gate::Decision::Cached(stored) = decision else {
        panic!("expected a cached body, got {decision:?}");
    };
    assert!(!stored.contains("redemption-handle"));
    assert!(!stored.contains("private grant"));
    assert!(!stored.contains("usable_now"));

    // A cached answer is served without a request and keeps the inventory.
    let cached = fetcher.fetch_usage(&creds, GATE_KEY).await.unwrap();
    assert_eq!(
        reset_credits_item(&cached, Utc::now()).map(|item| item.available_count),
        Some(2)
    );
    first.assert_async().await;
}

#[test]
fn absent_ineligible_or_malformed_inventory_preserves_quota_and_spend() {
    for block in [
        "null",
        "{}",
        r#"{"eligible":false,"grants":[]}"#,
        r#"{"eligible":true,"grants":"invalid"}"#,
    ] {
        let usage = ClaudeOAuthFetcher::parse_usage(&payload(block)).expect("usage parses");
        assert_eq!(usage.five_hour.as_ref().unwrap().utilization, Some(12.0));
        assert!(usage.extra_usage.is_some());
        assert!(reset_credits_item(&usage, Utc::now()).is_none(), "{block}");
    }
}
