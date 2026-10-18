//! Best-effort Claude OAuth profile lookup for the card header email.
//!
//! Upstream (#4538) reads the account email from `GET /api/oauth/profile`
//! off the usage refresh's critical path: one background lookup per access
//! token, bounded to [`LOOKUP_BUDGET`]. The usage refresh never waits for it,
//! so a refresh that starts the lookup may publish without an email; later
//! refreshes of the same token reuse the answer. Both outcomes are cached in
//! memory: a found email for the life of the process, a failure until the
//! token changes or CodexBar restarts. The email is display metadata for the
//! Claude card only.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use reqwest::Client;

const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
/// Whole-lookup budget: connect, response and body.
const LOOKUP_BUDGET: Duration = Duration::from_secs(2);

enum CachedProfile {
    Pending,
    Found(String),
    Failed,
}

/// Keyed by token fingerprint, so a new token never borrows another token's email.
static PROFILE_CACHE: LazyLock<Mutex<HashMap<String, CachedProfile>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Test-only profile URL, so fetch-level tests can observe lookups on a mock
// server. Thread-local because each test runs on its own thread.
#[cfg(test)]
thread_local! {
    static TEST_PROFILE_URL: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

fn profile_url() -> String {
    #[cfg(test)]
    {
        if let Some(url) = TEST_PROFILE_URL.with(|url| url.borrow().clone()) {
            return url;
        }
    }
    PROFILE_URL.to_string()
}

/// The known email for `access_token`, without waiting. The first call for a
/// token starts the background lookup and returns `None`, as do later calls
/// while it is pending or after it failed.
pub(super) fn display_email(
    client: &Client,
    access_token: &str,
    fingerprint: &str,
) -> Option<String> {
    lookup(
        client,
        &profile_url(),
        access_token,
        fingerprint,
        LOOKUP_BUDGET,
    )
    .0
}

/// [`display_email`] against `url`, also returning the started lookup task.
fn lookup(
    client: &Client,
    url: &str,
    access_token: &str,
    fingerprint: &str,
    budget: Duration,
) -> (Option<String>, Option<tokio::task::JoinHandle<()>>) {
    let mut cache = PROFILE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match cache.get(fingerprint) {
        Some(CachedProfile::Found(email)) => return (Some(email.clone()), None),
        Some(CachedProfile::Pending | CachedProfile::Failed) => return (None, None),
        None => {}
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return (None, None);
    };
    cache.insert(fingerprint.to_string(), CachedProfile::Pending);
    drop(cache);

    let client = client.clone();
    let url = url.to_string();
    let access_token = access_token.to_string();
    let fingerprint = fingerprint.to_string();
    let task = runtime.spawn(async move {
        let email = tokio::time::timeout(budget, request_email(&client, &url, &access_token))
            .await
            .unwrap_or_else(|_| {
                tracing::debug!("Claude OAuth profile lookup timed out");
                None
            });
        let entry = email.map_or(CachedProfile::Failed, CachedProfile::Found);
        PROFILE_CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fingerprint, entry);
    });
    (None, Some(task))
}

async fn request_email(client: &Client, url: &str, access_token: &str) -> Option<String> {
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), "Claude OAuth profile unavailable");
            return None;
        }
        Err(error) => {
            tracing::debug!(%error, "Claude OAuth profile request failed");
            return None;
        }
    };
    let body = response.text().await.ok()?;
    let email = parse_email(&body);
    if email.is_none() {
        tracing::debug!("Claude OAuth profile has no account email");
    }
    email
}

