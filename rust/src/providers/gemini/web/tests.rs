use super::*;
use crate::browser::detection::BrowserType;
use crate::browser::session_scan::test_support::FakeCookieStore;
use mockito::{Matcher, Mock, Server, ServerGuard};

/// Synthetic response shaped like the Gemini app usage query.
const USAGE_BODY: &str = r#")]}'
[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"subscriptionTier":"TIER1","pools":[
  {"poolType":"GXU_FIVE_HOURLY","percentageUsed":0.24,
   "gxu":{"windowRefreshesAt":"2026-10-15T05:43:53.558200Z"}},
  {"poolType":"GXU_WEEKLY","percentageUsed":0.01786011,
   "gxu":{"windowRefreshesAt":"2026-10-21T04:43:53.558273Z"}}
]}}}},"errors":[{"message":"Unrelated upsell unavailable"}]}]"#;

const SIGNED_OUT_BODY: &str = r#"[{"data":{"viewer":null}}]"#;

fn usage(outcome: SessionOutcome) -> UsageSnapshot {
    match outcome {
        SessionOutcome::Usage(usage) => *usage,
        SessionOutcome::Rejected => panic!("session was rejected"),
    }
}

#[test]
fn maps_fractions_windows_resets_and_tier() {
    let snapshot = usage(parse_usage(USAGE_BODY.as_bytes()).expect("parses"));

    assert!((snapshot.primary.used_percent - 24.0).abs() < 1e-9);
    assert_eq!(snapshot.primary.window_minutes, Some(300));
    assert_eq!(
        snapshot.primary.resets_at.map(|at| at.to_rfc3339()),
        Some("2026-10-15T05:43:53.558200+00:00".into())
    );
    let weekly = snapshot.secondary.as_ref().expect("weekly window");
    assert!((weekly.used_percent - 1.786011).abs() < 1e-6);
    assert_eq!(weekly.window_minutes, Some(10_080));
    assert!(weekly.resets_at.is_some());
    assert_eq!(snapshot.primary_label.as_deref(), Some("5-hour"));
    assert_eq!(snapshot.secondary_label.as_deref(), Some("Weekly"));
    assert_eq!(snapshot.login_method.as_deref(), Some("Pro"));
    assert_eq!(snapshot.account_email, None);
    assert_eq!(snapshot.account_organization, None);
}

#[test]
fn unknown_windows_stay_unavailable_while_a_reported_zero_stays_zero() {
    let body = r#"[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"subscriptionTier":"UNRECOGNIZED","pools":[
      {"poolType":"GXU_FIVE_HOURLY","percentageUsed":null},
      {"poolType":"GXU_WEEKLY","percentageUsed":0,"gxu":{"windowRefreshesAt":"unknown"}}
    ]}}}}}]"#;
    let snapshot = usage(parse_usage(body.as_bytes()).expect("parses"));

    assert!(
        snapshot.primary.is_informational,
        "missing 5-hour is not 0%"
    );
    assert_eq!(snapshot.primary.window_minutes, Some(300));
    let weekly = snapshot.secondary.as_ref().expect("weekly window");
    assert_eq!(weekly.used_percent, 0.0);
    assert!(!weekly.is_informational);
    assert_eq!(weekly.resets_at, None);
    assert_eq!(snapshot.login_method, None);
}

#[test]
fn usage_above_the_limit_is_capped_and_negative_values_are_dropped() {
    let body = r#"[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"pools":[
      {"poolType":"GXU_FIVE_HOURLY","percentageUsed":1.7},
      {"poolType":"GXU_WEEKLY","percentageUsed":-0.2}
    ]}}}}}]"#;
    let snapshot = usage(parse_usage(body.as_bytes()).expect("parses"));
    assert_eq!(snapshot.primary.used_percent, 100.0);
    assert!(snapshot.secondary.is_none());
}

#[test]
fn accounts_without_reported_quota_do_not_manufacture_empty_bars() {
    let body = r#"[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"pools":[]}}}}}]"#;
    let error = parse_usage(body.as_bytes()).expect_err("no windows");
    assert!(matches!(&error, ProviderError::Other(message) if message == NO_WINDOWS));
}

#[test]
fn signed_out_viewer_and_html_pages_reject_the_session() {
    assert!(matches!(
        parse_usage(SIGNED_OUT_BODY.as_bytes()),
        Ok(SessionOutcome::Rejected)
    ));
    assert!(matches!(
        parse_usage(b"  <!doctype html><html>Sign in</html>"),
        Ok(SessionOutcome::Rejected)
    ));
}

