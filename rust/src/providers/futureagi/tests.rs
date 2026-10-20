//! Fixture tests for the Future AGI provider, ported from upstream
//! `Tests/CodexBarTests/FutureAGIPluginTests.swift`. Fetch tests use an
//! injected key pair and a local mock server, so no test reads the keyring,
//! the environment, or the real Future AGI API.

use super::*;
use mockito::{Matcher, Server, ServerGuard};

const KEY: &str = "fixture-key";
const SECRET: &str = "fixture-secret";
const USAGE_PATH: &str = "/usage/v2/usage-overview/";

/// Shape from the production OpenAPI `UsageOverviewResponse`; values are
/// synthetic.
const FIXTURE: &str = concat!(
    r#"{"status":true,"result":{"plan_display_name":"Free","billing_period_start":"2026-10-01","#,
    r#""billing_period_end":"2026-11-01","total_estimated_cost":1.25,"dimensions":["#,
    r#"{"key":"ai_credits","display_name":"AI credits","display_unit":"credits","current_usage":500,"free_allowance":2000},"#,
    r#"{"key":"gateway_requests","display_name":"Gateway requests","display_unit":"requests","#,
    r#""current_usage":150000,"free_allowance":100000},"#,
    r#"{"key":"storage","display_name":"Storage","display_unit":"GB","current_usage":0.5,"free_allowance":1}]}}"#
);

fn context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        source_mode,
        api_key: Some(format!("{KEY}:{SECRET}")),
        ..FetchContext::default()
    }
}

async fn serve(body: &str, code: usize) -> ServerGuard {
    let mut server = Server::new_async().await;
    server
        .mock("GET", USAGE_PATH)
        .match_header("x-api-key", KEY)
        .match_header("x-secret-key", SECRET)
        .match_header("authorization", Matcher::Missing)
        .match_header("cookie", Matcher::Missing)
        .with_status(code)
        .with_header("content-type", "application/json")
        .with_header("retry-after", "0")
        .with_body(body)
        .create_async()
        .await;
    server
}

fn provider_for(server: &ServerGuard) -> FutureAgiProvider {
    FutureAgiProvider::with_usage_url(&format!("{}{USAGE_PATH}", server.url()))
}

async fn fetch_with(body: &str, code: usize) -> Result<ProviderFetchResult, ProviderError> {
    let server = serve(body, code).await;
    provider_for(&server)
        .fetch_usage(&context(SourceMode::Auto))
        .await
}

async fn fetch(body: &str) -> ProviderFetchResult {
    fetch_with(body, 200).await.expect("fixture fetch succeeds")
}

fn rows_in(result: &ProviderFetchResult, section: &str) -> Vec<(String, String)> {
    result
        .display_details()
        .iter()
        .filter(|detail| detail.section_title() == Some(section))
        .map(|detail| (detail.title().to_string(), detail.value().to_string()))
        .collect()
}

fn primary_percent(result: &ProviderFetchResult) -> Option<f64> {
    let primary = &result.usage.primary;
    (!primary.is_informational).then_some(primary.used_percent)
}

#[test]
fn metadata_and_source_contract_match_the_api_key_provider() {
    let provider = FutureAgiProvider::new();
    assert_eq!(provider.id(), ProviderId::FutureAgi);
    assert_eq!(provider.metadata().display_name, "Future AGI");
    assert_eq!(provider.metadata().session_label, "AI credits");
    assert_eq!(provider.metadata().weekly_label, "Gateway requests");
    assert_eq!(
        provider.metadata().dashboard_url,
        Some("https://app.futureagi.com/dashboard/settings/usage-summary")
    );
    assert!(!provider.metadata().default_enabled);
    assert_eq!(
        provider.available_sources(),
        vec![SourceMode::Auto, SourceMode::OAuth]
    );
    assert_eq!(ProviderId::FutureAgi.cookie_domain(), None);
    assert_eq!(
        ProviderId::from_cli_name("futureagi"),
        Some(ProviderId::FutureAgi)
    );
}

