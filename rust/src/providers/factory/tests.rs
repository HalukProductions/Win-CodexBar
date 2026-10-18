use super::*;
use crate::browser::detection::BrowserType;
use crate::browser::session_scan::test_support::FakeCookieStore;
use crate::providers::browser_cookie_sessions_from;
use mockito::{Matcher, Mock, Server, ServerGuard};
use std::fs;

#[test]
fn cleans_quotes_and_whitespace() {
    assert_eq!(
        clean_factory_secret(Some("  \"fk-quoted\"  ")).as_deref(),
        Some("fk-quoted")
    );
    assert_eq!(
        clean_factory_secret(Some("'fk-single'")).as_deref(),
        Some("fk-single")
    );
    assert_eq!(clean_factory_secret(Some("   ")), None);
    assert_eq!(clean_factory_secret(None), None);
}

#[test]
fn parses_factory_dotenv_variants() {
    assert_eq!(
        parse_factory_dotenv_key("FACTORY_API_KEY=fk-plain").as_deref(),
        Some("fk-plain")
    );
    assert_eq!(
        parse_factory_dotenv_key("export FACTORY_API_KEY='fk-single'").as_deref(),
        Some("fk-single")
    );
    assert_eq!(
        parse_factory_dotenv_key("# comment\nFACTORY_API_KEY=\"fk-double\"").as_deref(),
        Some("fk-double")
    );
    assert_eq!(parse_factory_dotenv_key("OTHER=1\n"), None);
    assert_eq!(
        parse_factory_dotenv_key(
            "export FACTORY_API_KEY='fk-quoted'\nMALFORMED\nFACTORY_API_KEY=\n"
        )
        .as_deref(),
        Some("fk-quoted")
    );
    // Empty assignment must not short-circuit a later real key.
    assert_eq!(
        parse_factory_dotenv_key("FACTORY_API_KEY=\nFACTORY_API_KEY=fk-real\n").as_deref(),
        Some("fk-real")
    );
    assert_eq!(
        parse_factory_dotenv_key("FACTORY_API_KEY=\"\"\nFACTORY_API_KEY='fk-later'\n").as_deref(),
        Some("fk-later")
    );
}

#[test]
fn key_resolution_honors_explicit_env_dotenv_precedence() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join(".factory")).unwrap();
    fs::write(
        home.path().join(".factory").join(".env"),
        "FACTORY_API_KEY=fk-dotenv\n",
    )
    .unwrap();

    let env_with_key = HashMap::from([
        (
            FACTORY_API_KEY_ENV.to_string(),
            "  \"fk-env\"  ".to_string(),
        ),
        ("USERPROFILE".to_string(), home.path().display().to_string()),
    ]);
    let env_dotenv_only =
        HashMap::from([("USERPROFILE".to_string(), home.path().display().to_string())]);
    let env_home_fallback =
        HashMap::from([("HOME".to_string(), home.path().display().to_string())]);

    assert_eq!(
        resolve_factory_api_key_from(Some(" 'fk-saved' "), &env_with_key, None).as_deref(),
        Some("fk-saved")
    );
    assert_eq!(
        resolve_factory_api_key_from(None, &env_with_key, Some(" fk-store ")).as_deref(),
        Some("fk-store")
    );
    assert_eq!(
        resolve_factory_api_key_from(None, &env_with_key, None).as_deref(),
        Some("fk-env")
    );
    assert_eq!(
        resolve_factory_api_key_from(None, &env_dotenv_only, None).as_deref(),
        Some("fk-dotenv")
    );
    assert_eq!(
        resolve_factory_api_key_from(None, &env_home_fallback, None).as_deref(),
        Some("fk-dotenv")
    );
    assert_eq!(
        resolve_factory_api_key_from(None, &HashMap::new(), None),
        None
    );
}

#[test]
fn parses_billing_limits_fixture_json() {
    let body = r#"{
          "usesTokenRateLimitsBilling": true,
          "limits": {
            "standard": {
              "fiveHour": { "usedPercent": 12, "secondsRemaining": 3600 },
              "weekly": { "usedPercent": 34, "secondsRemaining": 86400 },
              "monthly": { "usedPercent": 56, "secondsRemaining": 604800 }
            }
          },
          "extraUsageBalanceCents": 0,
          "extraUsageAllowed": false,
          "tokenRateLimitsRolloutEligible": true
        }"#;
    let parsed: FactoryBillingLimitsResponse = serde_json::from_str(body).unwrap();
    assert!(parsed.uses_token_rate_limits_billing);
    let limits = parsed.limits.unwrap();
    let snap = snapshot_from_billing_limits(&limits, None);
    assert!((snap.primary.used_percent - 12.0).abs() < f64::EPSILON);
    assert!((snap.secondary.unwrap().used_percent - 34.0).abs() < f64::EPSILON);
    assert!((snap.tertiary.unwrap().used_percent - 56.0).abs() < f64::EPSILON);
}

