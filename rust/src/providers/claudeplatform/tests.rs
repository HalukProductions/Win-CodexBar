//! Fixture tests ported from upstream `TestsPlugin/ClaudePlatformPluginTests.swift`.
//! Every fetch runs against a local mock Console with a pasted Cookie header
//! or an in-memory browser store, so no test reads a real browser or account.

use super::*;
use crate::browser::detection::BrowserType;
use crate::browser::session_scan::test_support::FakeCookieStore;
use mockito::{Matcher, Mock, Server, ServerGuard};

/// 2026-10-13T00:00:00Z.
const NOW: i64 = 1_791_849_600;
const CHAT_ORG: &str = "11111111-1111-4111-8111-111111111111";
const API_ORG: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_API_ORG: &str = "33333333-3333-4333-8333-333333333333";
const COOKIE_1: &str = "sessionKey=sk-ant-sid01-synthetic-1";
const ERROR_BODY: &str =
    r#"{"type":"error","error":{"type":"permission_error","message":"Invalid authorization"}}"#;

fn organizations() -> String {
    format!(
        r#"[{{"uuid":"{CHAT_ORG}","name":"Synthetic Chat Organization","capabilities":["chat","claude_max"]}},
 {{"uuid":"{API_ORG}","name":"Synthetic API Org","capabilities":["api","api_individual","customer_terms:standard"]}}]"#
    )
}

const CREDITS: &str = r#"{"amount":18725,"currency":"USD","balance":{"money":null,"credits":{"amount_minor":18725,"exponent":2}},
 "balance_credits":187,"amount_without_scoped_credits":6250,"auto_reload_settings":null,
 "last_paid_purchase_cents":10000,
 "tranches":[{"remaining_amount_minor_units":6250,"currency":"USD","expires_at":"2027-05-01T00:00:00Z",
   "granted_amount_minor_units":10000,"granted_at":"2026-05-01T00:00:00Z","program_id":"api_credits",
   "name":null,"id":"fixture-purchase","scope":{"kind":"unscoped","applies_to":[]}}],
 "promo_tranches":[{"remaining_amount_minor_units":12475,"currency":"USD","expires_at":"2026-10-28T00:00:00Z",
   "granted_amount_minor_units":20000,"granted_at":"2026-10-01T00:00:00Z",
   "program_id":"subscription_api_credit","name":"API credit (Max 20x)","id":"fixture-promo",
   "scope":{"kind":"scoped","applies_to":[{"product":"api"},{"product":"agent_sdk"}]}}],
 "next_expires_at":"2026-10-28T00:00:00Z"}"#;
const SPEND: &str = r#"{"amount":4321,"resets_at":"2026-11-01T00:00:00Z"}"#;

fn credits_with(exponent: i64, amount: i64, promo: &[String], tranches: &[String]) -> String {
    format!(
        r#"{{"amount":{amount},"currency":"USD","balance":{{"money":null,"credits":{{"amount_minor":{amount},"exponent":{exponent}}}}},"promo_tranches":[{}],"tranches":[{}]}}"#,
        promo.join(","),
        tranches.join(",")
    )
}

fn grant(
    name: Option<&str>,
    program: &str,
    remaining: i64,
    granted: i64,
    expires: Option<&str>,
) -> String {
    let name = name.map_or("null".to_owned(), |name| format!("\"{name}\""));
    let expires = expires.map_or("null".to_owned(), |date| format!("\"{date}\""));
    format!(
        r#"{{"remaining_amount_minor_units":{remaining},"granted_amount_minor_units":{granted},"currency":"USD","expires_at":{expires},"program_id":"{program}","name":{name}}}"#
    )
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(NOW, 0).unwrap()
}

fn manual(cookie: &str) -> FetchContext {
    FetchContext {
        source_mode: SourceMode::Auto,
        manual_cookie_header: Some(cookie.to_owned()),
        ..FetchContext::default()
    }
}

fn credits_path(org: &str) -> String {
    format!("/api/organizations/{org}/prepaid/credits")
}

