//! Claude Desktop Cowork audit transcripts in Claude cost history (upstream
//! 0.77.1 #3209). Cowork rows use the same parser as CLI transcripts, so a
//! response present in both is counted once, and `result` rows never count.
//! Cowork history belongs to the default history only: a mapped token account
//! or a set `CLAUDE_CONFIG_DIR` leaves it out.
//!
//! Each fixture runs in a child process so `CLAUDE_CONFIG_DIR` and the home
//! directory are exactly the temp dirs this test sets.

use super::*;

const CHILD_MARKER: &str = "CODEXBAR_CLAUDE_COWORK_TEST_CHILD";
const CHILD_DONE: &str = "claude cowork fixtures verified";

/// One assistant response. `session_key` is `sessionId` for CLI transcripts
/// and `session_id` for Cowork audit files.
fn response(
    timestamp: &str,
    session_key: &str,
    session_id: &str,
    message_id: &str,
    input: u64,
    output: u64,
) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","{session_key}":"{session_id}","message":{{"id":"{message_id}","model":"claude-sonnet-4-6","usage":{{"input_tokens":{input},"output_tokens":{output}}}}}}}"#
    )
}

/// A Cowork `result` row carries session totals. It must never be counted.
fn result_totals(timestamp: &str) -> String {
    format!(
        r#"{{"type":"result","timestamp":"{timestamp}","session_id":"s3","usage":{{"input_tokens":99999,"output_tokens":99999}}}}"#
    )
}

fn write_lines(path: &Path, lines: &[String]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
}

/// (input, output, sessions) of a summary, the three values this fixture pins.
fn totals(summary: &CostSummary) -> (u64, u64, u32) {
    (
        summary.input_tokens,
        summary.output_tokens,
        summary.sessions_count,
    )
}

fn child_body(config_dir_is_set: bool) {
    let home = PathBuf::from(std::env::var_os("CODEXBAR_TEST_HOME").unwrap());
    let now = Utc::now().to_rfc3339();
    let app_data = home.join("appdata");
    let audit = app_data
        .join("local-agent-mode-sessions")
        .join("acct")
        .join("ws")
        .join("sess")
        .join("audit.jsonl");

    if config_dir_is_set {
        // CLAUDE_CONFIG_DIR is set: only that folder is scanned, so the Cowork
        // audit row (m3) must stay out and the folder's own row (m2) counts.
        let config = PathBuf::from(std::env::var_os("CLAUDE_CONFIG_DIR").unwrap());
        write_lines(
            &config.join("projects").join("p").join("a.jsonl"),
            &[response(&now, "sessionId", "s2", "m2", 2000, 0)],
        );
        write_lines(
            &audit,
            &[response(&now, "session_id", "s3", "m3", 300, 100)],
        );
        let summary = CostScanner::new(1)
            .with_claude_desktop_app_data(vec![app_data])
            .scan_claude();
        assert_eq!(totals(&summary), (2000, 0, 1), "configured dir only");
    } else {
        // CLI history: m1 (s1) and m2 (s2) under the default projects folder.
        write_lines(
            &home
                .join(".claude")
                .join("projects")
                .join("cli")
                .join("cli.jsonl"),
            &[
                response(&now, "sessionId", "s1", "m1", 1000, 500),
                response(&now, "sessionId", "s2", "m2", 2000, 0),
            ],
        );
        // Cowork audit: m1 again (same session and message, so it dedupes),
        // a new m3, and a result row whose totals must be ignored.
        write_lines(
            &audit,
            &[
                response(&now, "session_id", "s1", "m1", 1000, 500),
                response(&now, "session_id", "s3", "m3", 300, 100),
                result_totals(&now),
            ],
        );

        let summary = CostScanner::new(1)
            .with_claude_desktop_app_data(vec![app_data.clone()])
            .scan_claude();
        // m1 once + m2 + m3 = 3300 input, 600 output, from two files.
        assert_eq!(totals(&summary), (3300, 600, 2), "cowork deduped");
        assert!(summary.history_coverage_established);

        let chart = CostScanner::new(1)
            .with_claude_desktop_app_data(vec![app_data])
            .scan_claude_chart_snapshot_with_cancel(None);
        assert_eq!(totals(&chart.summary), (3300, 600, 2), "chart matches");
        let daily_tokens: u64 = chart.daily_tokens.iter().map(|(_, tokens)| tokens).sum();
        assert_eq!(daily_tokens, 3900, "daily tokens match the summary");

        // A mapped token account scans only its own folder and never Cowork.
        let account = home.join("account");
        write_lines(
            &account.join("projects").join("p").join("a.jsonl"),
            &[response(&now, "sessionId", "s2", "m2", 2000, 0)],
        );
        let mapped = CostScanner::new(1)
            .with_claude_desktop_app_data(vec![home.join("appdata")])
            .with_claude_account_config_dir(Some(account))
            .scan_claude();
        assert_eq!(
            totals(&mapped),
            (2000, 0, 1),
            "mapped account excludes cowork"
        );
    }
    println!("{CHILD_DONE}");
}

fn run_child(test_name: &str, home: &Path, config_dir: Option<&Path>) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD_MARKER, "1")
        // Every home-relative root (claude-swap, Pi, OMP) resolves here.
        .env("CODEXBAR_TEST_HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR");
    if let Some(config_dir) = config_dir {
        command.env("CLAUDE_CONFIG_DIR", config_dir);
    }
    let output = command.output().expect("spawn isolated exact-test child");
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains(CHILD_DONE),
        "fixture child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn claude_cowork_history_counts_once_and_stays_in_default_history() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        child_body(std::env::var_os("CLAUDE_CONFIG_DIR").is_some());
        return;
    }
    let test_thread = std::thread::current();
    let test_name = test_thread.name().expect("test harness names this thread");

    let default_home = tempfile::tempdir().unwrap();
    run_child(test_name, default_home.path(), None);

    let configured_home = tempfile::tempdir().unwrap();
    let config = configured_home.path().join("config");
    run_child(test_name, configured_home.path(), Some(&config));
}
