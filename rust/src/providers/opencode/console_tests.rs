//! Console routing for migrated OpenCode workspaces (upstream 0.71
//! `OpenCodeConsoleUsageFetcherTests`), driven against a local mock server
//! with synthetic workspace ids and cookies.

use mockito::{Matcher, Mock, ServerGuard};
use std::time::Duration;

use super::*;

const WORKSPACE_ID: &str = "wrk_CONSOLE123";
const CONSOLE_COOKIE: &str = "__Host-console_session=synthetic-session";
const BOTH_COOKIES: &str = "__Host-console_session=synthetic-session; auth=synthetic-legacy";
const ORGS_PATH: &str = "/console/api/orgs";
const GO_PATH: &str = "/console/api/go/status";
const ORG_PATH: &str = "/console/api/orgs/current";
const BILLING_PATH: &str = "/console/api/billing/status";
const SUMMARY_PATH: &str = "/console/api/usage/summary";
const QUOTA: &str = r#"{"access":{"endsAt":"2026-10-19T00:00:00Z","meters":{
  "fiveHour":{"limitMicroCents":"1200000000","usedMicroCents":"300000000","resetsAt":"2026-09-20T03:00:00Z"},
  "week":{"limitMicroCents":"3000000000","usedMicroCents":"1200000000","resetsAt":"2026-09-21T00:00:00Z"},
  "month":{"limitMicroCents":"6000000000","usedMicroCents":"600000000"}}}}"#;
const LEGACY_QUOTA: &str = r#"{"rollingUsage":{"usagePercent":17,"resetInSec":600},
 "weeklyUsage":{"usagePercent":75,"resetInSec":7200}}"#;
const PAYG_BILLING: &str = r#"{"billingMode":"prepaid","mode":"pay-as-you-go","balanceMicroCents":"1250000000",
 "creditLimitMicroCents":"99900000000","dailyLimitMicroCents":"2000000000"}"#;

async fn console_mock(
    server: &mut ServerGuard,
    path: &str,
    status: usize,
    body: &str,
    hits: usize,
) -> Mock {
    let workspace = if path == ORGS_PATH {
        Matcher::Missing
    } else {
        Matcher::Exact(WORKSPACE_ID.to_string())
    };
    let mut mock = server
        .mock("GET", path)
        .match_header("x-org-id", workspace)
        .match_header("cookie", Matcher::Regex("__Host-console_session=".into()));
    if path == SUMMARY_PATH {
        mock = mock.match_query(Matcher::UrlEncoded("range".into(), "30d".into()));
    }
    mock.with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(hits)
        .create_async()
        .await
}

async fn legacy_mock(server: &mut ServerGuard, body: &str, hits: usize) -> Mock {
    server
        .mock("GET", "/_server")
        .match_query(Matcher::Any)
        .with_status(200)
        .with_body(body)
        .expect(hits)
        .create_async()
        .await
}

async fn fetch(server: &ServerGuard, cookie: &str) -> Result<ProviderFetchResult, ProviderError> {
    OpenCodeProvider::with_origin(server.url())
        .fetch_with_cookies(cookie, Duration::from_secs(5))
        .await
}