fn spend_path(org: &str) -> String {
    format!("/api/organizations/{org}/current_spend")
}

async fn mock(server: &mut ServerGuard, path: &str, status: usize, body: &str) -> Mock {
    let body = if status == 200 { body } else { ERROR_BODY };
    server
        .mock("GET", path)
        .match_header("accept", "application/json")
        .match_header(
            "cookie",
            Matcher::Regex("sessionKey=sk-ant-sid01-synthetic-".into()),
        )
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await
}

/// The default upstream fixture: organizations, credits, spend.
async fn console(server: &mut ServerGuard, credits: &str, spend: (usize, &str)) -> Vec<Mock> {
    vec![
        mock(server, "/api/organizations", 200, &organizations()).await,
        mock(server, &credits_path(API_ORG), 200, credits).await,
        mock(server, &spend_path(API_ORG), spend.0, spend.1).await,
    ]
}

async fn fetch(
    server: &ServerGuard,
    ctx: &FetchContext,
) -> Result<ProviderFetchResult, ProviderError> {
    let store = FakeCookieStore::default();
    ClaudePlatformProvider::with_base_url(&server.url())
        .fetch_web(ctx, &store, now())
        .await
}

fn row<'a>(result: &'a ProviderFetchResult, title: &str) -> Option<&'a ProviderDisplayDetail> {
    result
        .display_details
        .iter()
        .find(|detail| detail.title() == title)
}

fn rows_in<'a>(result: &'a ProviderFetchResult, section: &str) -> Vec<&'a ProviderDisplayDetail> {
    result
        .display_details
        .iter()
        .filter(|detail| detail.section_title() == Some(section))
        .collect()
}

fn progress(detail: &ProviderDisplayDetail) -> Option<f64> {
    detail
        .progress()
        .map(|progress| progress.used() / progress.total() * 100.0)
}

fn description(window: &RateWindow) -> Option<&str> {
    window.reset_description.as_deref()
}

fn assert_other(result: Result<ProviderFetchResult, ProviderError>, expected: &str) {
    match result {
        Err(ProviderError::Other(message)) => assert_eq!(message, expected),
        other => panic!("expected Other({expected:?}), got {other:?}"),
    }
}

#[test]
fn metadata_and_source_contract_match_upstream_descriptor() {
    let provider = ClaudePlatformProvider::new();
    assert_eq!(provider.id(), ProviderId::ClaudePlatform);
    let metadata = provider.metadata();
    assert_eq!(metadata.display_name, "Claude Platform");
    assert_eq!(metadata.session_label, "API credit");
    assert_eq!(metadata.weekly_label, "Purchased credit");
    assert!(!metadata.default_enabled);
    assert_eq!(metadata.dashboard_url, Some(DASHBOARD_URL));
    assert_eq!(metadata.status_page_url, Some(STATUS_PAGE_URL));
    assert_eq!(
        provider.available_sources(),
        vec![SourceMode::Auto, SourceMode::Web]
    );
    assert!(provider.supports_web());
    assert!(provider.owns_browser_cookie_resolution());
    assert_eq!(
        provider.manual_empty_cookie_policy(),
        ManualEmptyCookiePolicy::FailClosedWeb
    );
    assert_eq!(
        ProviderId::ClaudePlatform.cookie_domain(),
        Some("platform.claude.com")
    );
    assert_eq!(
        ProviderId::from_cli_name("claudeplatform"),
        Some(ProviderId::ClaudePlatform)
    );
    assert_eq!(
        ProviderId::from_cli_name("claude-platform"),
        Some(ProviderId::ClaudePlatform)
    );
}