#[tokio::test]
async fn documented_usage_preserves_allowances_overages_and_billing_dates() {
    let result = fetch(FIXTURE).await;

    let primary = &result.usage.primary;
    assert!(!primary.is_informational);
    assert_eq!(primary.used_percent, 25.0);
    assert_eq!(primary.window_minutes, Some(44_640));
    assert_eq!(
        primary.resets_at,
        DateTime::<Utc>::from_timestamp(1_793_491_200, 0)
    );
    let secondary = result.usage.secondary.as_ref().expect("gateway window");
    assert_eq!(secondary.used_percent, 100.0);
    assert_eq!(secondary.window_minutes, Some(44_640));

    assert_eq!(
        rows_in(&result, USAGE_SECTION),
        vec![
            ("AI credits".into(), "500 / 2,000 credits".into()),
            (
                "Gateway requests".into(),
                "150,000 / 100,000 requests".into()
            ),
            ("Storage".into(), "0.5 / 1 GB".into()),
        ]
    );
    assert_eq!(
        rows_in(&result, BILLING_SECTION),
        vec![("Estimated usage cost".into(), "$1.25".into())]
    );
    assert_eq!(result.usage.login_method.as_deref(), Some("Free"));
    assert!(result.cost.is_none());
    assert_eq!(result.usage.account_email, None);
    assert_eq!(result.source_label, "api");
}

#[tokio::test]
async fn zero_allowances_omit_bars_while_retaining_measured_usage() {
    let result = fetch(&FIXTURE.replace("\"free_allowance\":2000", "\"free_allowance\":0")).await;

    assert_eq!(primary_percent(&result), None);
    assert!(result.usage.primary.hidden_on_card);
    assert!(result.usage.secondary.is_some());
    assert_eq!(
        rows_in(&result, USAGE_SECTION)[0].1,
        "500 / 0 credits".to_string()
    );
}

#[tokio::test]
async fn offset_timestamps_are_accepted_for_the_billing_period() {
    let result = fetch(
        &FIXTURE
            .replace("\"2026-10-01\"", "\"2026-10-01T00:00:00+02:00\"")
            .replace("\"2026-11-01\"", "\"2026-11-01T00:00:00.000Z\""),
    )
    .await;
    assert_eq!(result.usage.primary.window_minutes, Some(44_760));
    assert_eq!(
        result.usage.primary.resets_at,
        DateTime::<Utc>::from_timestamp(1_793_491_200, 0)
    );
}