#[test]
fn parses_legacy_usage_fixture_json() {
    let body = r#"{
          "standard": { "used": 25.0, "allowance": 100.0 },
          "premium": { "used": 10.0, "allowance": 50.0 }
        }"#;
    let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
    let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
    assert!((snap.primary.used_percent - 25.0).abs() < f64::EPSILON);
    assert!((snap.secondary.unwrap().used_percent - 20.0).abs() < f64::EPSILON);
}

#[test]
fn parses_nested_usage_fixture_json() {
    let body = r#"{
          "usage": {
            "standard": { "userTokens": 1200, "totalAllowance": 4000, "usedRatio": 0.3 },
            "premium": { "userTokens": 100, "totalAllowance": 1000, "usedRatio": 0.1 }
          }
        }"#;
    let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
    let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
    assert!((snap.primary.used_percent - 30.0).abs() < f64::EPSILON);
    assert!((snap.secondary.unwrap().used_percent - 10.0).abs() < f64::EPSILON);
}

#[test]
fn empty_nested_usage_falls_through_to_top_level() {
    let body = r#"{
          "usage": {},
          "standard": { "used": 40.0, "allowance": 100.0 },
          "premium": { "used": 5.0, "allowance": 50.0 }
        }"#;
    let parsed: FactoryUsageResponse = serde_json::from_str(body).unwrap();
    let snap = FactoryProvider::usage_snapshot_from_response(&parsed);
    assert!((snap.primary.used_percent - 40.0).abs() < f64::EPSILON);
    assert!((snap.secondary.unwrap().used_percent - 10.0).abs() < f64::EPSILON);
}

#[test]
fn available_sources_do_not_advertise_cli() {
    let sources = FactoryProvider::new().available_sources();
    assert!(!sources.contains(&SourceMode::Cli));
    assert!(sources.contains(&SourceMode::Auto));
    assert!(sources.contains(&SourceMode::OAuth));
    assert!(sources.contains(&SourceMode::Web));
}

#[test]
fn parses_auth_fixture_json() {
    let body = r#"{
          "organization": {
            "id": "org_1",
            "name": "Acme",
            "subscription": {
              "factoryTier": "team",
              "orbSubscription": {
                "plan": { "name": "Team", "id": "plan_1" },
                "status": "active"
              }
            }
          },
          "userProfile": { "id": "u1", "email": "user@example.com" }
        }"#;
    let auth: FactoryAuthResponse = serde_json::from_str(body).unwrap();
    let snap =
        FactoryProvider::apply_auth_info(UsageSnapshot::new(RateWindow::new(0.0)), Some(auth));
    assert_eq!(snap.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(snap.account_organization.as_deref(), Some("Acme"));
    assert!(
        snap.login_method
            .as_deref()
            .is_some_and(|m| m.contains("team") || m.contains("Team"))
    );
}

#[test]
fn auto_api_errors_are_recoverable() {
    assert!(factory_api_error_is_recoverable(
        &ProviderError::AuthRequired
    ));
    assert!(factory_api_error_is_recoverable(&ProviderError::Timeout));
    assert!(factory_api_error_is_recoverable(&ProviderError::Parse(
        "bad json".into()
    )));
    assert!(factory_api_error_is_recoverable(&ProviderError::Other(
        "HTTP 500".into()
    )));
    assert!(factory_api_error_is_recoverable(
        &ProviderError::NotInstalled("missing".into())
    ));
    assert!(!factory_api_error_is_recoverable(
        &ProviderError::UnsupportedSource(SourceMode::Web)
    ));
}

#[test]
fn secret_redactor_covers_factory_keys() {
    let redacted = crate::core::SecretRedactor::redact("Factory key fk-test-key-abcdef");
    assert!(
        !redacted.contains("fk-test-key"),
        "factory key must not appear: {redacted}"
    );
}

// ── Browser session fall-through (web path) ──────────────────────────

const LEGACY_USAGE_BODY: &str = r#"{
  "standard": { "used": 25.0, "allowance": 100.0 }
}"#;

/// Auth answers for every cookie; the rest of the cases are decided by usage.
async fn mock_factory_auth(server: &mut ServerGuard) -> Mock {
    server
        .mock("GET", "/api/app/auth/me")
        .with_status(200)
        .with_body("{}")
        .create_async()
        .await
}