#[tokio::test]
async fn console_credits_map_balance_spend_and_expiring_grants() {
    let mut server = Server::new_async().await;
    let mocks = console(&mut server, CREDITS, (200, SPEND)).await;
    let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();
    for mock in &mocks {
        mock.assert_async().await;
    }

    let cost = result.cost.as_ref().expect("prepaid cost");
    assert_eq!(cost.balance, Some(187.25));
    assert_eq!(cost.used, 187.25);
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.period, "Prepaid credits");
    assert!(cost.resets_at.is_none());

    let usage = &result.usage;
    assert!((usage.primary.used_percent - 37.625).abs() < 1e-6);
    assert!(usage.primary.resets_at.is_none());
    assert_eq!(
        description(&usage.primary),
        Some("$124.75 of $200.00 left · Expires Oct 28 (UTC)")
    );
    let purchased = usage.secondary.as_ref().expect("purchased lane");
    assert!((purchased.used_percent - 37.5).abs() < 1e-6);
    assert_eq!(
        description(purchased),
        Some("$62.50 of $100.00 left · Expires May 1, 2027 (UTC)")
    );

    let sections: Vec<_> = result
        .display_details
        .iter()
        .filter_map(ProviderDisplayDetail::section_title)
        .collect();
    assert_eq!(sections, vec![BILLING_SECTION]);
    assert_eq!(row(&result, "This month (UTC)").unwrap().value(), "$43.21");
    assert_eq!(
        usage.account_organization.as_deref(),
        Some("Synthetic API Org")
    );
    assert!(usage.account_email.is_none());
    assert_eq!(usage.login_method.as_deref(), Some("Console session"));
    assert_eq!(result.source_label, "web");
}

#[tokio::test]
async fn a_forbidden_api_organization_falls_through_to_the_next_readable_one() {
    let mut server = Server::new_async().await;
    let organizations = format!(
        r#"[{{"uuid":"{CHAT_ORG}","name":"Synthetic Chat Organization","capabilities":["chat"]}},
 {{"uuid":"{API_ORG}","name":"Synthetic Member Org","capabilities":["api"]}},
 {{"uuid":"{OTHER_API_ORG}","name":"Synthetic Owner Org","capabilities":["api"]}}]"#
    );
    let mocks = vec![
        mock(&mut server, "/api/organizations", 200, &organizations).await,
        mock(&mut server, &credits_path(API_ORG), 403, "").await,
        mock(&mut server, &credits_path(OTHER_API_ORG), 200, CREDITS).await,
        mock(&mut server, &spend_path(OTHER_API_ORG), 200, SPEND).await,
    ];
    let untouched = server
        .mock("GET", spend_path(API_ORG).as_str())
        .expect(0)
        .create_async()
        .await;
    let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();
    for mock in &mocks {
        mock.assert_async().await;
    }
    untouched.assert_async().await;
    assert_eq!(result.cost.as_ref().unwrap().balance, Some(187.25));
    assert_eq!(
        result.usage.account_organization.as_deref(),
        Some("Synthetic Owner Org")
    );
}

#[tokio::test]
async fn subscription_lane_uses_the_soonest_grant_with_credit_and_lists_the_rest() {
    let credits = credits_with(
        2,
        18725,
        &[
            grant(
                Some("API credit (Max 20x)"),
                SUBSCRIPTION_PROGRAM,
                15000,
                20000,
                Some("2026-11-12T00:00:00Z"),
            ),
            grant(
                Some("API credit (Pro)"),
                SUBSCRIPTION_PROGRAM,
                0,
                5000,
                Some("2026-10-20T00:00:00Z"),
            ),
            grant(
                Some("Launch credit"),
                "promo",
                1000,
                1000,
                Some("2026-10-31Tnot-a-date"),
            ),
        ],
        &[grant(None, "api_credits", 2500, 5000, None)],
    );
    let mut server = Server::new_async().await;
    let _mocks = console(&mut server, &credits, (200, SPEND)).await;
    let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();

    let usage = &result.usage;
    assert!((usage.primary.used_percent - 25.0).abs() < 1e-6);
    assert_eq!(
        description(&usage.primary),
        Some("$150.00 of $200.00 left · Expires Nov 12 (UTC)")
    );
    let purchased = usage.secondary.as_ref().unwrap();
    assert!((purchased.used_percent - 50.0).abs() < 1e-6);
    assert_eq!(description(purchased), Some("$25.00 of $50.00 left"));

    let others = rows_in(&result, OTHER_GRANTS_SECTION);
    let labels: Vec<_> = others.iter().map(|row| row.title()).collect();
    assert_eq!(labels, vec!["API credit (Pro)", "Launch credit"]);
    assert_eq!(others[0].value(), "$0.00 of $50.00 left");
    assert_eq!(others[0].secondary_value(), Some("Expires Oct 20 (UTC)"));
    assert_eq!(progress(others[0]), Some(100.0));
    // An unparseable expiry is unknown, not a failed refresh.
    assert_eq!(others[1].value(), "$10.00 of $10.00 left");
    assert_eq!(others[1].secondary_value(), None);
    assert_eq!(progress(others[1]), Some(0.0));
}

