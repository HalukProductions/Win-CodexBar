//! Fixture tests for the Munito spend limits provider, ported from upstream
//! `TestsPlugin/MunitoPluginTests.swift`. Fetch tests use an injected API key
//! and a local mock server, so no test reads the keyring, the environment, or
//! the real Munito API.

use super::*;
use mockito::{Matcher, Server, ServerGuard};

const KEY: &str = "fixture-key";
const LIMITS_PATH: &str = "/admin/v1/limits";
const ORGANIZATION_PATH: &str = "/admin/v1/organization";

/// Documented `GET /admin/v1/limits` schema with synthetic values.
const LIMITS_FIXTURE: &str = concat!(
    r#"{"object":"spend_limits","usage_tier":"fixture-tier","periods":["#,
    r#"{"period":"day","spent_usd":4.25,"limit_usd":50,"organization_limit_usd":500,"account_limit_usd":40,"#,
    r#""stopped_until":null},"#,
    r#"{"period":"month","spent_usd":123.4,"limit_usd":1000,"organization_limit_usd":5000,"#,
    r#""account_limit_usd":900}]}"#
);
const ORGANIZATION_FIXTURE: &str = concat!(
    r#"{"object":"organization","id":"org_fixture","name":"Fixture Org","status":"active","#,
    r#""usage_tier":"fixture-tier","data_region":"eu","created_at":"2026-01-01T00:00:00Z"}"#
);

fn context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        source_mode,
        api_key: Some(KEY.to_string()),
        ..FetchContext::default()
    }
}

async fn serve(
    body: &str,
    code: usize,
    organization_body: &str,
    organization_code: usize,
) -> ServerGuard {
    let mut server = Server::new_async().await;
    for (path, body, code) in [
        (LIMITS_PATH, body, code),
        (ORGANIZATION_PATH, organization_body, organization_code),
    ] {
        server
            .mock("GET", path)
            .match_header("authorization", format!("Bearer {KEY}").as_str())
            .match_header("cookie", Matcher::Missing)
            .with_status(code)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;
    }
    server
}

async fn fetch_with(
    body: &str,
    code: usize,
    organization_body: &str,
    organization_code: usize,
) -> Result<ProviderFetchResult, ProviderError> {
    let server = serve(body, code, organization_body, organization_code).await;
    MunitoProvider::with_base_url(&format!("{}/admin/v1", server.url()))
        .fetch_usage(&context(SourceMode::Auto))
        .await
}

async fn fetch(body: &str) -> ProviderFetchResult {
    fetch_with(body, 200, ORGANIZATION_FIXTURE, 200)
        .await
        .expect("fixture fetch succeeds")
}

fn sections(result: &ProviderFetchResult) -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    for detail in result.display_details() {
        let title = detail.section_title().unwrap_or_default().to_string();
        if !titles.contains(&title) {
            titles.push(title);
        }
    }
    titles
}

fn rows_in(result: &ProviderFetchResult, section: &str) -> Vec<(String, String)> {
    result
        .display_details()
        .iter()
        .filter(|detail| detail.section_title() == Some(section))
        .map(|detail| (detail.title().to_string(), detail.value().to_string()))
        .collect()
}

fn labels_in(result: &ProviderFetchResult, section: &str) -> Vec<String> {
    rows_in(result, section)
        .into_iter()
        .map(|(label, _)| label)
        .collect()
}

fn primary_percent(result: &ProviderFetchResult) -> Option<f64> {
    let primary = &result.usage.primary;
    (!primary.is_informational).then_some(primary.used_percent)
}