/// `account.email_address` (also `emailAddress` / `email`), falling back to
/// the same keys at the top level. Blank or non-email values are ignored.
pub(super) fn parse_email(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let read = |object: &serde_json::Value| {
        ["email_address", "emailAddress", "email"]
            .iter()
            .filter_map(|key| object.get(*key).and_then(|v| v.as_str()))
            .map(str::trim)
            .find(|value| value.contains('@'))
            .map(str::to_string)
    };
    json.get("account").and_then(read).or_else(|| read(&json))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of the upstream profile response (synthetic values).
    const PROFILE_FIXTURE: &str = r#"{
        "account": {
            "uuid": "00000000-0000-4000-8000-000000000001",
            "full_name": "Parity User",
            "display_name": "Parity",
            "email_address": "parity.user@example.com",
            "has_claude_max": true,
            "has_claude_pro": false
        },
        "organization": {
            "uuid": "00000000-0000-4000-8000-000000000002",
            "name": "Parity Labs",
            "organization_type": "claude_max",
            "billing_type": "stripe_subscription",
            "rate_limit_tier": "default_claude_max_20x"
        }
    }"#;

    #[test]
    fn parses_the_account_email_from_the_profile_fixture() {
        assert_eq!(
            parse_email(PROFILE_FIXTURE).as_deref(),
            Some("parity.user@example.com")
        );
    }

    #[test]
    fn accepts_alternate_keys_and_top_level_email() {
        assert_eq!(
            parse_email(r#"{"account":{"emailAddress":" a@example.com "}}"#).as_deref(),
            Some("a@example.com")
        );
        assert_eq!(
            parse_email(r#"{"email":"top@example.com"}"#).as_deref(),
            Some("top@example.com")
        );
    }

    #[test]
    fn ignores_missing_blank_or_malformed_values() {
        assert!(parse_email("").is_none());
        assert!(parse_email("not json").is_none());
        assert!(parse_email(r#"{"account":{"email_address":"   "}}"#).is_none());
        assert!(parse_email(r#"{"account":{"email_address":42}}"#).is_none());
        assert!(parse_email(r#"{"organization":{"name":"Parity Labs"}}"#).is_none());
    }

    // Wire tests mirror upstream `ClaudeOAuthProfileIdentityTests` (#4538).
    // Every test uses its own tokens: the cache is process-wide.

    use mockito::{Matcher, Mock, Server, ServerGuard};
    use std::time::Instant;

    const TEST_BUDGET: Duration = Duration::from_secs(2);

    fn fingerprint(token: &str) -> String {
        super::super::usage_gate::token_fingerprint(token)
    }

    async fn profile_mock(
        server: &mut ServerGuard,
        token: &str,
        status: usize,
        calls: usize,
    ) -> Mock {
        server
            .mock("GET", "/api/oauth/profile")
            .match_header("authorization", format!("Bearer {token}").as_str())
            .match_header("anthropic-beta", "oauth-2025-04-20")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body(format!(
                r#"{{"account":{{"email_address":"{token}@example.com"}}}}"#
            ))
            .expect(calls)
            .create_async()
            .await
    }

    /// One "refresh": read the cache, starting a lookup when the token is new.
    async fn refresh(url: &str, token: &str, budget: Duration) -> Option<String> {
        let (email, task) = lookup(&Client::new(), url, token, &fingerprint(token), budget);
        if let Some(task) = task {
            task.await.unwrap();
        }
        email
    }

    #[tokio::test]
    async fn first_refresh_starts_the_lookup_and_later_refreshes_reuse_its_email() {
        let mut server = Server::new_async().await;
        let token = "fixture-profile-found";
        let mock = profile_mock(&mut server, token, 200, 1).await;
        let url = format!("{}/api/oauth/profile", server.url());

        assert_eq!(refresh(&url, token, TEST_BUDGET).await, None);
        for _ in 0..2 {
            assert_eq!(
                refresh(&url, token, TEST_BUDGET).await.as_deref(),
                Some("fixture-profile-found@example.com")
            );
        }
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn credential_changes_cannot_borrow_another_tokens_email() {
        let mut server = Server::new_async().await;
        let first = profile_mock(&mut server, "fixture-profile-first", 200, 1).await;
        let second = profile_mock(&mut server, "fixture-profile-second", 200, 1).await;
        let url = format!("{}/api/oauth/profile", server.url());

        for token in [
            "fixture-profile-first",
            "fixture-profile-second",
            "fixture-profile-first",
        ] {
            refresh(&url, token, TEST_BUDGET).await;
            assert_eq!(
                refresh(&url, token, TEST_BUDGET).await,
                Some(format!("{token}@example.com"))
            );
        }
        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn failed_lookup_is_not_retried_for_the_same_token() {
        let mut server = Server::new_async().await;
        let token = "fixture-profile-failed";
        let mock = profile_mock(&mut server, token, 500, 1).await;
        let url = format!("{}/api/oauth/profile", server.url());

        assert_eq!(refresh(&url, token, TEST_BUDGET).await, None);
        assert_eq!(refresh(&url, token, TEST_BUDGET).await, None);
        assert_eq!(refresh(&url, token, TEST_BUDGET).await, None);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn stalled_lookup_never_delays_the_caller_and_fails_soft_at_its_budget() {
        // Accepts connections and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/oauth/profile",
            listener.local_addr().unwrap()
        );
        let hold = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let token = "fixture-profile-stalled";

        let started = Instant::now();
        let (email, task) = lookup(
            &Client::new(),
            &url,
            token,
            &fingerprint(token),
            Duration::from_millis(200),
        );
        assert_eq!(email, None);
        assert!(started.elapsed() < Duration::from_millis(200));

        task.expect("lookup started").await.unwrap();
        let (email, task) = lookup(
            &Client::new(),
            &url,
            token,
            &fingerprint(token),
            Duration::from_millis(200),
        );
        assert_eq!(email, None);
        assert!(task.is_none(), "a timed-out token must not be retried");
        hold.abort();
    }

    #[test]
    fn without_a_runtime_no_lookup_starts_and_nothing_is_cached() {
        let token = "fixture-profile-no-runtime";
        let (email, task) = lookup(
            &Client::new(),
            "http://127.0.0.1:9/api/oauth/profile",
            token,
            &fingerprint(token),
            TEST_BUDGET,
        );
        assert!(email.is_none() && task.is_none());
        assert!(
            !PROFILE_CACHE
                .lock()
                .unwrap()
                .contains_key(&fingerprint(token))
        );
    }

    #[tokio::test]
    async fn expired_or_scope_less_tokens_send_no_profile_request() {
        let mut server = Server::new_async().await;
        let profile = server
            .mock("GET", "/api/oauth/profile")
            .with_status(200)
            .with_body(PROFILE_FIXTURE)
            .expect(0)
            .create_async()
            .await;
        TEST_PROFILE_URL.with(|url| {
            *url.borrow_mut() = Some(format!("{}/api/oauth/profile", server.url()));
        });
        let expired = super::super::ClaudeOAuthCredentials {
            access_token: "fixture-profile-expired".to_string(),
            refresh_token: None,
            expires_at: Some(chrono::Utc::now() - chrono::Duration::minutes(10)),
            scopes: vec!["user:profile".to_string()],
            rate_limit_tier: None,
        };
        let scope_less = super::super::ClaudeOAuthCredentials {
            access_token: "fixture-profile-scope-less".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec!["user:inference".to_string()],
            rate_limit_tier: None,
        };
        let fetcher = super::super::ClaudeOAuthFetcher::new();

        let expired_error = fetcher
            .fetch_with_credentials(expired, "fixture-gate")
            .await
            .err()
            .unwrap();
        let scope_error = fetcher
            .fetch_with_credentials(scope_less, "fixture-gate")
            .await
            .err()
            .unwrap();

        assert!(matches!(
            expired_error,
            crate::core::ProviderError::OAuthExpired(_)
        ));
        assert!(matches!(
            scope_error,
            crate::core::ProviderError::OAuth(ref message) if message.contains("user:profile")
        ));
        // A lookup that had started would answer the mock within milliseconds.
        tokio::time::sleep(Duration::from_millis(200)).await;
        profile.assert_async().await;
    }

    /// Control for the test above: the mock does see a /profile request when a
    /// lookup runs for a valid token.
    #[tokio::test]
    async fn a_valid_token_reaches_the_profile_endpoint() {
        let mut server = Server::new_async().await;
        let token = "fixture-profile-valid";
        let mock = profile_mock(&mut server, token, 200, 1).await;
        let url = format!("{}/api/oauth/profile", server.url());

        assert_eq!(
            refresh(&url, token, TEST_BUDGET).await,
            None,
            "the first refresh starts the lookup"
        );
        mock.assert_async().await;
    }
}
