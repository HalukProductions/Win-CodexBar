use super::*;

use crate::core::Provider;
use chrono::TimeZone;
use reqwest::Client;
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

const TOKEN: &str = "fixture-token";
const TEAM: &str = "team_fixture";

/// Fixed clock: Wednesday 2026-10-14 12:00 UTC.
fn wednesday() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
}

fn user_body(id: &str) -> String {
    json!({"user": {"id": id, "email": "fixture@example.com"}}).to_string()
}

fn budget_row(scope_id: &str, spend: f64, limit: f64, period: &str) -> Value {
    json!({
        "scopeType": "user",
        "scopeId": scope_id,
        "active": true,
        "archived": false,
        "refreshPeriod": period,
        "limitAmount": limit,
        "currentSpend": spend,
    })
}

fn page(rows: Vec<Value>, next: Option<&str>) -> String {
    json!({"budgets": rows, "nextCursor": next}).to_string()
}

/// Serves one scripted response per accepted connection and returns the
/// lower-cased request heads it saw, in order.
fn serve_sequence(responses: Vec<(&'static str, String)>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local test server");
    let address = listener.local_addr().expect("local server address");
    let handle = std::thread::spawn(move || {
        let mut heads = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("read request");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            heads.push(String::from_utf8_lossy(&buffer).to_ascii_lowercase());
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .expect("write response");
        }
        heads
    });
    (format!("http://{address}"), handle)
}

fn provider(base: &str) -> VercelProvider {
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("test HTTP client");
    VercelProvider::with_client("http://127.0.0.1:1/v1/credits", client)
        .with_api_base_url(base)
        .with_access_token_lookup(|| Some(TOKEN.to_owned()))
}

fn team_context() -> FetchContext {
    FetchContext {
        workspace_id: Some(TEAM.into()),
        ..FetchContext::default()
    }
}

async fn budget(
    responses: Vec<(&'static str, String)>,
) -> (Result<ProviderFetchResult, ProviderError>, Vec<String>) {
    let (base, server) = serve_sequence(responses);
    let result = provider(&base)
        .fetch_personal_budget(&team_context(), TOKEN, wednesday())
        .await;
    let heads = server.join().expect("test server thread");
    (result, heads)
}

async fn budget_ok(responses: Vec<(&'static str, String)>) -> (ProviderFetchResult, Vec<String>) {
    let (result, heads) = budget(responses).await;
    (result.expect("budget fetch"), heads)
}

fn assert_parse_failure(result: Result<ProviderFetchResult, ProviderError>) {
    match result {
        Err(ProviderError::Parse(message)) => {
            assert_eq!(message, UNRECOGNIZED);
        }
        other => panic!("expected parse failure, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn pick_access_token_prefers_stored_then_env_and_ignores_blanks() {
    assert_eq!(
        pick_access_token(Some("  stored-token  "), Some("env-token")).as_deref(),
        Some("stored-token")
    );
    assert_eq!(
        pick_access_token(Some("   "), Some(" env-token ")).as_deref(),
        Some("env-token")
    );
    assert_eq!(pick_access_token(None, Some("")), None);
    assert_eq!(pick_access_token(None, None), None);
}

#[test]
fn resolve_team_prefers_setting_then_env_and_validates_the_shape() {
    assert_eq!(
        resolve_team(Some(" team_fixture "), Some("team_env")).unwrap(),
        "team_fixture"
    );
    assert_eq!(
        resolve_team(Some("  "), Some("team_env")).unwrap(),
        "team_env"
    );
    assert_eq!(resolve_team(Some("my-team_1"), None).unwrap(), "my-team_1");

    let missing = resolve_team(None, None).expect_err("team required");
    assert!(matches!(missing, ProviderError::Other(ref hint) if hint.contains("Team ID")));

    for invalid in ["bad team", "../etc", "team?x=1", &"a".repeat(129)] {
        let error = resolve_team(Some(invalid), None).expect_err(invalid);
        assert!(
            error.to_string().contains("Invalid Vercel Team ID"),
            "{invalid}: {error}"
        );
    }
}

#[test]
fn native_user_id_budget_is_matched_and_rendered() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (result, heads) = budget_ok(vec![
            ("200 OK", user_body("fixtureUser")),
            (
                "200 OK",
                page(
                    vec![
                        budget_row("someoneElse", 999.0, 1000.0, "monthly"),
                        budget_row("fixtureUser", 30.0, 120.0, "monthly"),
                    ],
                    None,
                ),
            ),
        ])
        .await;

        assert_eq!(heads.len(), 2);
        assert!(heads[0].starts_with("get /v2/user "), "{}", heads[0]);
        assert!(
            heads[1].starts_with(
                "get /ai-gateway/budgets/list?teamid=team_fixture&scopetype=user&limit=200 "
            ),
            "{}",
            heads[1]
        );
        assert!(
            heads
                .iter()
                .all(|head| head.contains("authorization: bearer fixture-token"))
        );

        let primary = &result.usage.primary;
        assert!((primary.used_percent - 25.0).abs() < 1e-9);
        assert_eq!(
            primary.reset_description.as_deref(),
            Some("$30.00 / $120.00")
        );
        assert!(primary.description_is_detail);
        assert_eq!(primary.window_minutes, Some(31 * 24 * 60));
        assert_eq!(
            primary.resets_at,
            Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            result.usage.primary_label.as_deref(),
            Some("Personal budget")
        );
        let rows: Vec<(&str, &str)> = result
            .display_details()
            .iter()
            .map(|detail| (detail.title(), detail.value()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Spend", "$30.00"),
                ("Limit", "$120.00"),
                ("Remaining", "$90.00")
            ]
        );
        assert_eq!(result.usage.login_method.as_deref(), Some("Access token"));
        assert_eq!(result.usage.account_organization.as_deref(), Some(TEAM));
        assert_eq!(
            result.usage.account_email.as_deref(),
            Some("fixture@example.com")
        );
        assert_eq!(result.source_label, "api");
    });
}

#[tokio::test]
async fn prefixed_user_id_budget_is_matched() {
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![budget_row("usr_fixtureUser", 30.0, 120.0, "monthly")],
                None,
            ),
        ),
    ])
    .await;
    assert!((result.usage.primary.used_percent - 25.0).abs() < 1e-9);
    assert_eq!(
        result.usage.primary.reset_description.as_deref(),
        Some("$30.00 / $120.00")
    );
}