#[test]
fn metadata_and_source_contract_match_the_api_key_provider() {
    let provider = MunitoProvider::new();
    assert_eq!(provider.id(), ProviderId::Munito);
    assert_eq!(provider.metadata().display_name, "Munito");
    assert_eq!(provider.metadata().session_label, "Today");
    assert_eq!(provider.metadata().weekly_label, "This month");
    assert_eq!(
        provider.metadata().dashboard_url,
        Some("https://console.munito.ai")
    );
    assert!(!provider.metadata().default_enabled);
    assert_eq!(
        provider.available_sources(),
        vec![SourceMode::Auto, SourceMode::OAuth]
    );
    assert_eq!(ProviderId::Munito.cookie_domain(), None);
    assert_eq!(
        ProviderId::from_cli_name("munito"),
        Some(ProviderId::Munito)
    );
}

#[tokio::test]
async fn spend_windows_cost_details_and_organization_are_parsed() {
    let result = fetch(LIMITS_FIXTURE).await;

    let primary = &result.usage.primary;
    assert!(!primary.is_informational);
    assert_eq!(primary.used_percent, 8.5);
    assert_eq!(primary.window_minutes, Some(1440));
    assert_eq!(primary.resets_at, None);
    let secondary = result.usage.secondary.as_ref().expect("monthly window");
    assert!((secondary.used_percent - 12.34).abs() < 0.000_001);
    assert_eq!(secondary.window_minutes, None);

    let cost = result.cost.as_ref().expect("cost");
    assert_eq!(cost.used, 123.4);
    assert_eq!(cost.limit, Some(1000.0));
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.period, "This month");

    assert_eq!(sections(&result), vec!["Spend limits", "Organization"]);
    assert_eq!(
        rows_in(&result, "Spend limits"),
        vec![
            ("Today".into(), "$4.25 of $50.00".into()),
            ("Today organization limit".into(), "$500.00".into()),
            ("Today account limit".into(), "$40.00".into()),
            ("This month".into(), "$123.40 of $1,000.00".into()),
            ("This month organization limit".into(), "$5,000.00".into()),
            ("This month account limit".into(), "$900.00".into()),
            ("Usage tier".into(), "fixture-tier".into()),
        ]
    );
    let today = &result.display_details()[0];
    let progress = today.progress().expect("today progress");
    assert!((progress.used() / progress.total() * 100.0 - 8.5).abs() < 0.000_001);
    assert_eq!(
        rows_in(&result, "Organization"),
        vec![
            ("Status".into(), "Active".into()),
            ("Data region".into(), "eu".into()),
        ]
    );

    assert_eq!(result.usage.login_method.as_deref(), Some("Admin API key"));
    assert_eq!(
        result.usage.account_organization.as_deref(),
        Some("Fixture Org")
    );
    assert_eq!(result.account_identity(), Some("org_fixture"));
    assert_eq!(result.usage.account_email, None);
    assert_eq!(result.usage.subscription, None);
    assert_eq!(result.source_label, "api");
}

#[tokio::test]
async fn periods_without_limits_keep_spend_only_rows_and_no_windows() {
    let result = fetch(concat!(
        r#"{"object":"spend_limits","periods":[{"period":"day","spent_usd":2,"limit_usd":null},"#,
        r#"{"period":"month","spent_usd":"12.75","limit_usd":0}]}"#
    ))
    .await;

    assert_eq!(primary_percent(&result), None);
    assert!(result.usage.primary.hidden_on_card);
    assert!(result.usage.secondary.is_none());
    let cost = result.cost.as_ref().expect("cost");
    assert_eq!(cost.used, 12.75);
    assert_eq!(cost.limit, Some(0.0));
    assert_eq!(
        rows_in(&result, "Spend limits"),
        vec![
            ("Today".into(), "$2.00".into()),
            ("This month".into(), "$12.75".into()),
            ("Usage tier".into(), "fixture-tier".into()),
        ]
    );
    assert!(result.display_details()[0].progress().is_none());
}