async fn payg_mocks(server: &mut ServerGuard, go_status: &str, billing: &str) -> Vec<Mock> {
    vec![
        console_mock(server, ORGS_PATH, 200, r#"[{"id":"wrk_CONSOLE123"}]"#, 1).await,
        console_mock(server, GO_PATH, 200, go_status, 1).await,
        console_mock(
            server,
            ORG_PATH,
            200,
            r#"{"id":"wrk_CONSOLE123","hasGoSubscription":false}"#,
            1,
        )
        .await,
        console_mock(server, BILLING_PATH, 200, billing, 1).await,
        console_mock(
            server,
            SUMMARY_PATH,
            200,
            r#"{"totalCostMicroCents":"325000000","totalRequests":12}"#,
            1,
        )
        .await,
    ]
}

#[tokio::test]
async fn console_session_discovers_its_workspace_and_reads_quota() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        200,
        r#"[{"id":"wrk_CONSOLE123"},{"id":"wrk_OTHER456"}]"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 200, QUOTA, 1).await;
    let legacy = legacy_mock(&mut server, LEGACY_QUOTA, 0).await;

    let result = fetch(&server, CONSOLE_COOKIE).await.expect("console quota");

    let usage = &result.usage;
    assert!((usage.primary.used_percent - 25.0).abs() < 1e-9);
    assert_eq!(usage.primary.window_minutes, Some(300));
    assert!((usage.secondary.as_ref().unwrap().used_percent - 40.0).abs() < 1e-9);
    assert!(
        usage.tertiary.is_none(),
        "monthly Go meter stays in OpenCode Go"
    );
    assert_eq!(usage.login_method.as_deref(), Some("OpenCode"));
    assert!(result.cost.is_none());
    for mock in [orgs, go, legacy] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn payg_reads_stay_in_the_workspace_and_use_thirty_day_microcents() {
    for go_status in [r#"{"access":null}"#, "null"] {
        let mut server = mockito::Server::new_async().await;
        let mocks = payg_mocks(&mut server, go_status, PAYG_BILLING).await;

        let result = fetch(&server, CONSOLE_COOKIE).await.expect(go_status);

        let usage = &result.usage;
        assert!(usage.primary.is_informational, "{go_status}");
        assert_eq!(usage.primary.used_percent, 0.0);
        assert_eq!(usage.primary.window_minutes, None);
        assert_eq!(
            usage.primary.reset_description.as_deref(),
            Some("$3.25 over last 30 days")
        );
        assert_eq!(
            usage.login_method.as_deref(),
            Some("Pay-as-you-go · $12.50 prepaid")
        );
        let cost = result.cost.as_ref().expect("30-day spend cost row");
        assert!((cost.used - 3.25).abs() < 1e-9);
        assert_eq!(cost.limit, None);
        assert_eq!(cost.period, "Last 30 days");
        for mock in mocks {
            mock.assert_async().await;
        }
    }
}

#[tokio::test]
async fn missing_or_malformed_balance_keeps_spend_for_confirmed_payg() {
    for billing in [
        r#"{"billingMode":"prepaid","mode":"pay-as-you-go"}"#,
        r#"{"billingMode":"prepaid","mode":"pay-as-you-go","balanceMicroCents":"invalid"}"#,
    ] {
        let mut server = mockito::Server::new_async().await;
        let _mocks = payg_mocks(&mut server, "null", billing).await;

        let result = fetch(&server, CONSOLE_COOKIE).await.expect(billing);

        assert_eq!(result.usage.login_method.as_deref(), Some("Pay-as-you-go"));
        assert!((result.cost.as_ref().unwrap().used - 3.25).abs() < 1e-9);
    }
}

#[tokio::test]
async fn subscribed_org_with_null_quota_is_not_presented_as_spend() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        200,
        r#"[{"id":"wrk_CONSOLE123"}]"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 200, r#"{"access":null}"#, 1).await;
    let org = console_mock(
        &mut server,
        ORG_PATH,
        200,
        r#"{"hasGoSubscription":true}"#,
        1,
    )
    .await;
    let billing = console_mock(&mut server, BILLING_PATH, 200, PAYG_BILLING, 0).await;

    let error = fetch(&server, CONSOLE_COOKIE).await.unwrap_err();

    assert!(matches!(error, ProviderError::Parse(_)), "{error:?}");
    for mock in [orgs, go, org, billing] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn invoiceable_account_is_not_rendered_as_payg_spend() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        200,
        r#"[{"id":"wrk_CONSOLE123"}]"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 200, "null", 1).await;
    let org = console_mock(
        &mut server,
        ORG_PATH,
        200,
        r#"{"hasGoSubscription":false}"#,
        1,
    )
    .await;
    let billing = console_mock(
        &mut server,
        BILLING_PATH,
        200,
        r#"{"billingMode":"credit","mode":"invoiceable"}"#,
        1,
    )
    .await;
    let summary = console_mock(&mut server, SUMMARY_PATH, 200, "{}", 0).await;

    let error = fetch(&server, CONSOLE_COOKIE).await.unwrap_err();

    assert!(matches!(error, ProviderError::Parse(_)), "{error:?}");
    for mock in [orgs, go, org, billing, summary] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn console_unauthorized_can_fall_back_to_valid_legacy_quota() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        200,
        r#"[{"id":"wrk_CONSOLE123"}]"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 401, r#"{"error":"unauthorized"}"#, 1).await;
    let legacy = server
        .mock("GET", "/_server")
        .match_query(Matcher::UrlEncoded(
            "id".into(),
            SUBSCRIPTION_SERVER_ID.into(),
        ))
        .match_header("referer", Matcher::Regex(WORKSPACE_ID.into()))
        .with_status(200)
        .with_body(LEGACY_QUOTA)
        .expect(1)
        .create_async()
        .await;

    let result = fetch(&server, BOTH_COOKIES).await.expect("legacy quota");

    assert!((result.usage.primary.used_percent - 17.0).abs() < 1e-9);
    assert!((result.usage.secondary.as_ref().unwrap().used_percent - 75.0).abs() < 1e-9);
    for mock in [orgs, go, legacy] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn console_api_errors_survive_a_signed_out_legacy_fallback() {
    for status in [403, 500] {
        let mut server = mockito::Server::new_async().await;
        let _orgs = console_mock(
            &mut server,
            ORGS_PATH,
            200,
            r#"[{"id":"wrk_CONSOLE123"}]"#,
            1,
        )
        .await;
        let _go = console_mock(
            &mut server,
            GO_PATH,
            status,
            r#"{"error":"not associated with an account"}"#,
            1,
        )
        .await;
        let legacy = legacy_mock(
            &mut server,
            "<html><body>Please sign in to continue</body></html>",
            1,
        )
        .await;

        let error = fetch(&server, BOTH_COOKIES).await.unwrap_err();

        assert!(
            matches!(&error, ProviderError::Other(message)
                if message.starts_with("OpenCode Console Go status returned")
                    && message.contains(&status.to_string())),
            "{status}: {error:?}"
        );
        legacy.assert_async().await;
    }
}

#[tokio::test]
async fn console_only_credentials_never_trigger_a_legacy_request() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        200,
        r#"[{"id":"wrk_CONSOLE123"}]"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 401, r#"{"error":"unauthorized"}"#, 1).await;
    let legacy = legacy_mock(&mut server, LEGACY_QUOTA, 0).await;

    let error = fetch(&server, CONSOLE_COOKIE).await.unwrap_err();

    assert!(matches!(error, ProviderError::AuthRequired), "{error:?}");
    for mock in [orgs, go, legacy] {
        mock.assert_async().await;
    }
}

