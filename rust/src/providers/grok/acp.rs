//! Grok CLI billing over the ACP extension method `_x.ai/billing` (upstream #4595).
//!
//! `grok agent stdio` speaks newline-delimited JSON-RPC 2.0 on stdin and stdout.
//! CodexBar sends `initialize`, then `_x.ai/billing` with empty params. The CLI
//! answers from its own login, so no token from CodexBar is sent.
//!
//! This is a fallback only. `mod.rs` calls it after the bearer billing path has
//! failed, or when the ambient Grok CLI login is missing or expired, and only
//! for the ambient Grok CLI login. Isolated token accounts and saved account
//! rows never reach it.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::core::ProviderError;

use super::billing::GrokBillingSnapshot;
use super::credits_proxy::{
    BearerBilling, parse_credits_response, parse_timestamp, window_minutes,
};
use super::grok_plan_display_name;

const BILLING_METHOD: &str = "_x.ai/billing";
const INITIALIZE_ID: u64 = 1;
const BILLING_ID: u64 = 2;
const METHOD_NOT_FOUND: i64 = -32601;
/// Upper bound on one stdout line. Billing answers are small; anything larger
/// is not a reply CodexBar should buffer.
const MAX_LINE_BYTES: usize = 256 * 1024;
/// How long a killed child gets to be reaped before the caller moves on.
const EXIT_GRACE: Duration = Duration::from_secs(1);
const USAGE_UNAVAILABLE: &str = "Grok CLI billing returned no usage, reset or prepaid balance";
/// Shown when the CLI cannot be started because it is not installed.
pub(super) const CLI_NOT_FOUND: &str = "Grok CLI not found. Install it, then run `grok login`.";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Per-request budgets, matching upstream `GrokRPCClient` (4s for initialize,
/// 3s for billing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AcpTimeouts {
    pub(super) initialize: Duration,
    pub(super) billing: Duration,
}

impl Default for AcpTimeouts {
    fn default() -> Self {
        Self {
            initialize: Duration::from_secs(4),
            billing: Duration::from_secs(3),
        }
    }
}

/// Runs `program args...`, asks it for billing over ACP, and returns the answer.
///
/// The child has no console window, its stderr is discarded, and it is killed
/// before this returns on every path: success, error or timeout.
pub(super) async fn fetch_billing(
    program: &Path,
    args: &[&str],
    timeouts: AcpTimeouts,
) -> Result<BearerBilling, ProviderError> {
    let mut child = spawn_child(program, args)?;
    let outcome = run_child(&mut child, timeouts, Utc::now()).await;
    stop(&mut child).await;
    outcome
}

fn spawn_child(program: &Path, args: &[&str]) -> Result<Child, ProviderError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ProviderError::NotInstalled(CLI_NOT_FOUND.to_string())
        } else {
            ProviderError::Other("Grok CLI failed to start".to_string())
        }
    })
}

async fn run_child(
    child: &mut Child,
    timeouts: AcpTimeouts,
    now: DateTime<Utc>,
) -> Result<BearerBilling, ProviderError> {
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(ProviderError::Other(
            "Grok CLI did not open its ACP pipes".to_string(),
        ));
    };
    let mut stdout = BufReader::new(stdout);
    billing_over_pipes(&mut stdout, &mut stdin, timeouts, now).await
}

/// `grok agent stdio` is a long-running server, so every use stops it. A timeout
/// takes the same path: the child is killed, never left running.
async fn stop(child: &mut Child) {
    child.start_kill().ok();
    tokio::time::timeout(EXIT_GRACE, child.wait()).await.ok();
}

/// The handshake and billing request over an already-open pipe pair.
pub(super) async fn billing_over_pipes<R, W>(
    reader: &mut R,
    writer: &mut W,
    timeouts: AcpTimeouts,
    now: DateTime<Utc>,
) -> Result<BearerBilling, ProviderError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    request(
        reader,
        writer,
        INITIALIZE_ID,
        "initialize",
        initialize_params(),
        timeouts.initialize,
    )
    .await?;
    let result = request(
        reader,
        writer,
        BILLING_ID,
        BILLING_METHOD,
        json!({}),
        timeouts.billing,
    )
    .await?;
    parse_billing_result(&result, now)
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": "1",
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
        },
    })
}

async fn request<R, W>(
    reader: &mut R,
    writer: &mut W,
    id: u64,
    method: &str,
    params: Value,
    budget: Duration,
) -> Result<Value, ProviderError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| ProviderError::Other("Could not encode the Grok CLI request".to_string()))?;
    line.push(b'\n');
    match tokio::time::timeout(budget, exchange(reader, writer, &line, id)).await {
        Ok(result) => result,
        Err(_elapsed) => Err(ProviderError::Timeout),
    }
}

