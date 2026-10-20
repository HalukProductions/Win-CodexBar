//! Native Codex tool operations per conversation (upstream #4363).
//!
//! Read on demand from the conversation's local rollout logs, never during the
//! usage refresh, and never persisted. Only `event_msg/item_completed` records
//! that name this session as their thread count: command execution, MCP and
//! dynamic tools, file changes, web search, image operations and extensions.
//! Model orchestration calls are not operations. A nonzero exit code is kept
//! apart from a tool error and does not mark the user's task as failed.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::marker::PhantomData;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// An explicit display threshold, not a percentile or a failure classification.
const SLOW_OPERATION_MILLISECONDS: f64 = 10_000.0;
const PREVIEW_CHARS: usize = 160;
/// Bytes read from one rollout file before coverage is reported as partial.
pub(crate) const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
/// Operations kept for one conversation before coverage is reported as partial.
pub(crate) const MAX_OPERATIONS: usize = 20_000;
const READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexToolOperationKind {
    Command,
    Mcp,
    Dynamic,
    FileChange,
    WebSearch,
    Image,
    ExtensionItem,
}

impl CodexToolOperationKind {
    fn from_item_type(item_type: &str) -> Option<Self> {
        Some(match item_type {
            "CommandExecution" => Self::Command,
            "McpToolCall" => Self::Mcp,
            "DynamicToolCall" => Self::Dynamic,
            "FileChange" => Self::FileChange,
            "WebSearch" => Self::WebSearch,
            "ImageView" | "ImageGeneration" => Self::Image,
            "Extension" => Self::ExtensionItem,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Mcp => "mcp",
            Self::Dynamic => "dynamic",
            Self::FileChange => "fileChange",
            Self::WebSearch => "webSearch",
            Self::Image => "image",
            Self::ExtensionItem => "extensionItem",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexToolOperationOutcome {
    Completed,
    NonzeroExit,
    ToolError,
    Declined,
    Unknown,
}

/// Where a duration came from. Recorded intervals are labeled and never
/// compete with native execution time for "longest".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexToolOperationTiming {
    Native,
    RecordedInterval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexToolOperation {
    pub turn_id: String,
    pub item_id: String,
    pub kind: CodexToolOperationKind,
    /// `server/tool` for MCP, `namespace/tool` for dynamic tools, the extension
    /// kind for extensions, otherwise the native item type.
    pub name: String,
    /// First characters of the recorded command line.
    pub preview: Option<String>,
    pub completed_at: DateTime<Utc>,
    pub outcome: CodexToolOperationOutcome,
    pub exit_code: Option<i64>,
    /// `None` when no timing was recorded, which is distinct from zero.
    pub duration_milliseconds: Option<f64>,
    pub timing: Option<CodexToolOperationTiming>,
    pub needs_attention: bool,
    pub is_slow: bool,
}

/// Tool operations for one conversation, newest completion first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionToolActivity {
    pub session_id: String,
    pub operations: Vec<CodexToolOperation>,
    /// Malformed records and native operations that could not be assigned.
    pub ignored_record_count: u32,
    /// Some records were incomplete, unassignable, or beyond the scan limits.
    pub is_partial: bool,
    /// Names and previews were removed for Hide personal information.
    pub redacted: bool,
}

impl CodexSessionToolActivity {
    /// Drop tool names and command previews; outcomes and timings stay.
    pub fn redact_for_privacy(&mut self) {
        for operation in &mut self.operations {
            operation.name = operation.kind.as_str().to_string();
            operation.preview = None;
        }
        self.redacted = true;
    }
}

/// Accumulates operations across the rollout files of one session.
pub(crate) struct ToolActivityScan {
    session_id: String,
    entries: Vec<(u64, CodexToolOperation)>,
    positions: HashMap<(String, String), usize>,
    sequence: u64,
    ignored: u32,
    partial: bool,
}

impl ToolActivityScan {
    pub(crate) fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            entries: Vec::new(),
            positions: HashMap::new(),
            sequence: 0,
            ignored: 0,
            partial: false,
        }
    }

    /// A file that cannot be opened or read leaves coverage partial.
    pub(crate) fn scan_file(&mut self, path: &Path) {
        match File::open(path) {
            Ok(file) => {
                let reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
                if self.scan_reader(reader).is_err() {
                    self.partial = true;
                }
            }
            Err(_) => self.partial = true,
        }
    }

    pub(crate) fn scan_reader<R: BufRead>(&mut self, mut reader: R) -> io::Result<()> {
        let mut consumed = 0_u64;
        loop {
            if consumed >= MAX_SCAN_BYTES || self.entries.len() >= MAX_OPERATIONS {
                if !reader.fill_buf()?.is_empty() {
                    self.partial = true;
                }
                return Ok(());
            }
            let mut line = LineReader::new(&mut reader);
            // Unknown values, including multi-megabyte outputs, are skipped while
            // parsing; only the metadata fields below are retained. serde_json
            // reads one byte at a time, hence the buffer over the line.
            let parsed = serde_json::from_reader::<_, Lenient<RootRecord>>(
                BufReader::with_capacity(READ_BUFFER_BYTES, &mut line),
            );
            io::copy(&mut line, &mut io::sink())?;
            if let Some(error) = line.error.take() {
                return Err(error);
            }
            consumed = consumed.saturating_add(line.length);
            if line.length == 0 {
                return Ok(());
            }
            if !line.terminated {
                // A record without its newline may still be being written.
                if line.non_blank {
                    self.partial = true;
                }
                return Ok(());
            }
            // A JSON value that is not an object is not a record either.
            match parsed {
                Ok(Lenient(Some(record))) => self.observe(record),
                _ if line.non_blank => self.ignored = self.ignored.saturating_add(1),
                _ => {}
            }
        }
    }

    fn observe(&mut self, record: RootRecord) {
        match classify(record, &self.session_id) {
            Observation::Operation(operation) => {
                self.sequence += 1;
                let key = (operation.turn_id.clone(), operation.item_id.clone());
                // A later observation of the same item (for example a background
                // command finishing) replaces the earlier one.
                if let Some(&index) = self.positions.get(&key) {
                    self.entries[index] = (self.sequence, operation);
                } else {
                    self.positions.insert(key, self.entries.len());
                    self.entries.push((self.sequence, operation));
                }
            }
            Observation::Unassignable => self.ignored = self.ignored.saturating_add(1),
            Observation::Other => {}
        }
    }

    /// Operations whose completion passes `keep`, newest first.
    pub(crate) fn finish(self, keep: impl Fn(&DateTime<Utc>) -> bool) -> CodexSessionToolActivity {
        let mut entries = self
            .entries
            .into_iter()
            .filter(|(_, operation)| keep(&operation.completed_at))
            .collect::<Vec<_>>();
        entries.sort_by(|(left_seq, left), (right_seq, right)| {
            right
                .completed_at
                .cmp(&left.completed_at)
                .then_with(|| right_seq.cmp(left_seq))
        });
        CodexSessionToolActivity {
            session_id: self.session_id,
            operations: entries
                .into_iter()
                .map(|(_, operation)| operation)
                .collect(),
            ignored_record_count: self.ignored,
            is_partial: self.partial || self.ignored > 0,
            redacted: false,
        }
    }
}

enum Observation {
    Operation(CodexToolOperation),
    /// A native operation with no explicit owner, or missing its identity or
    /// timestamp. Counted, never guessed into this session.
    Unassignable,
    Other,
}

fn classify(record: RootRecord, session_id: &str) -> Observation {
    let Some(mut payload) = record.payload.0 else {
        return Observation::Other;
    };
    let Some(item) = payload.item.0.take() else {
        return Observation::Other;
    };
    let native = as_str(record.kind.as_ref()) == Some("event_msg")
        && as_str(payload.kind.as_ref()) == Some("item_completed");
    let Some(kind) = as_str(item.kind.as_ref()).and_then(CodexToolOperationKind::from_item_type)
    else {
        return Observation::Other;
    };
    if !native {
        return Observation::Other;
    }
    match as_str(payload.thread_id.as_ref()) {
        Some(owner) if owner == session_id => {}
        // Copied history from another thread is excluded, not partial.
        Some(_) => return Observation::Other,
        None => return Observation::Unassignable,
    }
    let turn_id = non_empty(payload.turn_id.as_ref());
    let item_id = non_empty(item.id.as_ref());
    let completed_at = as_str(record.timestamp.as_ref())
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp.trim()).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc));
    let (Some(turn_id), Some(item_id), Some(completed_at)) = (turn_id, item_id, completed_at)
    else {
        return Observation::Unassignable;
    };

    let native_duration = item.duration.as_ref().and_then(native_duration);
    let interval = recorded_interval(&payload);
    let item_type = as_str(item.kind.as_ref()).unwrap_or_default();
    let qualified = |left: Option<&Value>, right: Option<&Value>| {
        [as_str(left), as_str(right)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("/")
    };
    let name = match kind {
        CodexToolOperationKind::Mcp => qualified(item.server.as_ref(), item.tool.as_ref()),
        CodexToolOperationKind::Dynamic => qualified(item.namespace.as_ref(), item.tool.as_ref()),
        CodexToolOperationKind::ExtensionItem => as_str(item.extension_kind.as_ref())
            .unwrap_or(item_type)
            .to_string(),
        _ => item_type.to_string(),
    };
    let preview = command_line(item.command.as_ref())
        .map(|command| command.chars().take(PREVIEW_CHARS).collect::<String>());
    let outcome = outcome(&item, kind);
    let duration_milliseconds = native_duration.or(interval);
    Observation::Operation(CodexToolOperation {
        turn_id,
        item_id,
        kind,
        name: if name.is_empty() {
            item_type.to_string()
        } else {
            name
        },
        preview,
        completed_at,
        outcome,
        exit_code: item.exit_code.as_ref().and_then(integer),
        duration_milliseconds,
        timing: if native_duration.is_some() {
            Some(CodexToolOperationTiming::Native)
        } else {
            interval.map(|_| CodexToolOperationTiming::RecordedInterval)
        },
        needs_attention: matches!(
            outcome,
            CodexToolOperationOutcome::NonzeroExit
                | CodexToolOperationOutcome::ToolError
                | CodexToolOperationOutcome::Declined
        ),
        is_slow: duration_milliseconds.is_some_and(|ms| ms >= SLOW_OPERATION_MILLISECONDS),
    })
}