#[tokio::test]
async fn no_matching_budget_is_the_unconfigured_state_not_zero_spend() {
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![budget_row("usr_otherUser", 5.0, 10.0, "monthly")],
                None,
            ),
        ),
    ])
    .await;
    assert!(result.usage.primary.is_informational);
    assert!(!result.usage.primary.usage_known);
    let details = result.display_details();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].title(), "No personal budget configured");
    assert_eq!(details[0].value(), "—");
}

#[tokio::test]
async fn custom_budget_takes_precedence_over_an_inherited_default() {
    let mut default_row = budget_row("fixtureUser", 50.0, 60.0, "monthly");
    default_row["source"] = json!("default");
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![
                    default_row,
                    budget_row("fixtureUser", 30.0, 120.0, "monthly"),
                ],
                None,
            ),
        ),
    ])
    .await;
    assert!((result.usage.primary.used_percent - 25.0).abs() < 1e-9);
}

#[tokio::test]
async fn an_inherited_default_applies_when_no_custom_budget_exists() {
    let mut default_row = budget_row("fixtureUser", 15.0, 60.0, "monthly");
    default_row["source"] = json!("default");
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        ("200 OK", page(vec![default_row], None)),
    ])
    .await;
    assert!((result.usage.primary.used_percent - 25.0).abs() < 1e-9);
    assert_eq!(
        result.usage.primary.reset_description.as_deref(),
        Some("$15.00 / $60.00")
    );
}

#[tokio::test]
async fn two_custom_budgets_for_one_user_are_ambiguous() {
    let (result, _) = budget(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![
                    budget_row("fixtureUser", 1.0, 10.0, "monthly"),
                    budget_row("usr_fixtureUser", 2.0, 20.0, "daily"),
                ],
                None,
            ),
        ),
    ])
    .await;
    assert_parse_failure(result);
}

#[tokio::test]
async fn pagination_follows_the_cursor_and_stops_when_it_is_null() {
    let (result, heads) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![budget_row("someoneElse", 1.0, 2.0, "monthly")],
                Some("cursor-2"),
            ),
        ),
        (
            "200 OK",
            page(
                vec![budget_row("fixtureUser", 30.0, 120.0, "monthly")],
                None,
            ),
        ),
    ])
    .await;
    assert_eq!(heads.len(), 3);
    assert!(heads[2].contains("cursor=cursor-2"), "{}", heads[2]);
    assert!((result.usage.primary.used_percent - 25.0).abs() < 1e-9);
}

#[tokio::test]
async fn a_repeated_cursor_fails_the_refresh() {
    let (result, _) = budget(vec![
        ("200 OK", user_body("fixtureUser")),
        ("200 OK", page(vec![], Some("same"))),
        ("200 OK", page(vec![], Some("same"))),
    ])
    .await;
    assert_parse_failure(result);
}

#[tokio::test]
async fn pagination_is_capped_at_twenty_pages() {
    let mut responses = vec![("200 OK", user_body("fixtureUser"))];
    for index in 0..MAX_BUDGET_PAGES {
        let next = format!("cursor-{index}");
        responses.push(("200 OK", page(vec![], Some(&next))));
    }
    let (result, heads) = budget(responses).await;
    assert_parse_failure(result);
    assert_eq!(heads.len(), 1 + MAX_BUDGET_PAGES);
}

#[tokio::test]
async fn a_matching_row_with_a_non_boolean_active_flag_fails_the_refresh() {
    let mut row = budget_row("fixtureUser", 30.0, 120.0, "monthly");
    row["active"] = json!("yes");
    let (result, _) = budget(vec![
        ("200 OK", user_body("fixtureUser")),
        ("200 OK", page(vec![row], None)),
    ])
    .await;
    assert_parse_failure(result);
}