#[test]
fn query_drift_fails_without_echoing_the_response() {
    for body in [
        r#"[{"data":{},"errors":[{"message":"private-response"}]}]"#,
        r#"{"data":{"viewer":{}}}"#,
        r#"[{"data":{"viewer":{"rpcs":{}}}}]"#,
        r#"[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"pools":"private-response"}}}}}]"#,
        "private-response",
    ] {
        let error = parse_usage(body.as_bytes()).expect_err("query drift");
        assert!(
            matches!(&error, ProviderError::Parse(message) if message == QUERY_CHANGED),
            "got {error:?}"
        );
        assert!(!error.to_string().contains("private-response"));
    }
}

// ── Browser session traversal ────────────────────────────────────────

async fn mock_session(
    server: &mut ServerGuard,
    cookie: &str,
    status: usize,
    body: &str,
    hits: usize,
) -> Mock {
    server
        .mock("GET", QUERY_PATH)
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("query_signature".into(), QUERY_SIGNATURE.into()),
            Matcher::UrlEncoded("variables".into(), QUERY_VARIABLES.into()),
        ]))
        .match_header("cookie", cookie)
        .match_header("referer", USAGE_PAGE_URL)
        .with_status(status)
        .with_body(body)
        .expect(hits)
        .create_async()
        .await
}

/// Each browser holds one Google session cookie plus a sibling-host cookie
/// that must never be sent to gemini.google.com.
fn two_browser_store(first: &'static str, second: &'static str) -> FakeCookieStore {
    FakeCookieStore::default()
        .host_cookies(
            BrowserType::Chrome,
            "google.com",
            &[
                (".google.com", "SID", first),
                ("mail.google.com", "MAIL", "x"),
            ],
        )
        .host_cookies(
            BrowserType::Edge,
            "google.com",
            &[
                (".google.com", "SID", second),
                ("mail.google.com", "MAIL", "y"),
            ],
        )
}

async fn fetch_with_store(
    store: &FakeCookieStore,
    server: &ServerGuard,
    ctx: &FetchContext,
) -> Result<UsageSnapshot, ProviderError> {
    let web = GeminiWeb::with_base_url(server.url(), Client::new());
    let sessions = browser_cookie_request_sessions_from(store, ctx, COOKIE_SCOPE);
    web.fetch_sessions(sessions).await
}

#[tokio::test]
async fn signed_out_session_moves_on_to_the_next_browser() {
    let mut server = Server::new_async().await;
    let signed_out = mock_session(&mut server, "SID=chrome", 200, SIGNED_OUT_BODY, 1).await;
    let valid = mock_session(&mut server, "SID=edge", 200, USAGE_BODY, 1).await;
    let store = two_browser_store("chrome", "edge");

    let usage = fetch_with_store(&store, &server, &FetchContext::default())
        .await
        .expect("the second browser's account is accepted");

    signed_out.assert_async().await;
    valid.assert_async().await;
    assert!((usage.primary.used_percent - 24.0).abs() < 1e-9);
}

#[tokio::test]
async fn unauthorized_forbidden_and_html_sessions_move_on() {
    let mut server = Server::new_async().await;
    let unauthorized = mock_session(&mut server, "SID=chrome", 401, "", 1).await;
    let forbidden = mock_session(&mut server, "SID=edge", 403, "", 1).await;
    let html = mock_session(&mut server, "SID=firefox", 200, "<html>login</html>", 1).await;
    let valid = mock_session(&mut server, "SID=brave", 200, USAGE_BODY, 1).await;
    let store = two_browser_store("chrome", "edge")
        .host_cookies(
            BrowserType::Firefox,
            "google.com",
            &[(".google.com", "SID", "firefox")],
        )
        .host_cookies(
            BrowserType::Brave,
            "google.com",
            &[(".google.com", "SID", "brave")],
        );

    fetch_with_store(&store, &server, &FetchContext::default())
        .await
        .expect("the fourth browser's account is accepted");

    unauthorized.assert_async().await;
    forbidden.assert_async().await;
    html.assert_async().await;
    valid.assert_async().await;
}

#[tokio::test]
async fn first_validated_account_ends_the_search_even_without_windows() {
    let mut server = Server::new_async().await;
    let empty = mock_session(
        &mut server,
        "SID=chrome",
        200,
        r#"[{"data":{"viewer":{"rpcs":{"getUsageInfo":{"pools":[]}}}}}]"#,
        1,
    )
    .await;
    let untried = mock_session(&mut server, "SID=edge", 200, USAGE_BODY, 0).await;
    let store = two_browser_store("chrome", "edge");

    let error = fetch_with_store(&store, &server, &FetchContext::default())
        .await
        .expect_err("the validated account has no windows");

    empty.assert_async().await;
    untried.assert_async().await;
    assert!(matches!(&error, ProviderError::Other(message) if message == NO_WINDOWS));
    assert!(
        store
            .reads()
            .iter()
            .all(|(browser, _)| *browser != BrowserType::Edge),
        "the browser after a validated account must not be read"
    );
}

