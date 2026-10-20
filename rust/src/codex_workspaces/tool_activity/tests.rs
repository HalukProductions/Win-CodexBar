use serde_json::{Value, json};

use super::{
    CodexSessionToolActivity, CodexToolOperation, CodexToolOperationKind as Kind,
    CodexToolOperationOutcome as Outcome, CodexToolOperationTiming as Timing, ToolActivityScan,
    integer, native_duration,
};

const SESSION: &str = "owned";

fn record(id: &str, item_type: &str, owner: Option<&str>, extra: Value) -> Value {
    let mut item = json!({ "type": item_type, "id": id });
    if let (Some(item), Some(extra)) = (item.as_object_mut(), extra.as_object()) {
        item.extend(extra.clone());
    }
    let mut payload = json!({ "type": "item_completed", "turn_id": "turn", "item": item });
    if let Some(owner) = owner {
        payload["thread_id"] = json!(owner);
    }
    json!({ "timestamp": "2026-05-10T12:00:00Z", "type": "event_msg", "payload": payload })
}

fn owned(id: &str, item_type: &str, extra: Value) -> Value {
    record(id, item_type, Some(SESSION), extra)
}

fn jsonl(records: &[Value]) -> String {
    records.iter().map(|record| format!("{record}\n")).collect()
}

fn scan_text(text: &str) -> CodexSessionToolActivity {
    let mut scan = ToolActivityScan::new(SESSION);
    scan.scan_reader(text.as_bytes()).expect("in-memory read");
    scan.finish(|_| true)
}

fn find<'a>(activity: &'a CodexSessionToolActivity, id: &str) -> &'a CodexToolOperation {
    activity
        .operations
        .iter()
        .find(|operation| operation.item_id == id)
        .unwrap_or_else(|| panic!("operation {id}"))
}

#[test]
fn owned_native_operations_are_deduplicated_and_orchestration_is_ignored() {
    let command = owned("command", "CommandExecution", json!({ "exit_code": 1 }));
    let mcp = owned(
        "mcp",
        "McpToolCall",
        json!({ "server": "example", "tool": "read", "result": { "isError": true } }),
    );
    let foreign = record("parent", "CommandExecution", Some("parent"), json!({}));
    let model_call = json!({
        "type": "response_item",
        "payload": { "type": "function_call", "name": "exec", "call_id": "model" }
    });
    let activity = scan_text(&jsonl(&[
        command.clone(),
        command,
        mcp,
        foreign,
        model_call,
    ]));

    assert_eq!(activity.operations.len(), 2);
    let command = find(&activity, "command");
    assert_eq!(command.outcome, Outcome::NonzeroExit);
    assert_eq!(command.exit_code, Some(1));
    assert!(command.needs_attention);
    let mcp = find(&activity, "mcp");
    assert_eq!(mcp.outcome, Outcome::ToolError);
    assert_eq!(mcp.name, "example/read");
    assert_eq!(mcp.kind, Kind::Mcp);
    assert!(!activity.is_partial);
    assert_eq!(activity.ignored_record_count, 0);
}

#[test]
fn a_later_terminal_observation_updates_the_same_operation() {
    let activity = scan_text(&jsonl(&[
        owned(
            "background",
            "CommandExecution",
            json!({ "status": "in_progress" }),
        ),
        owned(
            "background",
            "CommandExecution",
            json!({ "status": "completed", "exit_code": 0, "duration": { "secs": 15, "nanos": 0 } }),
        ),
    ]));

    assert_eq!(activity.operations.len(), 1);
    let operation = &activity.operations[0];
    assert_eq!(operation.outcome, Outcome::Completed);
    assert_eq!(operation.duration_milliseconds, Some(15_000.0));
    assert_eq!(operation.timing, Some(Timing::Native));
    assert!(operation.is_slow);
    assert!(!operation.needs_attention);
}

