//! Scripted stand-in for `grok agent stdio`, used by the ACP billing tests.
//!
//! Usage: `fake_grok_acp agent stdio <scenario> [pid-file]`
//!
//! It speaks newline-delimited JSON-RPC 2.0 on stdin and stdout, as the real CLI
//! does. When a pid file is given, it first writes `"<pid> <console>"`, where
//! `<console>` is `1` when the process has a console window and `0` otherwise.
//! The tests use that to check that CodexBar starts the child hidden.
//!
//! This is a cargo example, not a shipped binary. `cargo test` builds it next to
//! the test binary, so no dependency or PATH setup is needed.

use std::fs;
use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

const BILLING_METHOD: &str = "_x.ai/billing";
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// Larger than the client's 256 KiB line limit.
const OVERSIZED_LINE_BYTES: usize = 300 * 1024;

#[cfg(windows)]
unsafe extern "system" {
    fn GetConsoleWindow() -> *mut std::ffi::c_void;
}

fn has_console() -> bool {
    #[cfg(windows)]
    {
        // SAFETY: GetConsoleWindow takes no arguments and only reads state.
        !unsafe { GetConsoleWindow() }.is_null()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args[1] != "agent" || args[2] != "stdio" {
        eprintln!("usage: fake_grok_acp agent stdio <scenario> [pid-file]");
        std::process::exit(2);
    }
    let scenario = args[3].as_str();
    if let Some(path) = args.get(4) {
        let record = format!("{} {}", std::process::id(), u8::from(has_console()));
        if fs::write(path, record).is_err() {
            std::process::exit(3);
        }
    }
    serve(scenario, io::stdin().lock().lines());
}

/// Writes one JSON-RPC message per line. A failed write means the client has
/// gone away, so the server exits.
fn send(value: &Value) {
    send_raw(&format!("{value}\n"));
}

fn send_raw(text: &str) {
    let mut out = io::stdout().lock();
    if out
        .write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        std::process::exit(0);
    }
}

/// Blocks until the client kills this process.
fn hang() -> ! {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn next_request(lines: &mut impl Iterator<Item = io::Result<String>>) -> Option<Value> {
    loop {
        let line = lines.next()?.ok()?;
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line).ok();
    }
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn serve(scenario: &str, mut lines: impl Iterator<Item = io::Result<String>>) {
    let Some(init) = next_request(&mut lines) else {
        return;
    };
    match scenario {
        // The CLI reads the handshake and then exits without answering.
        "exit_early" => return,
        "hang_init" => hang(),
        _ => {}
    }
    let init_id = init.get("id").cloned().unwrap_or(Value::Null);
    if init.get("method").and_then(Value::as_str) != Some("initialize")
        || init
            .pointer("/params/protocolVersion")
            .and_then(Value::as_str)
            != Some("1")
    {
        send(&error_reply(&init_id, INVALID_PARAMS, "bad initialize"));
        return;
    }
    if scenario == "auth_init" {
        send(&error_reply(&init_id, -32000, "Authentication required"));
        return;
    }
    noise(scenario, &init_id);
    send(&json!({
        "jsonrpc": "2.0",
        "id": init_id,
        "result": {
            "protocolVersion": "1",
            "agentCapabilities": { "loadSession": false },
        },
    }));

    let Some(billing) = next_request(&mut lines) else {
        return;
    };
    let billing_id = billing.get("id").cloned().unwrap_or(Value::Null);
    match scenario {
        "hang" => hang(),
        "partial_eof" => {
            // Half a frame, then the process ends.
            send_raw("{\"jsonrpc\":\"2.0\",\"id\":2,\"res");
            return;
        }
        "oversized" => {
            send_raw(&"a".repeat(OVERSIZED_LINE_BYTES));
            send_raw("\n");
            hang();
        }
        "method_not_found" => {
            send(&error_reply(
                &billing_id,
                METHOD_NOT_FOUND,
                "Method not found",
            ));
            return;
        }
        "auth_billing" => {
            send(&error_reply(
                &billing_id,
                -32000,
                "Authentication required. Run `grok login`.",
            ));
            return;
        }
        _ => {}
    }
    if billing.get("method").and_then(Value::as_str) != Some(BILLING_METHOD) {
        send(&error_reply(
            &billing_id,
            METHOD_NOT_FOUND,
            "unexpected method",
        ));
        return;
    }
    noise(scenario, &billing_id);
    let result = if scenario == "legacy" {
        legacy_billing_result()
    } else {
        credits_billing_result()
    };
    send(&json!({ "jsonrpc": "2.0", "id": billing_id, "result": result }));
}

/// Traffic that is not the reply to the pending request: a truncated frame,
/// non-JSON text, a notification, a reply to another id, and a server request
/// that reuses the pending id. Only the first two are sent by `garbled`.
fn noise(scenario: &str, pending_id: &Value) {
    match scenario {
        "garbled" => {
            send_raw("{\"jsonrpc\":\"2.0\",\"id\":\n");
            send_raw("not json at all\n");
        }
        "noise" => {
            send_raw("not json at all\n");
            send(&json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "s1", "update": { "sessionUpdate": "agent_message_chunk" } },
            }));
            send(&json!({ "jsonrpc": "2.0", "id": 999, "result": {} }));
            send(&json!({
                "jsonrpc": "2.0",
                "id": pending_id,
                "method": "fs/read_text_file",
                "params": { "path": "/nonexistent" },
            }));
        }
        _ => {}
    }
}

/// Current `_x.ai/billing` result: the nested `config` credits payload with the
/// plan at the top level, as upstream reads it.
fn credits_billing_result() -> Value {
    json!({
        "config": {
            "creditUsagePercent": 42,
            "currentPeriod": {
                "start": "2026-01-01T00:00:00Z",
                "end": "2099-02-01T00:00:00Z",
            },
        },
        "subscription_tier": "SUPERGROK_HEAVY",
    })
}

/// Older top-level result: monetary values are `{ "val": <cents> }`.
fn legacy_billing_result() -> Value {
    json!({
        "billingCycle": {
            "billingPeriodStart": "2026-01-01T00:00:00Z",
            "billingPeriodEnd": "2099-02-01T00:00:00Z",
        },
        "monthlyLimit": { "val": 10000 },
        "usage": { "totalUsed": { "val": 2500 } },
        "subscription_tier": "SUPERGROK",
    })
}