#[tokio::test]
async fn expired_grants_drop_out_and_exhausted_subscription_credit_leaves_no_api_credit_lane() {
    let credits = credits_with(
        2,
        18725,
        &[
            // Expiring exactly now counts as expired.
            grant(
                Some("API credit (Max 5x)"),
                SUBSCRIPTION_PROGRAM,
                5000,
                10000,
                Some("2026-10-13T00:00:00Z"),
            ),
            grant(
                Some("API credit (Pro)"),
                SUBSCRIPTION_PROGRAM,
                0,
                5000,
                Some("2026-10-20T00:00:00Z"),
            ),
        ],
        &[
            grant(
                None,
                "api_credits",
                4000,
                4000,
                Some("2026-10-12T00:00:00Z"),
            ),
            grant(None, "api_credits", 2500, 5000, None),
        ],
    );
    let mut server = Server::new_async().await;
    let _mocks = console(&mut server, &credits, (200, SPEND)).await;
    let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();

    // No subscription lane: the primary is the hidden balance placeholder.
    assert!(result.usage.primary.is_informational);
    assert!(result.usage.primary.hidden_on_card);
    assert_eq!(
        result.usage.primary.reset_description.as_deref(),
        Some("Balance: $187.25")
    );
    assert_eq!(
        description(result.usage.secondary.as_ref().unwrap()),
        Some("$25.00 of $50.00 left")
    );
    let labels: Vec<_> = rows_in(&result, OTHER_GRANTS_SECTION)
        .iter()
        .map(|row| row.title())
        .collect();
    assert_eq!(labels, vec!["API credit (Pro)"]);
}