#[test]
fn large_escaped_output_before_metadata_keeps_timing_and_outcome() {
    let output = "quote\" slash\\ braces{} Unicode你好\n".repeat(70_000);
    let encoded = serde_json::to_string(&output).expect("encode output");
    let line = format!(
        r#"{{"timestamp":"2026-05-10T12:00:00Z","type":"event_msg","payload":{{"type":"item_completed","thread_id":"owned","turn_id":"turn","item":{{"type":"CommandExecution","id":"large","aggregated_output":{encoded},"command":["echo","example"],"exit_code":2,"status":"completed","duration":{{"secs":12,"nanos":500000000}}}}}}}}"#
    );
    assert!(line.len() > 2 * 1024 * 1024);
    let activity = scan_text(&format!("{line}\n"));

    let operation = &activity.operations[0];
    assert_eq!(operation.duration_milliseconds, Some(12_500.0));
    assert_eq!(operation.timing, Some(Timing::Native));
    assert_eq!(operation.outcome, Outcome::NonzeroExit);
    assert_eq!(operation.preview.as_deref(), Some("echo example"));
    assert!(operation.is_slow);
    assert!(!activity.is_partial);
}

#[test]
fn records_larger_than_four_mebibytes_are_still_indexed() {
    let activity = scan_text(&jsonl(&[owned(
        "huge",
        "CommandExecution",
        json!({
            "command": ["echo", "synthetic"],
            "exit_code": 0,
            "aggregated_output": "你好".repeat(800_000),
        }),
    )]));

    assert_eq!(activity.operations.len(), 1);
    assert_eq!(activity.operations[0].outcome, Outcome::Completed);
    assert!(!activity.is_partial);
}

#[test]
fn duration_validation_distinguishes_zero_missing_booleans_and_overflow() {
    assert_eq!(
        native_duration(&json!({ "secs": 0, "nanos": 0 })),
        Some(0.0)
    );
    assert_eq!(
        native_duration(&json!({ "secs": 1, "nanos": 250_000_000 })),
        Some(1250.0)
    );
    assert_eq!(native_duration(&json!({ "secs": true, "nanos": 0 })), None);
    assert_eq!(native_duration(&json!({ "secs": -1, "nanos": 0 })), None);
    assert_eq!(
        native_duration(&json!({ "secs": 1, "nanos": 1_000_000_000 })),
        None
    );
    assert_eq!(native_duration(&json!({ "secs": 1.5, "nanos": 0 })), None);
    assert_eq!(native_duration(&json!({ "secs": "1", "nanos": 0 })), None);
    assert_eq!(
        native_duration(&json!({ "secs": i64::MAX, "nanos": 0 })),
        None
    );
    assert_eq!(native_duration(&json!({ "secs": 1 })), None);
    assert_eq!(integer(&json!(3.0)), Some(3));
    assert_eq!(integer(&json!(false)), None);
    assert_eq!(integer(&json!(u64::MAX)), None);
}

fn with_interval(mut record: Value, start: Value, end: Value) -> Value {
    record["payload"]["started_at_ms"] = start;
    record["payload"]["completed_at_ms"] = end;
    record
}

#[test]
fn recorded_intervals_are_labeled_and_native_timing_wins() {
    let activity = scan_text(&jsonl(&[
        with_interval(
            owned(
                "timed",
                "CommandExecution",
                json!({ "duration": { "secs": 3, "nanos": 0 }, "exit_code": 0 }),
            ),
            json!(1000),
            json!(9000),
        ),
        with_interval(
            owned("interval", "FileChange", json!({})),
            json!(1),
            json!(1),
        ),
        with_interval(
            owned("zero-start", "FileChange", json!({})),
            json!(0),
            json!(0),
        ),
        with_interval(
            owned("backwards", "FileChange", json!({})),
            json!(10),
            json!(9),
        ),
    ]));

    let timed = find(&activity, "timed");
    assert_eq!(timed.duration_milliseconds, Some(3000.0));
    assert_eq!(timed.timing, Some(Timing::Native));
    let interval = find(&activity, "interval");
    assert_eq!(interval.duration_milliseconds, Some(0.0));
    assert_eq!(interval.timing, Some(Timing::RecordedInterval));
    for id in ["zero-start", "backwards"] {
        assert_eq!(find(&activity, id).duration_milliseconds, None);
        assert_eq!(find(&activity, id).timing, None);
    }
}