#[tokio::test]
async fn a_missing_month_period_reports_the_day_cost_as_today() {
    let result = fetch(
        r#"{"object":"spend_limits","periods":[{"period":"day","spent_usd":3.5,"limit_usd":35}]}"#,
    )
    .await;

    assert_eq!(primary_percent(&result), Some(10.0));
    let cost = result.cost.as_ref().expect("cost");
    assert_eq!(cost.used, 3.5);
    assert_eq!(cost.limit, Some(35.0));
    assert_eq!(cost.period, "Today");
}

#[tokio::test]
async fn unknown_periods_are_ignored_and_stopped_spend_keeps_its_date() {
    let result = fetch(concat!(
        r#"{"object":"spend_limits","periods":["#,
        r#"{"period":"quarter","spent_usd":999,"limit_usd":5000},"#,
        r#"{"period":"day","spent_usd":50,"limit_usd":50,"stopped_until":"2030-01-01T00:00:00Z"}]}"#
    ))
    .await;

    assert_eq!(primary_percent(&result), Some(100.0));
    assert_eq!(
        result.cost.as_ref().map(|cost| cost.period.as_str()),
        Some("Today")
    );
    let stopped = rows_in(&result, "Spend limits")
        .into_iter()
        .find(|(label, _)| label == "Today stopped until")
        .expect("stopped row");
    assert_eq!(stopped.1, "2030-01-01T00:00:00.000Z");
}

#[tokio::test]
async fn daily_and_monthly_spending_stops_retain_their_separate_times() {
    let result = fetch(
        r#"{"periods":[
          {"period":"day","spent_usd":50,"limit_usd":50,"stopped_until":"2030-01-02T08:30:00Z"},
          {"period":"month","spent_usd":500,"limit_usd":500,"stopped_until":"2030-02-01T00:00:00Z"}]}"#,
    )
    .await;

    let stops: Vec<(String, String)> = rows_in(&result, "Spend limits")
        .into_iter()
        .filter(|(label, _)| label.ends_with("stopped until"))
        .collect();
    assert_eq!(
        stops,
        vec![
            (
                "Today stopped until".into(),
                "2030-01-02T08:30:00.000Z".into()
            ),
            (
                "This month stopped until".into(),
                "2030-02-01T00:00:00.000Z".into()
            ),
        ]
    );
}

#[tokio::test]
async fn an_unavailable_organization_read_keeps_spend_and_limits() {
    for organization in ["private-response", "null", "[]"] {
        let result = fetch_with(LIMITS_FIXTURE, 200, organization, 200)
            .await
            .unwrap();
        assert_eq!(primary_percent(&result), Some(8.5));
        assert_eq!(result.cost.as_ref().map(|cost| cost.used), Some(123.4));
        assert_eq!(result.usage.account_organization, None);
        assert_eq!(result.usage.login_method.as_deref(), Some("Admin API key"));
        assert_eq!(sections(&result), vec!["Spend limits"]);
    }
    let result = fetch_with(LIMITS_FIXTURE, 200, ORGANIZATION_FIXTURE, 500)
        .await
        .unwrap();
    assert_eq!(primary_percent(&result), Some(8.5));
    assert_eq!(result.account_identity(), None);
}