#[tokio::test]
async fn malformed_usage_never_becomes_zero_usage() {
    let duplicate_key = FIXTURE.replace("\"key\":\"storage\"", "\"key\":\"ai_credits\"");
    let too_many = format!(
        r#"{{"status":true,"result":{{"billing_period_start":"2026-10-01","billing_period_end":"2026-11-01","total_estimated_cost":0,"dimensions":[{}]}}}}"#,
        (0..17)
            .map(|index| format!(
                r#"{{"key":"m{index}","display_name":"M","display_unit":"u","current_usage":0,"free_allowance":1}}"#
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    for body in [
        r#"{"status":false,"result":"private-response"}"#.to_string(),
        "private-response".to_string(),
        "null".to_string(),
        FIXTURE.replace("\"current_usage\":500", "\"current_usage\":\"500\""),
        FIXTURE.replace("\"current_usage\":500", "\"current_usage\":-1"),
        FIXTURE.replace("2026-11-01", "invalid-date"),
        FIXTURE.replace("2026-11-01", "2026-09-01"),
        FIXTURE.replace("2026-11-01", "2026-13-01"),
        FIXTURE.replace("\"total_estimated_cost\":1.25", "\"total_estimated_cost\":null"),
        FIXTURE.replace("\"display_unit\":\"GB\"", "\"display_unit\":\"  \""),
        FIXTURE.replace("\"display_name\":\"Storage\"", &format!("\"display_name\":\"{}\"", "x".repeat(121))),
        duplicate_key,
        too_many,
        r#"{"status":true,"result":{"billing_period_start":"2026-10-01","billing_period_end":"2026-11-01","total_estimated_cost":0,"dimensions":[]}}"#.to_string(),
    ] {
        match fetch_with(&body, 200).await {
            Err(ProviderError::Parse(message)) => {
                assert_eq!(message, UNRECOGNIZED, "{body}");
                assert!(!message.contains("private-response"));
            }
            other => panic!("expected a parse failure for {body}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn http_failures_keep_their_status_without_echoing_response_bodies() {
    for code in [401, 403] {
        match fetch_with("private-response", code).await {
            Err(ProviderError::AuthRequired) => {}
            other => panic!("expected an auth failure for HTTP {code}, got {other:?}"),
        }
    }
    match fetch_with("private-response", 429).await {
        Err(ProviderError::Other(message)) => {
            assert!(message.contains("rate limited"), "{message}");
            assert!(message.contains("429"), "{message}");
            assert!(!message.contains("private-response"));
            assert!(!message.contains(KEY));
            assert!(!message.contains(SECRET));
        }
        other => panic!("expected a rate-limit failure, got {other:?}"),
    }
    for code in [503, 400] {
        match fetch_with("private-response", code).await {
            Err(ProviderError::Other(message)) => {
                assert_eq!(message, format!("Future AGI returned HTTP {code}."));
                assert!(!message.contains("private-response"));
                assert!(!message.contains(KEY));
                assert!(!message.contains(SECRET));
            }
            other => panic!("expected HTTP {code} failure, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn web_and_cli_sources_are_unsupported() {
    let provider = FutureAgiProvider::with_usage_url("http://127.0.0.1:9");
    for mode in [SourceMode::Web, SourceMode::Cli] {
        assert!(matches!(
            provider.fetch_usage(&context(mode)).await,
            Err(ProviderError::UnsupportedSource(source)) if source == mode
        ));
    }
}

#[tokio::test]
async fn oauth_source_uses_the_same_api_key_lane() {
    let server = serve(FIXTURE, 200).await;
    let result = provider_for(&server)
        .fetch_usage(&context(SourceMode::OAuth))
        .await
        .unwrap();
    assert_eq!(primary_percent(&result), Some(25.0));
}

#[test]
fn resolve_credentials_cleans_each_half_not_the_pair() {
    // Cleaning the whole Settings value before the colon split would strip
    // the outer quote pair and corrupt both halves; each half is cleaned on
    // its own after splitting.
    assert_eq!(
        resolve_credentials(Some("\"fixture-key\":\"fixture-secret\"")).ok(),
        pair("fixture-key", "fixture-secret")
    );
    assert_eq!(
        resolve_credentials(Some(" 'fixture-key' : 'fixture-secret' ")).ok(),
        pair("fixture-key", "fixture-secret")
    );
}

#[tokio::test]
async fn quoted_settings_pairs_send_clean_halves() {
    for raw in [
        format!("\"{KEY}\":\"{SECRET}\""),
        format!(" '{KEY}' : '{SECRET}' "),
    ] {
        // The mock matches only the exact clean headers, so a successful
        // fetch proves the halves were split before quote stripping.
        let server = serve(FIXTURE, 200).await;
        let result = provider_for(&server)
            .fetch_usage(&FetchContext {
                source_mode: SourceMode::Auto,
                api_key: Some(raw.clone()),
                ..FetchContext::default()
            })
            .await;
        assert_eq!(
            primary_percent(&result.unwrap_or_else(|error| panic!("{raw}: {error}"))),
            Some(25.0),
            "{raw}"
        );
    }
}

fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

fn pair(api_key: &str, secret_key: &str) -> Option<Credentials> {
    Some(Credentials {
        api_key: api_key.into(),
        secret_key: secret_key.into(),
    })
}

#[test]
fn incomplete_credential_pairs_are_rejected() {
    for pairs in [
        &[][..],
        &[(API_KEY_ENV, "fixture-key")][..],
        &[(SECRET_KEY_ENV, "fixture-secret")][..],
        &[(API_KEY_ENV, "fixture-key"), (SECRET_KEY_ENV, "  ")][..],
    ] {
        assert_eq!(credentials_from(None, env(pairs)), None, "{pairs:?}");
    }
    assert_eq!(credentials_from(Some("fixture-key"), env(&[])), None);
    assert_eq!(credentials_from(Some("fixture-key:"), env(&[])), None);
    assert_eq!(credentials_from(Some(":fixture-secret"), env(&[])), None);
}

#[test]
fn stored_halves_override_their_environment_variables() {
    let both = env(&[(API_KEY_ENV, "env-key"), (SECRET_KEY_ENV, "env-secret")]);
    assert_eq!(credentials_from(None, &both), pair("env-key", "env-secret"));
    assert_eq!(
        credentials_from(Some(" 'fixture-key' : \"fixture-secret\" "), &both),
        pair("fixture-key", "fixture-secret")
    );
    assert_eq!(
        credentials_from(Some("fixture-key"), &both),
        pair("fixture-key", "env-secret")
    );
    assert_eq!(
        credentials_from(Some(":fixture-secret"), &both),
        pair("env-key", "fixture-secret")
    );
    assert_eq!(
        credentials_from(Some("fixture-key:secret:with:colons"), env(&[])),
        pair("fixture-key", "secret:with:colons")
    );
}