#[tokio::test]
async fn rows_for_other_scopes_are_ignored_even_when_malformed() {
    let other = json!({"scopeType": "team", "scopeId": "fixtureUser", "active": "yes"});
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        ("200 OK", page(vec![other], None)),
    ])
    .await;
    assert_eq!(
        result.display_details()[0].title(),
        "No personal budget configured"
    );
}

#[tokio::test]
async fn http_failures_map_to_fixed_messages_without_the_body() {
    for (status_line, needle) in [
        ("401 Unauthorized", "rejected the access token"),
        ("403 Forbidden", "non-Contributor role"),
        ("429 Too Many Requests", "rate limit"),
        ("503 Service Unavailable", "unavailable"),
        ("400 Bad Request", "HTTP 400"),
    ] {
        let (result, _) = budget(vec![(status_line, "private-response".into())]).await;
        let error = result.expect_err(status_line);
        let message = error.to_string();
        assert!(message.contains(needle), "{status_line}: {message}");
        assert!(!message.contains("private-response"), "{message}");
    }
}

#[tokio::test]
async fn a_malformed_user_response_is_a_parse_failure() {
    let (result, _) = budget(vec![("200 OK", json!({"user": {"id": ""}}).to_string())]).await;
    assert_parse_failure(result);
}

#[test]
fn reset_boundaries_are_documented_utc_periods() {
    let now = wednesday();
    let day = period_bounds("daily", now).unwrap().unwrap();
    assert_eq!(day.0, Utc.with_ymd_and_hms(2026, 10, 14, 0, 0, 0).unwrap());
    assert_eq!(day.1, Utc.with_ymd_and_hms(2026, 10, 15, 0, 0, 0).unwrap());

    // Weekly budgets reset on Monday; a Sunday must map back to the Monday six days earlier.
    let sunday = Utc.with_ymd_and_hms(2026, 10, 18, 9, 0, 0).unwrap();
    let week = period_bounds("weekly", sunday).unwrap().unwrap();
    assert_eq!(week.0, Utc.with_ymd_and_hms(2026, 10, 12, 0, 0, 0).unwrap());
    assert_eq!(week.1, Utc.with_ymd_and_hms(2026, 10, 19, 0, 0, 0).unwrap());

    let month = period_bounds("monthly", now).unwrap().unwrap();
    assert_eq!(month.0, Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap());
    assert_eq!(month.1, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());

    assert_eq!(period_bounds("none", now).unwrap(), None);
    assert!(matches!(
        period_bounds("hourly", now),
        Err(ProviderError::Parse(_))
    ));
}

#[tokio::test]
async fn cumulative_budget_has_no_reset_or_window() {
    let mut row = budget_row("fixtureUser", 30.0, 120.0, "none");
    row["refreshPeriod"] = json!("none");
    let (result, _) = budget_ok(vec![
        ("200 OK", user_body("fixtureUser")),
        ("200 OK", page(vec![row], None)),
    ])
    .await;
    assert_eq!(result.usage.primary.resets_at, None);
    assert_eq!(result.usage.primary.window_minutes, None);
    assert!((result.usage.primary.used_percent - 25.0).abs() < 1e-9);
}

#[tokio::test]
async fn fetch_usage_with_a_token_routes_to_the_budget_and_never_sends_the_gateway_key() {
    let (base, server) = serve_sequence(vec![
        ("200 OK", user_body("fixtureUser")),
        (
            "200 OK",
            page(
                vec![budget_row("fixtureUser", 30.0, 120.0, "monthly")],
                None,
            ),
        ),
    ]);
    let context = FetchContext {
        api_key: Some("gateway-key".into()),
        workspace_id: Some(TEAM.into()),
        ..FetchContext::default()
    };
    let result = provider(&base)
        .fetch_usage(&context)
        .await
        .expect("budget via fetch_usage");
    let heads = server.join().expect("test server thread");
    assert!(
        heads.iter().all(|head| !head.contains("gateway-key")),
        "{heads:?}"
    );
    assert!(result.usage.primary.reset_description.is_some());
}

#[tokio::test]
async fn without_a_token_fetch_usage_keeps_the_team_credits_endpoint() {
    let (base, server) = serve_sequence(vec![(
        "200 OK",
        r#"{"balance":"4.50","total_used":"1.25"}"#.into(),
    )]);
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("test HTTP client");
    let unconfigured = VercelProvider::with_client(format!("{base}/v1/credits"), client)
        .with_api_base_url(&base)
        .with_access_token_lookup(|| None);
    let context = FetchContext {
        api_key: Some("gateway-key".into()),
        workspace_id: Some(TEAM.into()),
        ..FetchContext::default()
    };
    let result = unconfigured
        .fetch_usage(&context)
        .await
        .expect("credits fetch");
    let heads = server.join().expect("test server thread");
    assert!(heads[0].starts_with("get /v1/credits "), "{}", heads[0]);
    assert!(result.usage.primary.is_informational);
    assert_eq!(result.display_details()[0].title(), "Available balance");
}