fn outcome(item: &ItemRecord, kind: CodexToolOperationKind) -> CodexToolOperationOutcome {
    use CodexToolOperationKind as Kind;
    use CodexToolOperationOutcome as Outcome;
    let status = as_str(item.status.as_ref());
    let exit_code = item.exit_code.as_ref().and_then(integer);
    let success = item.success.as_ref().and_then(Value::as_bool);
    if status == Some("declined") {
        return Outcome::Declined;
    }
    if kind == Kind::Command && exit_code.is_some_and(|code| code != 0) {
        return Outcome::NonzeroExit;
    }
    if matches!(kind, Kind::Mcp | Kind::Dynamic)
        && (status == Some("failed")
            || item.result.as_ref().and_then(|result| result.is_error) == Some(true)
            || success == Some(false)
            || item.error.is_some())
    {
        return Outcome::ToolError;
    }
    if status == Some("failed") && matches!(kind, Kind::FileChange | Kind::Command) {
        return Outcome::ToolError;
    }
    if status == Some("completed")
        || (kind == Kind::Command && exit_code == Some(0))
        || (kind == Kind::Dynamic && success == Some(true))
    {
        return Outcome::Completed;
    }
    Outcome::Unknown
}

/// Native `{secs, nanos}` duration in milliseconds.
pub(crate) fn native_duration(raw: &Value) -> Option<f64> {
    let secs = integer(raw.get("secs")?)?;
    let nanos = integer(raw.get("nanos")?)?;
    if secs < 0 || !(0..1_000_000_000).contains(&nanos) {
        return None;
    }
    let base = secs.checked_mul(1000)?;
    let millis = base as f64 + nanos as f64 / 1_000_000.0;
    millis.is_finite().then_some(millis)
}