/// Usage answers `status` only for requests carrying exactly `cookie`.
async fn mock_factory_session(
    server: &mut ServerGuard,
    cookie: &str,
    status: usize,
    body: &str,
    hits: usize,
) -> Mock {
    server
        .mock("GET", "/api/organization/subscription/usage")
        .match_query(Matcher::Any)
        .match_header("cookie", cookie)
        .with_status(status)
        .with_body(body)
        .expect(hits)
        .create_async()
        .await
}

/// Two-browser store: each browser holds one Factory cookie on the app host.
fn factory_browser_store(first: &'static str, second: &'static str) -> FakeCookieStore {
    FakeCookieStore::default()
        .cookies(BrowserType::Chrome, "app.factory.ai", &[("session", first)])
        .cookies(BrowserType::Edge, "app.factory.ai", &[("session", second)])
}

async fn fetch_web_with_store(
    store: &FakeCookieStore,
    base: &str,
) -> Result<UsageSnapshot, ProviderError> {
    let client = FactoryProvider::build_client()?;
    let sessions =
        browser_cookie_sessions_from(store, &FetchContext::default(), FACTORY_COOKIE_DOMAINS);
    FactoryProvider::new()
        .fetch_web_sessions(&client, base, sessions)
        .await
}

#[tokio::test]
async fn rejected_browser_session_moves_on_to_the_next_browser() {
    let mut server = Server::new_async().await;
    let _auth = mock_factory_auth(&mut server).await;
    let stale = mock_factory_session(&mut server, "session=stale-chrome", 401, "{}", 1).await;
    let valid =
        mock_factory_session(&mut server, "session=valid-edge", 200, LEGACY_USAGE_BODY, 1).await;
    let store = factory_browser_store("stale-chrome", "valid-edge").cookies(
        BrowserType::Firefox,
        "app.factory.ai",
        &[("session", "never-read")],
    );

    let usage = fetch_web_with_store(&store, &server.url())
        .await
        .expect("the second browser's session is accepted");

    stale.assert_async().await;
    valid.assert_async().await;
    assert!((usage.primary.used_percent - 25.0).abs() < f64::EPSILON);
    assert!(
        store
            .reads()
            .iter()
            .all(|(browser, _)| *browser != BrowserType::Firefox),
        "the browser after the accepted session must not be read"
    );
}

#[tokio::test]
async fn forbidden_browser_session_also_moves_on() {
    let mut server = Server::new_async().await;
    let _auth = mock_factory_auth(&mut server).await;
    let forbidden = mock_factory_session(&mut server, "session=stale-chrome", 403, "{}", 1).await;
    let valid =
        mock_factory_session(&mut server, "session=valid-edge", 200, LEGACY_USAGE_BODY, 1).await;
    let store = factory_browser_store("stale-chrome", "valid-edge");

    fetch_web_with_store(&store, &server.url())
        .await
        .expect("a 403 is a rejected session, so the next browser is tried");

    forbidden.assert_async().await;
    valid.assert_async().await;
}

#[tokio::test]
async fn every_rejected_browser_session_reports_auth_required() {
    let mut server = Server::new_async().await;
    let _auth = mock_factory_auth(&mut server).await;
    let chrome = mock_factory_session(&mut server, "session=stale-chrome", 401, "{}", 1).await;
    let edge = mock_factory_session(&mut server, "session=stale-edge", 401, "{}", 1).await;
    let store = factory_browser_store("stale-chrome", "stale-edge");

    let error = fetch_web_with_store(&store, &server.url())
        .await
        .expect_err("no browser session is accepted");

    chrome.assert_async().await;
    edge.assert_async().await;
    assert!(
        matches!(error, ProviderError::AuthRequired),
        "all rejected sessions report the sign-in error, got {error:?}"
    );
}

#[tokio::test]
async fn server_error_from_a_browser_session_stops_the_search() {
    let mut server = Server::new_async().await;
    let _auth = mock_factory_auth(&mut server).await;
    let broken = mock_factory_session(&mut server, "session=stale-chrome", 500, "{}", 1).await;
    let untried = mock_factory_session(&mut server, "session=valid-edge", 200, "{}", 0).await;
    let store = factory_browser_store("stale-chrome", "valid-edge");

    let error = fetch_web_with_store(&store, &server.url())
        .await
        .expect_err("a server error is not a rejected session");

    broken.assert_async().await;
    untried.assert_async().await;
    assert!(
        matches!(error, ProviderError::Other(_)),
        "server error is reported, not replaced by a later browser: {error:?}"
    );
}

#[tokio::test]
async fn no_browser_cookies_reports_no_cookies_without_a_request() {
    let server = Server::new_async().await;
    let store = FakeCookieStore::default();

    let error = fetch_web_with_store(&store, &server.url())
        .await
        .expect_err("no browser holds a Factory cookie");

    assert!(matches!(error, ProviderError::NoCookies), "got {error:?}");
}
