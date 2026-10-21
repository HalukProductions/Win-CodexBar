use super::*;
use chrono::TimeZone;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, duplex};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap()
}

fn short_timeouts() -> AcpTimeouts {
    AcpTimeouts {
        initialize: Duration::from_millis(200),
        billing: Duration::from_millis(200),
    }
}

/// Nested `config` payload as the current `_x.ai/billing` result carries it.
fn credits_result(percent: f64) -> Value {
    json!({
        "config": {
            "creditUsagePercent": percent,
            "currentPeriod": {
                "start": "2026-01-01T00:00:00Z",
                "end": "2026-02-01T00:00:00Z",
            },
        },
    })
}

#[test]
fn current_result_maps_usage_reset_window_and_plan() {
    let mut result = credits_result(37.5);
    result["subscription_tier"] = json!("heavy");

    let bearer = parse_billing_result(&result, now()).unwrap();

    assert_eq!(bearer.billing.used_percent, Some(37.5));
    assert!(bearer.billing.used_percent_is_wire_published);
    assert_eq!(
        bearer.billing.resets_at,
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).single()
    );
    // 2026-01-01 to 2026-02-01 is 31 days.
    assert_eq!(bearer.billing.window_minutes, Some(31 * 24 * 60));
    assert_eq!(bearer.subscription_tier.as_deref(), Some("SuperGrok Heavy"));
    assert_eq!(bearer.prepaid_balance_usd, None);
}

#[test]
fn top_level_tier_overrides_the_config_tier_and_config_tier_is_the_fallback() {
    let config_only = json!({
        "config": { "creditUsagePercent": 5.0, "subscriptionTier": "SuperGrok" },
    });
    assert_eq!(
        parse_billing_result(&config_only, now())
            .unwrap()
            .subscription_tier
            .as_deref(),
        Some("SuperGrok")
    );

    let mut both = config_only.clone();
    both["subscription_tier"] = json!("heavy");
    assert_eq!(
        parse_billing_result(&both, now())
            .unwrap()
            .subscription_tier
            .as_deref(),
        Some("SuperGrok Heavy")
    );
}

#[test]
fn prepaid_balance_alone_is_a_valid_answer_without_a_quota_percent() {
    let result = json!({
        "config": { "prepaidBalance": { "val": 1250 } },
    });

    let bearer = parse_billing_result(&result, now()).unwrap();

    assert_eq!(bearer.billing.used_percent, None);
    assert_eq!(bearer.billing.resets_at, None);
    assert_eq!(bearer.prepaid_balance_usd, Some(12.5));
}

#[test]
fn prepaid_balance_reads_string_cents_and_an_empty_wallet_as_zero() {
    let string_cents = json!({ "config": { "prepaidBalance": { "val": "250" } } });
    assert_eq!(
        parse_billing_result(&string_cents, now())
            .unwrap()
            .prepaid_balance_usd,
        Some(2.5)
    );

    let empty_wallet = json!({ "config": { "prepaidBalance": {} } });
    assert_eq!(
        parse_billing_result(&empty_wallet, now())
            .unwrap()
            .prepaid_balance_usd,
        Some(0.0)
    );
}

#[test]
fn quota_and_prepaid_balance_arrive_together() {
    let mut result = credits_result(10.0);
    result["config"]["prepaidBalance"] = json!({ "val": 500 });

    let bearer = parse_billing_result(&result, now()).unwrap();

    assert_eq!(bearer.billing.used_percent, Some(10.0));
    assert_eq!(bearer.prepaid_balance_usd, Some(5.0));
}

#[test]
fn reset_time_alone_is_not_usage() {
    let result = json!({
        "config": {
            "currentPeriod": { "start": "2026-01-01T00:00:00Z", "end": "2026-02-01T00:00:00Z" },
        },
    });

    assert!(matches!(
        parse_billing_result(&result, now()),
        Err(ProviderError::Parse(_))
    ));
}

#[test]
fn legacy_top_level_shape_still_maps_usage_and_reset() {
    let result = json!({
        "billingCycle": {
            "billingPeriodStart": "2026-01-01T00:00:00Z",
            "billingPeriodEnd": "2026-02-01T00:00:00Z",
        },
        "monthlyLimit": { "val": 100 },
        "usage": { "totalUsed": { "val": 25 } },
        "subscription_tier": "SuperGrok",
    });

    let bearer = parse_billing_result(&result, now()).unwrap();

    assert_eq!(bearer.billing.used_percent, Some(25.0));
    assert!(bearer.billing.used_percent_is_wire_published);
    assert_eq!(
        bearer.billing.resets_at,
        Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).single()
    );
    assert_eq!(bearer.billing.window_minutes, Some(31 * 24 * 60));
    assert_eq!(bearer.subscription_tier.as_deref(), Some("SuperGrok"));
    assert_eq!(bearer.prepaid_balance_usd, None);
}