#[tokio::test]
async fn empty_periods_keep_an_identity_only_card() {
    let result = fetch(r#"{"object":"spend_limits","periods":[]}"#).await;

    assert_eq!(primary_percent(&result), None);
    assert!(result.usage.secondary.is_none());
    assert!(result.cost.is_none());
    assert_eq!(sections(&result), vec!["Spend limits", "Organization"]);
    assert_eq!(labels_in(&result, "Spend limits"), vec!["Usage tier"]);
    assert_eq!(result.usage.login_method.as_deref(), Some("Admin API key"));
    assert_eq!(
        result.usage.account_organization.as_deref(),
        Some("Fixture Org")
    );
}

#[tokio::test]
async fn malformed_bodies_fail_closed() {
    for body in [
        "private-response",
        "null",
        "[]",
        "42",
        r#"{}"#,
        r#"{"object":"spend_limits"}"#,
        r#"{"periods":{}}"#,
        r#"{"periods":"private-response"}"#,
        r#"{"periods":["private-response"]}"#,
        r#"{"periods":[{"period":"day","spent_usd":"private-response","limit_usd":5}]}"#,
        r#"{"periods":[{"period":"day","spent_usd":-1,"limit_usd":5}]}"#,
        r#"{"periods":[{"period":"day","spent_usd":1,"limit_usd":"private-response"}]}"#,
        r#"{"periods":[{"period":"day","spent_usd":1,"limit_usd":5,"stopped_until":"yesterday"}]}"#,
    ] {
        match fetch_with(body, 200, ORGANIZATION_FIXTURE, 200).await {
            Err(ProviderError::Parse(message)) => {
                assert!(
                    message.contains("unrecognized") || message.contains("unusable"),
                    "{body}: {message}"
                );
                assert!(!message.contains("private-response"));
                assert!(!message.contains(KEY));
            }
            other => panic!("expected a parse failure for {body}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn unusable_money_names_the_field() {
    let error = fetch_with(
        r#"{"periods":[{"period":"month","spent_usd":1,"account_limit_usd":-2}]}"#,
        200,
        ORGANIZATION_FIXTURE,
        200,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Parse error: Munito returned an unusable month account limit."
    );
}

#[tokio::test]
async fn http_errors_keep_their_messages_without_echoing_private_bodies() {
    for (code, expected) in [
        (401, "Munito returned HTTP 401."),
        (403, PERMISSION_DENIED),
        (429, "Munito returned HTTP 429."),
        (503, "Munito returned HTTP 503."),
        (400, "Munito returned HTTP 400."),
    ] {
        match fetch_with("private-response", code, ORGANIZATION_FIXTURE, 200).await {
            Err(ProviderError::Other(message)) => {
                assert_eq!(message, expected);
                assert!(!message.contains("private-response"));
                assert!(!message.contains(KEY));
                if code == 403 {
                    assert!(message.contains("Read account data"));
                }
            }
            other => panic!("expected HTTP {code} failure, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn web_and_cli_sources_are_unsupported() {
    let provider = MunitoProvider::with_base_url("http://127.0.0.1:9");
    for mode in [SourceMode::Web, SourceMode::Cli] {
        assert!(matches!(
            provider.fetch_usage(&context(mode)).await,
            Err(ProviderError::UnsupportedSource(source)) if source == mode
        ));
    }
}

#[tokio::test]
async fn oauth_source_uses_the_same_api_key_lane() {
    let server = serve(LIMITS_FIXTURE, 200, ORGANIZATION_FIXTURE, 200).await;
    let result = MunitoProvider::with_base_url(&format!("{}/admin/v1", server.url()))
        .fetch_usage(&context(SourceMode::OAuth))
        .await
        .unwrap();
    assert_eq!(primary_percent(&result), Some(8.5));
}

#[test]
fn credentials_stay_explicit_and_aliased() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    assert_eq!(
        key_from_environment(env(&[("MUNITO_API_KEY", "fixture-key")])).as_deref(),
        Some("fixture-key")
    );
    assert_eq!(
        key_from_environment(env(&[("MUNITO_ADMIN_KEY", "fixture-key")])).as_deref(),
        Some("fixture-key")
    );
    assert_eq!(
        key_from_environment(env(&[("MUNITO_ADMIN_KEY", " 'alias' ")])).as_deref(),
        Some("alias")
    );
    assert_eq!(key_from_environment(env(&[("MUNITO_API_KEY", "  ")])), None);
    assert_eq!(
        key_from_environment(env(&[("OTHER_API_KEY", "fixture-key")])),
        None
    );
    assert_eq!(cleaned_key("  \"quoted\"  ").as_deref(), Some("quoted"));
    assert_eq!(cleaned_key("\"\""), None);
    assert_eq!(cleaned_key("\""), None);
}