#[test]
fn missing_results_are_unknown_and_failures_are_tool_errors() {
    let activity = scan_text(&jsonl(&[
        owned("one", "FileChange", json!({})),
        owned("two", "DynamicToolCall", json!({ "tool": "example" })),
        owned("failed", "CommandExecution", json!({ "status": "failed" })),
        owned("numeric", "DynamicToolCall", json!({ "success": 0 })),
    ]));

    assert_eq!(activity.operations.len(), 4);
    for id in ["one", "two", "numeric"] {
        let operation = find(&activity, id);
        assert_eq!(operation.outcome, Outcome::Unknown);
        assert_eq!(operation.duration_milliseconds, None);
        assert!(!operation.needs_attention && !operation.is_slow);
    }
    assert_eq!(find(&activity, "two").name, "example");
    assert_eq!(find(&activity, "failed").outcome, Outcome::ToolError);
}

#[test]
fn incomplete_trailing_records_are_excluded_and_reported() {
    let complete = jsonl(&[owned("tail", "CommandExecution", json!({ "exit_code": 0 }))]);
    let unterminated = complete.trim_end_matches('\n');

    let pending = scan_text(unterminated);
    assert!(pending.operations.is_empty());
    assert!(pending.is_partial);

    let finished = scan_text(&complete);
    assert_eq!(finished.operations.len(), 1);
    assert!(!finished.is_partial);

    let appended = scan_text(&format!("{complete}{{\"payload\":"));
    assert_eq!(appended.operations.len(), 1);
    assert!(appended.is_partial);
}

#[test]
fn declines_namespaces_turn_identities_and_malformed_lines_stay_distinct() {
    let mut other_turn = owned(
        "same",
        "McpToolCall",
        json!({ "server": "other", "tool": "read", "status": "declined" }),
    );
    other_turn["payload"]["turn_id"] = json!("other-turn");
    let dynamic = owned(
        "namespaced",
        "DynamicToolCall",
        json!({ "namespace": "space", "tool": "run", "success": true }),
    );
    let text = format!(
        "{}{{invalid}}\n",
        jsonl(&[
            owned(
                "same",
                "McpToolCall",
                json!({ "server": "one", "tool": "read" })
            ),
            other_turn,
            dynamic,
        ])
    );
    let activity = scan_text(&text);

    assert_eq!(activity.operations.len(), 3);
    let declined = activity
        .operations
        .iter()
        .filter(|operation| operation.outcome == Outcome::Declined)
        .collect::<Vec<_>>();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].name, "other/read");
    assert!(declined[0].needs_attention);
    let dynamic = find(&activity, "namespaced");
    assert_eq!(dynamic.name, "space/run");
    assert_eq!(dynamic.outcome, Outcome::Completed);
    assert_eq!(activity.ignored_record_count, 1);
    assert!(activity.is_partial);
}

#[test]
fn nested_mcp_content_does_not_obscure_errors() {
    let activity = scan_text(&jsonl(&[owned(
        "large-mcp",
        "McpToolCall",
        json!({
            "server": "one",
            "tool": "image",
            "arguments": { "input": "input".repeat(50_000) },
            "result": {
                "content": [{ "type": "image", "data": "a".repeat(2_000_000) }],
                "isError": true,
            },
            "duration": { "secs": 1, "nanos": 5 },
        }),
    )]));

    let operation = &activity.operations[0];
    assert_eq!(operation.outcome, Outcome::ToolError);
    assert_eq!(operation.duration_milliseconds, Some(1_000.000_005));
    assert!(!activity.is_partial);
}

#[test]
fn missing_identity_or_owner_marks_partial_without_assigning_foreign_history() {
    let mut missing_time = owned("missing-time", "CommandExecution", json!({}));
    if let Some(root) = missing_time.as_object_mut() {
        root.remove("timestamp");
    }
    let unowned = record("unowned", "McpToolCall", None, json!({}));
    let foreign = record("parent", "CommandExecution", Some("parent"), json!({}));
    let activity = scan_text(&jsonl(&[missing_time, unowned, foreign]));

    assert!(activity.operations.is_empty());
    assert_eq!(activity.ignored_record_count, 2);
    assert!(activity.is_partial);
}