async fn legacy_discovery_mocks(server: &mut ServerGuard) -> Vec<Mock> {
    vec![
        server
            .mock("GET", "/_server")
            .match_query(Matcher::UrlEncoded(
                "id".into(),
                WORKSPACES_SERVER_ID.into(),
            ))
            .with_status(200)
            .with_body(r#"[{id:"wrk_LEGACY1"}]"#)
            .expect(1)
            .create_async()
            .await,
        server
            .mock("GET", "/_server")
            .match_query(Matcher::UrlEncoded(
                "id".into(),
                SUBSCRIPTION_SERVER_ID.into(),
            ))
            .match_header("referer", Matcher::Regex("wrk_LEGACY1".into()))
            .with_status(200)
            .with_body(LEGACY_QUOTA)
            .expect(1)
            .create_async()
            .await,
    ]
}

#[tokio::test]
async fn console_workspace_discovery_failure_switches_to_the_legacy_path() {
    for (status, body) in [
        (401, r#"{"error":"unauthorized"}"#),
        (500, r#"{"error":"internal"}"#),
        (200, "[]"),
    ] {
        let mut server = mockito::Server::new_async().await;
        let orgs = console_mock(&mut server, ORGS_PATH, status, body, 1).await;
        let go = console_mock(&mut server, GO_PATH, 200, QUOTA, 0).await;
        let legacy = legacy_discovery_mocks(&mut server).await;

        let result = fetch(&server, BOTH_COOKIES)
            .await
            .unwrap_or_else(|error| panic!("{status} {body}: {error:?}"));

        assert!(
            (result.usage.primary.used_percent - 17.0).abs() < 1e-9,
            "{status} {body}"
        );
        assert!((result.usage.secondary.as_ref().unwrap().used_percent - 75.0).abs() < 1e-9);
        for mock in [orgs, go].into_iter().chain(legacy) {
            mock.assert_async().await;
        }
    }
}

#[tokio::test]
async fn console_only_discovery_unauthorized_is_auth_required_without_legacy() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(
        &mut server,
        ORGS_PATH,
        401,
        r#"{"error":"unauthorized"}"#,
        1,
    )
    .await;
    let go = console_mock(&mut server, GO_PATH, 200, QUOTA, 0).await;
    let legacy = legacy_mock(&mut server, LEGACY_QUOTA, 0).await;

    let error = fetch(&server, CONSOLE_COOKIE).await.unwrap_err();

    assert!(matches!(error, ProviderError::AuthRequired), "{error:?}");
    for mock in [orgs, go, legacy] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn legacy_only_cookies_keep_the_server_function_path() {
    let mut server = mockito::Server::new_async().await;
    let orgs = console_mock(&mut server, ORGS_PATH, 200, "[]", 0).await;
    let workspaces = server
        .mock("GET", "/_server")
        .match_query(Matcher::UrlEncoded(
            "id".into(),
            WORKSPACES_SERVER_ID.into(),
        ))
        .with_status(200)
        .with_body(r#"[{id:"wrk_LEGACY1"}]"#)
        .expect(1)
        .create_async()
        .await;
    let subscription = server
        .mock("GET", "/_server")
        .match_query(Matcher::UrlEncoded(
            "id".into(),
            SUBSCRIPTION_SERVER_ID.into(),
        ))
        .with_status(200)
        .with_body(LEGACY_QUOTA)
        .expect(1)
        .create_async()
        .await;

    let result = fetch(&server, "auth=synthetic-legacy")
        .await
        .expect("legacy quota");

    assert!((result.usage.primary.used_percent - 17.0).abs() < 1e-9);
    for mock in [orgs, workspaces, subscription] {
        mock.assert_async().await;
    }
}
