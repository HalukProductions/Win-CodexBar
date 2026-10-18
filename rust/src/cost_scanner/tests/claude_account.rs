//! Upstream 0.77.0 `ClaudeTokenAccountCostTests`, translated to the scanner.

use super::*;

fn row(timestamp: &str, id: &str, input: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","requestId":"request-{id}","message":{{"id":"{id}","model":"claude-sonnet-4-5-20250929","usage":{{"input_tokens":{input},"output_tokens":10}}}}}}"#
    )
}

fn write_history(config_dir: &Path, id: &str, input: u64) {
    let project = config_dir.join("projects").join("sample");
    fs::create_dir_all(&project).unwrap();
    let timestamp = Utc::now().to_rfc3339();
    fs::write(
        project.join("session.jsonl"),
        row(&timestamp, id, input) + "\n",
    )
    .unwrap();
}

fn mapped_scanner(dir: &Path) -> CostScanner {
    CostScanner::new(30).with_claude_account_config_dir(Some(dir.to_path_buf()))
}

fn tokens(summary: &CostSummary) -> u64 {
    summary.input_tokens + summary.output_tokens
}

#[test]
fn mapped_costs_follow_the_active_directory_and_never_fall_back_to_other_history() {
    let root = tempfile::tempdir().unwrap();
    let personal = root.path().join("personal");
    let work = root.path().join("work");
    let ambient = root.path().join("ambient");
    for (index, dir) in [&personal, &work, &ambient].into_iter().enumerate() {
        let index = index as u64;
        write_history(dir, &format!("message-{index}"), (index + 1) * 100);
    }

    // Switching back to an earlier mapping returns the same totals.
    for (dir, expected) in [(&personal, 110), (&work, 210), (&personal, 110)] {
        let summary = mapped_scanner(dir).scan_claude_with_cancel_and_pi_sessions(None, true);
        assert_eq!(tokens(&summary), expected);
        assert!(summary.total_cost_usd > 0.0);
        assert!(summary.history_coverage_established);
        assert!(!summary.known_zero);
    }

    let chart = mapped_scanner(&work).scan_claude_chart_snapshot_with_cancel(None);
    assert_eq!(tokens(&chart.summary), 210);
    assert_eq!(
        chart.daily_tokens.iter().map(|(_, day)| day).sum::<u64>(),
        210
    );
    assert!(chart.quota_history.history_coverage_established);
}

#[test]
fn a_missing_mapped_directory_leaves_the_estimate_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let personal = root.path().join("personal");
    write_history(&personal, "message-0", 100);
    assert_eq!(
        tokens(&mapped_scanner(&personal).scan_claude_with_cancel_and_pi_sessions(None, true)),
        110
    );

    fs::rename(&personal, root.path().join("moved")).unwrap();
    let summary = mapped_scanner(&personal).scan_claude_with_cancel_and_pi_sessions(None, true);
    assert_eq!(tokens(&summary), 0);
    assert!(!summary.history_coverage_established);
    assert!(!summary.known_zero);
}

#[test]
fn a_cleared_mapping_restores_the_combined_roots() {
    let scanner = CostScanner::new(1).with_claude_account_config_dir(None);
    assert!(!scanner.claude_projects_roots().account_mapped);
    assert!(
        CostScanner::new(1)
            .with_claude_account_config_dir(Some(PathBuf::from(r"C:\fixtures\claude")))
            .claude_projects_roots()
            .account_mapped
    );
}