#[test]
fn legacy_usage_is_clamped_to_the_limit() {
    let result = json!({
        "monthlyLimit": { "val": 100 },
        "usage": { "totalUsed": { "val": 150 } },
    });

    assert_eq!(
        parse_billing_result(&result, now())
            .unwrap()
            .billing
            .used_percent,
        Some(100.0)
    );
}

#[test]
fn legacy_shape_without_a_positive_limit_is_unavailable() {
    for result in [
        json!({ "monthlyLimit": { "val": 0 }, "usage": { "totalUsed": { "val": 5 } } }),
        json!({ "usage": { "totalUsed": { "val": 5 } } }),
        json!({ "monthlyLimit": { "val": 100 } }),
    ] {
        assert!(
            matches!(
                parse_billing_result(&result, now()),
                Err(ProviderError::Parse(_))
            ),
            "{result}"
        );
    }
}

#[test]
fn missing_or_null_fields_are_parse_errors_not_empty_snapshots() {
    for result in [
        json!({}),
        json!({ "config": {} }),
        json!({ "config": null }),
        json!({ "config": { "creditUsagePercent": "lots" } }),
        Value::Null,
    ] {
        assert!(
            matches!(
                parse_billing_result(&result, now()),
                Err(ProviderError::Parse(_))
            ),
            "{result}"
        );
    }
}

#[test]
fn a_percent_without_a_reset_is_still_usage() {
    let result = json!({ "config": { "creditUsagePercent": 10.0 } });

    let bearer = parse_billing_result(&result, now()).unwrap();

    assert_eq!(bearer.billing.used_percent, Some(10.0));
    assert_eq!(bearer.billing.resets_at, None);
    assert_eq!(bearer.billing.window_minutes, None);
}

#[test]
fn reply_for_matches_only_the_requested_id() {
    // Notifications carry no id; replies to other requests carry another id.
    assert!(reply_for(&json!({ "jsonrpc": "2.0", "method": "session/update" }), 2).is_none());
    assert!(reply_for(&json!({ "jsonrpc": "2.0", "id": 1, "result": {} }), 2).is_none());

    let reply = reply_for(
        &json!({ "jsonrpc": "2.0", "id": 2, "result": { "a": 1 } }),
        2,
    )
    .expect("matching reply")
    .expect("result is a success");
    assert_eq!(reply, json!({ "a": 1 }));
}

#[test]
fn reply_for_treats_a_null_error_as_no_error_and_a_bare_id_as_malformed() {
    let reply = reply_for(
        &json!({ "jsonrpc": "2.0", "id": 2, "error": null, "result": {} }),
        2,
    )
    .expect("matching reply")
    .expect("null error is not an error");
    assert_eq!(reply, json!({}));

    assert!(matches!(
        reply_for(&json!({ "jsonrpc": "2.0", "id": 2 }), 2),
        Some(Err(ProviderError::Parse(_)))
    ));
}

#[test]
fn rpc_error_objects_map_to_auth_unsupported_and_other() {
    assert!(matches!(
        rpc_error(&json!({
            "code": -32000,
            "message": "Authentication required to fetch billing data. Run 'grok login' to authenticate.",
        })),
        ProviderError::AuthRequired
    ));
    assert!(matches!(
        rpc_error(&json!({ "code": -32601, "message": "Method not found" })),
        ProviderError::Other(message) if message.contains("does not provide billing")
    ));
    assert!(matches!(
        rpc_error(&json!({ "code": -32000, "message": "quota service down" })),
        ProviderError::Other(message) if message.contains("quota service down")
    ));
    assert!(matches!(
        rpc_error(&json!({ "code": -32000 })),
        ProviderError::Other(message) if message.contains("unknown JSON-RPC error")
    ));
}

/// Two in-memory pipes: the client writes to the server and reads from it.
fn pipes() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
) {
    let (client_writer, server_reader) = duplex(64 * 1024);
    let (server_writer, client_reader) = duplex(64 * 1024);
    (
        client_writer,
        BufReader::new(client_reader),
        server_reader,
        server_writer,
    )
}

#[tokio::test]
async fn handshake_then_billing_skips_noise_and_decodes_the_answer() {
    let (mut client_writer, mut client_reader, server_reader, mut server_writer) = pipes();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let init: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["id"], 1);
        assert_eq!(init["params"]["protocolVersion"], "1");
        assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
        server_writer.write_all(b"not json at all\n").await.unwrap();
        server_writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{}}\n")
            .await
            .unwrap();
        server_writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
            .await
            .unwrap();

        let billing_line = lines.next_line().await.unwrap().unwrap();
        // The extension method must reach the wire literally, without a
        // `\/` escape: grok does not unescape method names.
        assert!(billing_line.contains("\"_x.ai/billing\""), "{billing_line}");
        let billing: Value = serde_json::from_str(&billing_line).unwrap();
        assert_eq!(billing["id"], 2);
        assert_eq!(billing["method"], "_x.ai/billing");
        assert_eq!(billing["params"], json!({}));
        let answer = json!({ "jsonrpc": "2.0", "id": 2, "result": credits_result(42.0) });
        server_writer
            .write_all(format!("{answer}\n").as_bytes())
            .await
            .unwrap();
    });

    let bearer = billing_over_pipes(
        &mut client_reader,
        &mut client_writer,
        short_timeouts(),
        now(),
    )
    .await
    .unwrap();

    assert_eq!(bearer.billing.used_percent, Some(42.0));
    server.await.unwrap();
}