fn recorded_interval(payload: &PayloadRecord) -> Option<f64> {
    let start = payload.started_at_ms.as_ref().and_then(integer)?;
    let end = payload.completed_at_ms.as_ref().and_then(integer)?;
    (start > 0 && end >= start).then(|| (end - start) as f64)
}

/// Whole JSON numbers only; booleans, strings and fractions are not integers.
pub(crate) fn integer(raw: &Value) -> Option<i64> {
    let number = raw.as_number()?;
    if let Some(value) = number.as_i64() {
        return Some(value);
    }
    if number.is_u64() {
        return None;
    }
    let value = number.as_f64()?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is whole and checked against the i64 range first"
    )]
    let whole = (value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value < i64::MAX as f64)
        .then_some(value as i64);
    whole
}

fn command_line(raw: Option<&Value>) -> Option<String> {
    let parts = raw?.as_array()?;
    let parts = parts
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join(" "))
}

fn as_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    as_str(value)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Visitor methods that accept every JSON scalar and return `$value`.
macro_rules! ignore_scalars {
    ($value:expr) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($value)
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($value)
        }
    };
}

#[derive(Deserialize)]
struct RootRecord {
    #[serde(rename = "type")]
    kind: Option<Value>,
    timestamp: Option<Value>,
    #[serde(default)]
    payload: Lenient<PayloadRecord>,
}