#[tokio::test]
async fn amounts_use_the_reported_credit_exponent() {
    let credits = credits_with(
        3,
        187_250,
        &[grant(
            None,
            SUBSCRIPTION_PROGRAM,
            124_750,
            200_000,
            Some("2026-10-28T00:00:00Z"),
        )],
        &[],
    );
    let mut server = Server::new_async().await;
    let _mocks = console(&mut server, &credits, (200, r#"{"amount":43210}"#)).await;
    let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();
    assert_eq!(result.cost.as_ref().unwrap().balance, Some(187.25));
    assert_eq!(
        description(&result.usage.primary),
        Some("$124.75 of $200.00 left · Expires Oct 28 (UTC)")
    );
    assert_eq!(row(&result, "This month (UTC)").unwrap().value(), "$43.21");
}

#[tokio::test]
async fn accounts_without_an_api_organization_fail_before_billing_requests() {
    let mut server = Server::new_async().await;
    let organizations = format!(
        r#"[{{"uuid":"{CHAT_ORG}","name":"Synthetic Chat Organization","capabilities":["chat"]}}]"#
    );
    let list = mock(&mut server, "/api/organizations", 200, &organizations).await;
    let billing = server
        .mock("GET", Matcher::Regex("/prepaid/".into()))
        .expect(0)
        .create_async()
        .await;
    assert_other(fetch(&server, &manual(COOKIE_1)).await, NO_API_ORGANIZATION);
    list.assert_async().await;
    billing.assert_async().await;
}

#[tokio::test]
async fn rejected_sessions_report_sign_in_guidance_after_every_profile() {
    for status in [401, 403] {
        let mut server = Server::new_async().await;
        let list = mock(&mut server, "/api/organizations", status, "")
            .await
            .expect(2);
        let store = FakeCookieStore::default()
            .cookies(
                BrowserType::Chrome,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-1")],
            )
            .cookies(
                BrowserType::Edge,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-2")],
            );
        let provider = ClaudePlatformProvider::with_base_url(&server.url());
        let ctx = FetchContext::default();
        let error = provider.fetch_web(&ctx, &store, now()).await.unwrap_err();
        assert!(matches!(&error, ProviderError::Other(message) if message == SESSION_EXPIRED));
        assert!(SESSION_EXPIRED.contains("Sign in at platform.claude.com"));
        assert_eq!(
            provider.error_state_kind(&error),
            ProviderStateKind::ExpiredSession
        );
        list.assert_async().await;
    }
}

#[tokio::test]
async fn an_unauthorized_profile_advances_to_the_next_profile() {
    for unauthorized in ["/api/organizations".to_owned(), credits_path(API_ORG)] {
        let mut server = Server::new_async().await;
        let first = server
            .mock("GET", unauthorized.as_str())
            .match_header("cookie", "sessionKey=sk-ant-sid01-synthetic-1")
            .with_status(401)
            .with_body(ERROR_BODY)
            .create_async()
            .await;
        if unauthorized != "/api/organizations" {
            server
                .mock("GET", "/api/organizations")
                .match_header("cookie", "sessionKey=sk-ant-sid01-synthetic-1")
                .with_status(200)
                .with_body(organizations())
                .create_async()
                .await;
        }
        let _mocks = console(&mut server, CREDITS, (200, SPEND)).await;
        let store = FakeCookieStore::default()
            .cookies(
                BrowserType::Chrome,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-1")],
            )
            .cookies(
                BrowserType::Edge,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-2")],
            );
        let provider = ClaudePlatformProvider::with_base_url(&server.url());
        let result = provider
            .fetch_web(&FetchContext::default(), &store, now())
            .await
            .unwrap();
        first.assert_async().await;
        assert_eq!(result.cost.as_ref().unwrap().balance, Some(187.25));
        assert_eq!(store.reads().len(), 2);
    }
}

#[tokio::test]
async fn browser_sessions_without_session_key_are_skipped() {
    let mut server = Server::new_async().await;
    let _mocks = console(&mut server, CREDITS, (200, SPEND)).await;
    let store = FakeCookieStore::default()
        .cookies(BrowserType::Chrome, COOKIE_DOMAIN, &[("consent", "yes")])
        .cookies(
            BrowserType::Edge,
            COOKIE_DOMAIN,
            &[("sessionKey", "sk-ant-sid01-synthetic-2")],
        );
    let result = ClaudePlatformProvider::with_base_url(&server.url())
        .fetch_web(&FetchContext::default(), &store, now())
        .await
        .unwrap();
    assert_eq!(result.cost.as_ref().unwrap().balance, Some(187.25));
}

#[tokio::test]
async fn credit_balance_failures_keep_the_session() {
    for (status, expected) in [
        (403, BILLING_DENIED),
        (429, RATE_LIMITED),
        (500, UNAVAILABLE),
    ] {
        let mut server = Server::new_async().await;
        let _list = mock(&mut server, "/api/organizations", 200, &organizations()).await;
        let _credits = mock(&mut server, &credits_path(API_ORG), status, "").await;
        // A second browser would only be tried after a rejected session.
        let store = FakeCookieStore::default()
            .cookies(
                BrowserType::Chrome,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-1")],
            )
            .cookies(
                BrowserType::Edge,
                COOKIE_DOMAIN,
                &[("sessionKey", "sk-ant-sid01-synthetic-2")],
            );
        let result = ClaudePlatformProvider::with_base_url(&server.url())
            .fetch_web(&FetchContext::default(), &store, now())
            .await;
        assert_other(result, expected);
        assert_eq!(store.reads().len(), 1);
    }
}

#[tokio::test]
async fn unavailable_spend_keeps_the_prepaid_balance() {
    for (status, body) in [
        (500, "{}"),
        (200, "not-json"),
        (200, r#"{"amount":"4321"}"#),
        (200, r#"{"amount":-1}"#),
        (401, "{}"),
        (403, "{}"),
        (429, "{}"),
    ] {
        let mut server = Server::new_async().await;
        let _mocks = console(&mut server, CREDITS, (status, body)).await;
        let result = fetch(&server, &manual(COOKIE_1)).await.unwrap();
        let cost = result.cost.as_ref().unwrap();
        assert_eq!(cost.balance, Some(187.25));
        assert_eq!(cost.used, 187.25);
        assert_eq!(cost.period, "Prepaid credits");
        assert!(!result.usage.primary.is_informational);
        assert_eq!(row(&result, "Spend").unwrap().value(), "Unavailable");
        assert!(row(&result, "This month (UTC)").is_none());
    }
}

#[tokio::test]
async fn malformed_required_credit_balance_fails_closed() {
    for credits in [
        r#"{"amount":null,"currency":"USD"}"#,
        r#"{"amount":"18725","currency":"USD"}"#,
        r#"{"amount":187.25,"currency":"USD"}"#,
        r#"{"amount":1e99,"currency":"USD"}"#,
        r#"{"amount":18725,"currency":"USD","tranches":{}}"#,
        r#"{"amount":18725,"currency":"usd"}"#,
        r#"{"amount":18725,"balance":{"credits":{"exponent":7}}}"#,
        "not-json",
    ] {
        let mut server = Server::new_async().await;
        let _list = mock(&mut server, "/api/organizations", 200, &organizations()).await;
        let _credits = mock(&mut server, &credits_path(API_ORG), 200, credits).await;
        let result = fetch(&server, &manual(COOKIE_1)).await;
        assert!(
            matches!(result, Err(ProviderError::Parse(_))),
            "{credits}: {result:?}"
        );
    }
}

#[tokio::test]
async fn a_rejected_manual_header_reports_an_expired_session() {
    let mut server = Server::new_async().await;
    let _list = mock(&mut server, "/api/organizations", 403, "").await;
    assert_other(fetch(&server, &manual(COOKIE_1)).await, SESSION_EXPIRED);
}

#[tokio::test]
async fn missing_cookies_ask_for_a_browser_sign_in_without_network() {
    let server = Server::new_async().await;
    let empty_manual = FetchContext {
        manual_cookie_missing: true,
        ..FetchContext::default()
    };
    for ctx in [empty_manual, FetchContext::default()] {
        let result = fetch(&server, &ctx).await;
        assert!(
            matches!(result, Err(ProviderError::BrowserSignInRequired { .. })),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn disabled_cookies_never_read_a_browser() {
    let provider = ClaudePlatformProvider::new();
    let ctx = FetchContext {
        source_mode: SourceMode::Cli,
        ..FetchContext::default()
    };
    assert_other(provider.fetch_usage(&ctx).await, COOKIES_OFF);
}

#[test]
fn safe_integers_reject_strings_fractions_and_huge_numbers() {
    let parse = |raw: &str| safe_integer(Some(&serde_json::from_str::<Value>(raw).unwrap()));
    assert_eq!(parse("18725"), Some(18725));
    assert_eq!(parse("1.0e3"), Some(1000));
    assert_eq!(parse("9007199254740991"), Some(9_007_199_254_740_991));
    assert_eq!(parse("9007199254740992"), None);
    assert_eq!(parse("187.25"), None);
    assert_eq!(parse("1e99"), None);
    assert_eq!(parse("\"18725\""), None);
    assert_eq!(parse("null"), None);
}

#[test]
fn organization_ids_must_be_safe_path_segments() {
    let list = serde_json::json!([{"uuid":"../evil","capabilities":["api"]}]);
    assert!(matches!(
        api_organizations(&list),
        Err(ProviderError::Parse(_))
    ));
}