async fn exchange<R, W>(
    reader: &mut R,
    writer: &mut W,
    line: &[u8],
    id: u64,
) -> Result<Value, ProviderError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    writer.write_all(line).await.map_err(pipe_error)?;
    writer.flush().await.map_err(pipe_error)?;
    loop {
        let Some(bytes) = read_bounded_line(reader).await.map_err(pipe_error)? else {
            return Err(ProviderError::Other(
                "Grok CLI closed its ACP output before replying".to_string(),
            ));
        };
        // Lines that are not JSON, and notifications, are not replies.
        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if let Some(reply) = reply_for(&message, id) {
            return reply;
        }
    }
}

fn pipe_error(error: io::Error) -> ProviderError {
    match error.kind() {
        io::ErrorKind::InvalidData => {
            ProviderError::Other("Grok CLI sent an oversized ACP message".to_string())
        }
        kind => ProviderError::Other(format!("Grok CLI ACP pipe failed: {kind:?}")),
    }
}

/// Reads one `\n`-terminated line, without the size of a line ever exceeding
/// [`MAX_LINE_BYTES`]. `None` means the stream ended with nothing buffered.
async fn read_bounded_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buffered = reader.fill_buf().await?;
        if buffered.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let newline = buffered.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(buffered.len(), |index| index + 1);
        if line.len() + take > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACP line exceeds the size limit",
            ));
        }
        line.extend_from_slice(&buffered[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

/// The reply to request `id`, or `None` for notifications, requests from the
/// server, and replies to other requests. A message with a `method` is never a
/// reply, even when its `id` happens to match ours.
fn reply_for(message: &Value, id: u64) -> Option<Result<Value, ProviderError>> {
    if message.get("method").is_some() {
        return None;
    }
    let message_id = message.get("id")?.as_u64()?;
    if message_id != id {
        return None;
    }
    if let Some(error) = message.get("error").filter(|error| !error.is_null()) {
        return Some(Err(rpc_error(error)));
    }
    match message.get("result") {
        Some(result) => Some(Ok(result.clone())),
        None => Some(Err(ProviderError::Parse(
            "Grok CLI reply had no result".to_string(),
        ))),
    }
}

fn rpc_error(error: &Value) -> ProviderError {
    if error.get("code").and_then(Value::as_i64) == Some(METHOD_NOT_FOUND) {
        return ProviderError::Other(
            "This Grok CLI version does not provide billing over ACP".to_string(),
        );
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown JSON-RPC error");
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("authentication required") || lowered.contains("grok login") {
        return ProviderError::AuthRequired;
    }
    ProviderError::Other(format!("Grok CLI billing request failed: {message}"))
}

/// Decodes a `_x.ai/billing` result. The current shape is the nested `config`
/// credits payload, read by the shared credits parser. The older top-level
/// `monthlyLimit` / `usage.totalUsed` shape is still accepted. A top-level
/// `subscription_tier` names the plan in both cases.
pub(super) fn parse_billing_result(
    result: &Value,
    now: DateTime<Utc>,
) -> Result<BearerBilling, ProviderError> {
    let plan = grok_plan_display_name(result.get("subscription_tier").and_then(Value::as_str));
    if let Ok(body) = serde_json::to_vec(result)
        && let Ok(mut credits) = parse_credits_response(&body, now)
        && (credits.billing.used_percent.is_some() || credits.prepaid_balance_usd.is_some())
    {
        credits.subscription_tier = plan.or(credits.subscription_tier);
        return Ok(credits);
    }
    if let Some(billing) = legacy_billing(result, now) {
        return Ok(BearerBilling {
            billing,
            subscription_tier: plan,
            prepaid_balance_usd: None,
        });
    }
    Err(ProviderError::Parse(USAGE_UNAVAILABLE.to_string()))
}

/// Legacy top-level billing shape. Monetary values are `{ "val": <cents> }`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyBilling {
    billing_cycle: Option<LegacyCycle>,
    monthly_limit: Option<LegacyCent>,
    usage: Option<LegacyUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCycle {
    billing_period_start: Option<String>,
    billing_period_end: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyUsage {
    total_used: Option<LegacyCent>,
}

#[derive(Deserialize)]
struct LegacyCent {
    val: Option<f64>,
}

/// Usage needs a positive limit and a used amount. Without both there is no
/// percent, so the payload is not accepted as legacy billing.
fn legacy_billing(result: &Value, now: DateTime<Utc>) -> Option<GrokBillingSnapshot> {
    let legacy = LegacyBilling::deserialize(result).ok()?;
    let cycle = legacy.billing_cycle.as_ref();
    let resets_at = parse_timestamp(cycle.and_then(|cycle| cycle.billing_period_end.as_deref()));
    let minutes = window_minutes(
        cycle.and_then(|cycle| cycle.billing_period_start.as_deref()),
        resets_at,
        now,
    );
    let limit = legacy.monthly_limit?.val.filter(|limit| *limit > 0.0)?;
    let used = legacy.usage?.total_used?.val?;
    Some(GrokBillingSnapshot {
        used_percent: Some((used / limit * 100.0).clamp(0.0, 100.0)),
        used_percent_is_wire_published: true,
        used_percent_is_implicit_zero: false,
        resets_at,
        window_minutes: minutes,
        product_usage: Vec::new(),
    })
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "acp_process_tests.rs"]
mod process_tests;