#[tokio::test]
async fn silent_child_times_out_on_initialize() {
    let (mut client_writer, mut client_reader, server_reader, server_writer) = pipes();
    // Keep the server ends open so the silence is a timeout, not end of stream.
    let _held = (server_reader, server_writer);

    let started = Instant::now();
    let error = billing_over_pipes(
        &mut client_reader,
        &mut client_writer,
        short_timeouts(),
        now(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::Timeout), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn silent_child_times_out_on_billing_after_a_good_handshake() {
    let (mut client_writer, mut client_reader, server_reader, mut server_writer) = pipes();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let init = lines.next_line().await.unwrap().unwrap();
        let id = serde_json::from_str::<Value>(&init).unwrap()["id"].clone();
        let reply = json!({ "jsonrpc": "2.0", "id": id, "result": {} });
        server_writer
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
        // Read the billing request, then never answer it.
        let _billing = lines.next_line().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let error = billing_over_pipes(
        &mut client_reader,
        &mut client_writer,
        short_timeouts(),
        now(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::Timeout), "{error}");
    server.abort();
}

#[tokio::test]
async fn closed_output_before_a_reply_is_an_error() {
    let (mut client_writer, mut client_reader, server_reader, server_writer) = pipes();
    // The request can be written, but the reply side closes.
    let _held_reader = server_reader;
    drop(server_writer);

    let error = billing_over_pipes(
        &mut client_reader,
        &mut client_writer,
        short_timeouts(),
        now(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, ProviderError::Other(message) if message.contains("closed")),
        "{error}"
    );
}

#[tokio::test]
async fn an_auth_error_during_the_handshake_is_surfaced_as_auth_required() {
    let (mut client_writer, mut client_reader, server_reader, mut server_writer) = pipes();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_reader).lines();
        let _init = lines.next_line().await.unwrap();
        let reply = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32000, "message": "Authentication required" },
        });
        server_writer
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });

    let error = billing_over_pipes(
        &mut client_reader,
        &mut client_writer,
        short_timeouts(),
        now(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::AuthRequired), "{error}");
    server.await.unwrap();
}

#[tokio::test]
async fn bounded_line_reader_returns_each_line_then_none_at_end_of_stream() {
    let (mut writer, reader) = duplex(64 * 1024);
    let mut reader = BufReader::new(reader);
    writer.write_all(b"{\"a\":1}\nlast").await.unwrap();
    drop(writer);

    assert_eq!(
        read_bounded_line(&mut reader).await.unwrap(),
        Some(b"{\"a\":1}\n".to_vec())
    );
    // A final line without a newline is still delivered once.
    assert_eq!(
        read_bounded_line(&mut reader).await.unwrap(),
        Some(b"last".to_vec())
    );
    assert_eq!(read_bounded_line(&mut reader).await.unwrap(), None);
}

#[tokio::test]
async fn bounded_line_reader_rejects_a_line_over_the_limit() {
    let (mut writer, reader) = duplex(64 * 1024);
    let mut reader = BufReader::new(reader);
    let feed = tokio::spawn(async move {
        let oversized = vec![b'a'; MAX_LINE_BYTES + 1];
        writer.write_all(&oversized).await.ok();
        writer.write_all(b"\n").await.ok();
    });

    let error = read_bounded_line(&mut reader).await.unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    drop(reader);
    feed.await.ok();
}

/// A child that stays alive and never speaks ACP.
#[cfg(windows)]
const SILENT_CHILD: (&str, &[&str]) = (
    "powershell",
    &[
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Start-Sleep -Seconds 120",
    ],
);
#[cfg(not(windows))]
const SILENT_CHILD: (&str, &[&str]) = ("sleep", &["120"]);

#[tokio::test]
async fn timed_out_child_is_stopped_and_reaped() {
    let (program, args) = SILENT_CHILD;
    let mut child = spawn_child(std::path::Path::new(program), args).expect("silent child starts");

    let outcome = run_child(&mut child, short_timeouts(), now()).await;
    // `fetch_billing` runs exactly this sequence.
    stop(&mut child).await;

    assert!(
        matches!(outcome, Err(ProviderError::Timeout)),
        "{outcome:?}"
    );
    assert!(
        child
            .try_wait()
            .expect("child status is readable")
            .is_some(),
        "the child must be reaped, not left running"
    );
}

#[tokio::test]
async fn a_missing_grok_binary_is_reported_as_not_installed() {
    let error = fetch_billing(
        std::path::Path::new("codexbar-test-no-such-grok-binary"),
        &["agent", "stdio"],
        short_timeouts(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::NotInstalled(_)), "{error}");
}