#[test]
fn unrelated_record_shapes_do_not_count_as_malformed() {
    let text = jsonl(&[
        json!({ "type": "session_meta", "payload": "not an object" }),
        json!({ "type": "event_msg", "payload": { "type": "item_completed", "item": [1, 2] } }),
        json!({ "type": "event_msg", "payload": { "type": "agent_message", "message": "hi" } }),
        json!([1, 2, 3]),
    ]);
    let activity = scan_text(&format!("{text}\n\n"));

    assert!(activity.operations.is_empty());
    assert_eq!(
        activity.ignored_record_count, 1,
        "the bare array is not a record"
    );
    let objects_only = scan_text(&jsonl(&[
        json!({ "type": "session_meta", "payload": "not an object" }),
        json!({ "type": "event_msg", "payload": { "type": "agent_message" } }),
    ]));
    assert_eq!(objects_only.ignored_record_count, 0);
    assert!(!objects_only.is_partial);
}

#[test]
fn operations_are_newest_first_and_filtered_by_completion() {
    let mut early = owned("early", "WebSearch", json!({ "status": "completed" }));
    early["timestamp"] = json!("2026-05-09T08:00:00Z");
    let late = owned("late", "ImageView", json!({}));
    let mut scan = ToolActivityScan::new(SESSION);
    scan.scan_reader(jsonl(&[early, late]).as_bytes())
        .expect("in-memory read");
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-05-10T00:00:00Z")
        .expect("cutoff")
        .with_timezone(&chrono::Utc);
    let activity = scan.finish(|completed_at| *completed_at >= cutoff);

    assert_eq!(activity.operations.len(), 1);
    assert_eq!(activity.operations[0].item_id, "late");
    assert_eq!(activity.operations[0].kind, Kind::Image);

    let all = scan_text(&jsonl(&[
        owned("first", "WebSearch", json!({})),
        owned("second", "WebSearch", json!({})),
    ]));
    let order = all
        .operations
        .iter()
        .map(|operation| operation.item_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order, ["second", "first"]);
}

#[test]
fn command_previews_are_bounded_and_redaction_hides_names_and_previews() {
    let long = "x".repeat(400);
    let mut activity = scan_text(&jsonl(&[
        owned(
            "cmd",
            "CommandExecution",
            json!({ "command": ["bash", "-lc", long], "exit_code": 0 }),
        ),
        owned(
            "mcp",
            "McpToolCall",
            json!({ "server": "private", "tool": "secret" }),
        ),
        owned("ext", "Extension", json!({ "kind": "sleep" })),
    ]));
    assert_eq!(
        find(&activity, "cmd")
            .preview
            .as_ref()
            .map(|p| p.chars().count()),
        Some(160)
    );
    assert_eq!(find(&activity, "ext").name, "sleep");
    assert_eq!(find(&activity, "ext").kind, Kind::ExtensionItem);

    activity.redact_for_privacy();
    assert!(activity.redacted);
    assert_eq!(find(&activity, "cmd").preview, None);
    assert_eq!(find(&activity, "mcp").name, "mcp");
    assert_eq!(find(&activity, "ext").name, "extensionItem");
}

#[test]
fn serializes_bridge_shape() {
    let activity = scan_text(&jsonl(&[owned(
        "cmd",
        "FileChange",
        json!({ "status": "completed" }),
    )]));
    let value = serde_json::to_value(&activity).expect("serialize");
    assert_eq!(value["sessionId"], "owned");
    assert_eq!(value["ignoredRecordCount"], 0);
    assert_eq!(value["isPartial"], false);
    assert_eq!(value["redacted"], false);
    let operation = &value["operations"][0];
    assert_eq!(operation["turnId"], "turn");
    assert_eq!(operation["itemId"], "cmd");
    assert_eq!(operation["kind"], "fileChange");
    assert_eq!(operation["outcome"], "completed");
    assert_eq!(operation["completedAt"], "2026-05-10T12:00:00Z");
    assert!(operation["durationMilliseconds"].is_null());
    assert!(operation["timing"].is_null());
    assert_eq!(operation["needsAttention"], false);
    assert_eq!(operation["isSlow"], false);
}