#[derive(Deserialize)]
struct PayloadRecord {
    #[serde(rename = "type")]
    kind: Option<Value>,
    thread_id: Option<Value>,
    turn_id: Option<Value>,
    started_at_ms: Option<Value>,
    completed_at_ms: Option<Value>,
    #[serde(default)]
    item: Lenient<ItemRecord>,
}

#[derive(Deserialize)]
struct ItemRecord {
    #[serde(rename = "type")]
    kind: Option<Value>,
    id: Option<Value>,
    status: Option<Value>,
    exit_code: Option<Value>,
    duration: Option<Value>,
    server: Option<Value>,
    tool: Option<Value>,
    namespace: Option<Value>,
    #[serde(rename = "kind")]
    extension_kind: Option<Value>,
    command: Option<Value>,
    result: Option<ResultFlags>,
    success: Option<Value>,
    /// Only presence matters; the error body is skipped.
    error: Option<IgnoredAny>,
}

/// The `isError` flag of a tool result. Result content is skipped unread.
struct ResultFlags {
    is_error: Option<bool>,
}

impl<'de> Deserialize<'de> for ResultFlags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlagsVisitor;
        impl<'de> Visitor<'de> for FlagsVisitor {
            type Value = ResultFlags;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ResultFlags, A::Error> {
                let mut is_error = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "isError" {
                        is_error = map.next_value::<Value>()?.as_bool();
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(ResultFlags { is_error })
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<ResultFlags, A::Error> {
                drain_seq(seq)?;
                Ok(ResultFlags { is_error: None })
            }
            ignore_scalars!(ResultFlags { is_error: None });
        }
        deserializer.deserialize_any(FlagsVisitor)
    }
}

/// An object decoded as `T`; any other JSON value is skipped as `None`, so an
/// unrelated record shape never fails the whole line.
struct Lenient<T>(Option<T>);

impl<T> Default for Lenient<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LenientVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for LenientVisitor<T> {
            type Value = Lenient<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Lenient<T>, A::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(|v| Lenient(Some(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Lenient<T>, A::Error> {
                drain_seq(seq)?;
                Ok(Lenient(None))
            }
            ignore_scalars!(Lenient(None));
        }
        deserializer.deserialize_any(LenientVisitor(PhantomData))
    }
}

fn drain_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

/// Reads one JSONL record: bytes up to the next newline, then end of input.
/// JSON strings cannot hold a raw newline, so the byte always ends a record.
struct LineReader<'a, R: BufRead> {
    inner: &'a mut R,
    done: bool,
    terminated: bool,
    non_blank: bool,
    length: u64,
    /// serde_json reports reader failures as parse errors; keep the real one.
    error: Option<io::Error>,
}

impl<'a, R: BufRead> LineReader<'a, R> {
    fn new(inner: &'a mut R) -> Self {
        Self {
            inner,
            done: false,
            terminated: false,
            non_blank: false,
            length: 0,
            error: None,
        }
    }
}

impl<R: BufRead> Read for LineReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let available = match self.inner.fill_buf() {
            Ok(available) => available,
            Err(error) => {
                self.done = true;
                let kind = error.kind();
                self.error = Some(error);
                return Err(io::Error::from(kind));
            }
        };
        if available.is_empty() {
            self.done = true;
            return Ok(0);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let line_end = newline.unwrap_or(available.len());
        let count = line_end.min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        if !self.non_blank {
            self.non_blank = available[..count]
                .iter()
                .any(|byte| !byte.is_ascii_whitespace());
        }
        let mut used = count;
        if newline.is_some() && count == line_end {
            used += 1;
            self.done = true;
            self.terminated = true;
        }
        self.inner.consume(used);
        self.length = self.length.saturating_add(used as u64);
        Ok(count)
    }
}

#[cfg(test)]
mod tests;