#[tokio::test]
async fn server_error_and_query_drift_stop_the_search() {
    for (status, body) in [
        (503, "<html>Temporarily unavailable</html>"),
        (200, r#"[{"data":{}}]"#),
        (429, ""),
    ] {
        let mut server = Server::new_async().await;
        let first = mock_session(&mut server, "SID=chrome", status, body, 1).await;
        let untried = mock_session(&mut server, "SID=edge", 200, USAGE_BODY, 0).await;
        let store = two_browser_store("chrome", "edge");

        let error = fetch_with_store(&store, &server, &FetchContext::default())
            .await
            .expect_err("not a rejected session");

        first.assert_async().await;
        untried.assert_async().await;
        assert!(
            !matches!(error, ProviderError::BrowserSignInRequired { .. }),
            "HTTP {status} must keep its own classification, got {error:?}"
        );
    }
}

#[tokio::test]
async fn every_rejected_session_asks_to_sign_in_again() {
    let mut server = Server::new_async().await;
    let chrome = mock_session(&mut server, "SID=chrome", 401, "", 1).await;
    let edge = mock_session(&mut server, "SID=edge", 200, SIGNED_OUT_BODY, 1).await;
    let store = two_browser_store("chrome", "edge");

    let error = fetch_with_store(&store, &server, &FetchContext::default())
        .await
        .expect_err("no account is signed in");

    chrome.assert_async().await;
    edge.assert_async().await;
    assert!(
        matches!(
            &error,
            ProviderError::BrowserSignInRequired { message, sign_in_url }
                if message == SESSION_EXPIRED && sign_in_url == USAGE_PAGE_URL
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn no_browser_session_reports_sign_in_without_a_request() {
    let mut server = Server::new_async().await;
    let none = server
        .mock("GET", Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let store = FakeCookieStore::default().host_cookies(
        BrowserType::Chrome,
        "google.com",
        &[("mail.google.com", "MAIL", "x")],
    );

    let error = fetch_with_store(&store, &server, &FetchContext::default())
        .await
        .expect_err("no cookie applies to gemini.google.com");

    none.assert_async().await;
    assert!(
        matches!(
            &error,
            ProviderError::BrowserSignInRequired { message, .. } if message == MISSING_SESSION
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn missing_cookie_gate_reads_no_browser_and_sends_nothing() {
    let mut server = Server::new_async().await;
    let none = server
        .mock("GET", Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let store = two_browser_store("chrome", "edge");
    let ctx = FetchContext {
        manual_cookie_missing: true,
        ..FetchContext::default()
    };

    fetch_with_store(&store, &server, &ctx)
        .await
        .expect_err("import is forbidden");

    none.assert_async().await;
    assert!(store.reads().is_empty(), "no browser may be read");
}

#[tokio::test]
async fn pasted_cookie_header_is_used_alone() {
    let mut server = Server::new_async().await;
    let pasted = mock_session(&mut server, "SID=pasted", 200, USAGE_BODY, 1).await;
    let web = GeminiWeb::with_base_url(server.url(), Client::new());
    let ctx = FetchContext {
        source_mode: crate::core::SourceMode::Web,
        manual_cookie_header: Some("Cookie: SID=pasted".into()),
        ..FetchContext::default()
    };

    let usage = web.fetch(&ctx).await.expect("pasted session is used");

    pasted.assert_async().await;
    assert_eq!(usage.login_method.as_deref(), Some("Pro"));
}

#[tokio::test]
async fn rejected_pasted_cookie_asks_to_sign_in_again() {
    let mut server = Server::new_async().await;
    let pasted = mock_session(&mut server, "SID=pasted", 403, "", 1).await;
    let web = GeminiWeb::with_base_url(server.url(), Client::new());
    let ctx = FetchContext {
        manual_cookie_header: Some("SID=pasted".into()),
        ..FetchContext::default()
    };

    let error = web.fetch(&ctx).await.expect_err("rejected");

    pasted.assert_async().await;
    assert!(
        matches!(&error, ProviderError::BrowserSignInRequired { message, .. } if message == SESSION_EXPIRED),
        "got {error:?}"
    );
}
