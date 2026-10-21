use super::*;
use std::io::Write;
use tempfile::tempdir;

fn parse_pi_assistant_entry(value: &Value, target: PiMappedProvider) -> Option<PiEntry> {
    let entry = parse_pi_assistant_entry_any(value)?;
    (entry.provider == Some(target)).then_some(entry)
}

#[test]
fn maps_openai_codex_and_anthropic_providers() {
    assert_eq!(
        map_provider("openai-codex-responses"),
        Some(PiMappedProvider::Codex)
    );
    assert_eq!(map_provider("anthropic"), Some(PiMappedProvider::Claude));
    assert_eq!(map_provider("google"), None);
}

#[test]
fn parses_assistant_usage_row() {
    let raw = serde_json::json!({
        "id": "msg-1",
        "role": "assistant",
        "provider": "openai-codex",
        "model": "gpt-5",
        "timestamp": "2026-07-20T12:00:00Z",
        "usage": { "input": 100, "output": 20, "cacheRead": 10 }
    });
    let entry = parse_pi_assistant_entry(&raw, PiMappedProvider::Codex).unwrap();
    assert_eq!(entry.input, 100);
    assert_eq!(entry.output, 20);
    assert_eq!(entry.cache_read, 10);
    assert_eq!(entry.model, "gpt-5");
    assert!(entry.pricing_known);
}

#[test]
fn parses_astra_cache_write_and_prices_it() {
    let raw = serde_json::json!({
        "id": "astra-msg-1",
        "role": "assistant",
        "provider": "openai-codex",
        "model": "gpt-6-astra",
        "usage": {
            "input": 1_000,
            "output": 100,
            "cacheRead": 200,
            "cacheWrite": 300
        }
    });
    let entry = parse_pi_assistant_entry(&raw, PiMappedProvider::Codex).unwrap();
    let expected = 500.0 * 1e-5 + 200.0 * 1e-6 + 300.0 * 1.25e-5 + 100.0 * 5e-5;
    assert_eq!(entry.cache_create, 300);
    assert!((entry.cost - expected).abs() < 1e-12);
    assert!(entry.pricing_known);
}

#[test]
fn prices_codex_rows_at_their_utc_timestamp() {
    let cost = |timestamp: Option<&str>| {
        let mut raw = serde_json::json!({
            "id": "sol-1", "role": "assistant", "provider": "openai-codex",
            "model": "gpt-5.6-sol",
            "usage": { "input": 100, "output": 5, "cacheRead": 10, "cacheWrite": 20 }
        });
        if let Some(timestamp) = timestamp {
            raw["timestamp"] = timestamp.into();
        }
        parse_pi_assistant_entry(&raw, PiMappedProvider::Codex)
            .unwrap()
            .cost
    };
    let historical = 70.0 * 5e-6 + 10.0 * 5e-7 + 20.0 * 6.25e-6 + 5.0 * 3e-5;
    let current = 70.0 * 4e-6 + 10.0 * 4e-7 + 20.0 * 5e-6 + 5.0 * 2e-5;
    for (timestamp, expected) in [
        (Some("2026-07-10T12:00:00Z"), historical),
        (Some("2026-08-20T23:59:59Z"), historical),
        (Some("2026-08-21T00:00:00Z"), current),
        (Some("2026-09-10T12:00:00Z"), current),
        (None, current),
    ] {
        assert!((cost(timestamp) - expected).abs() < 1e-12, "{timestamp:?}");
    }
}

#[test]
fn unknown_model_keeps_tokens_and_marks_pricing_incomplete() {
    let raw = serde_json::json!({
        "id": "unknown-model-1",
        "role": "assistant",
        "provider": "openai-codex",
        "model": "future-model-without-a-rate",
        "usage": { "input": 11, "output": 3 }
    });
    let entry = parse_pi_assistant_entry_any(&raw).unwrap();
    assert!(!entry.pricing_known);
    assert_eq!(entry.cost, 0.0);

    let mut summary = CostSummary::default();
    apply_entry(&mut summary, &entry);
    assert_eq!(summary.input_tokens, 11);
    assert_eq!(summary.output_tokens, 3);
    assert!(
        summary
            .unknown_models
            .contains("future-model-without-a-rate")
    );
    assert!(summary.model_pricing_completeness.is_partial());
}

#[test]
fn standalone_parser_keeps_the_mapped_provider_for_mixed_history() {
    let codex = serde_json::json!({
        "id": "codex-1",
        "role": "assistant",
        "provider": "openai-codex",
        "model": "gpt-5",
        "usage": { "input": 100, "output": 20 }
    });
    let claude = serde_json::json!({
        "id": "claude-1",
        "role": "assistant",
        "provider": "anthropic",
        "model": "claude-sonnet-4-6",
        "usage": { "input": 100, "output": 20 }
    });

    assert_eq!(
        parse_pi_assistant_entry_any(&codex)
            .expect("Codex row should parse")
            .provider,
        Some(PiMappedProvider::Codex)
    );
    assert_eq!(
        parse_pi_assistant_entry_any(&claude)
            .expect("Claude row should parse")
            .provider,
        Some(PiMappedProvider::Claude)
    );
}

#[test]
fn unsupported_backend_keeps_tokens_as_unpriced_standalone_history() {
    let raw = serde_json::json!({
        "id": "bedrock-1",
        "role": "assistant",
        "provider": "amazon-bedrock",
        "model": "anthropic.claude-sonnet",
        "timestamp": "2026-07-20T12:00:00Z",
        "usage": { "input": 11, "output": 3, "cacheRead": 2 }
    });
    let entry = parse_pi_assistant_entry_any(&raw).expect("unsupported backend row should parse");
    assert_eq!(entry.provider, None);
    assert_eq!(entry.model, "amazon-bedrock/anthropic.claude-sonnet");
    assert!(!entry.pricing_known);
    assert_eq!(entry.cost, 0.0);

    let mut summary = CostSummary::default();
    apply_entry(&mut summary, &entry);
    assert_eq!(summary.input_tokens, 11);
    assert_eq!(summary.output_tokens, 3);
    assert_eq!(summary.cached_tokens, 2);
    assert!(
        summary
            .unknown_models
            .contains("amazon-bedrock/anthropic.claude-sonnet")
    );
    assert!(summary.model_pricing_completeness.is_partial());
}

#[test]
fn native_scans_skip_unsupported_backends_without_losing_completeness() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let codex = r#"{"id":"codex-1","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#;
    let dated = r#"{"id":"bedrock-1","role":"assistant","provider":"amazon-bedrock","model":"claude","timestamp":"2026-07-20T13:00:00Z","usage":{"input":70,"output":7}}"#;
    let undated = r#"{"id":"bedrock-2","role":"assistant","provider":"amazon-bedrock","model":"claude","usage":{"input":9,"output":1}}"#;
    std::fs::write(
        sessions.join("mixed.jsonl"),
        format!("{codex}\n{dated}\n{undated}\n"),
    )
    .unwrap();
    let cutoff = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut native = CostSummary::default();
    let evidence = scan_roots_into(
        &mut native,
        cutoff,
        None,
        &mut HashSet::new(),
        vec![sessions.clone()],
        Some(PiMappedProvider::Codex),
    );
    assert!(evidence.complete);
    assert_eq!(native.input_tokens, 50);
    assert!(native.unknown_models.is_empty());

    // The undated unsupported row keeps standalone history incomplete, but its
    // tokens are still retained.
    let mut standalone = CostSummary::default();
    let evidence = scan_roots_into(
        &mut standalone,
        cutoff,
        None,
        &mut HashSet::new(),
        vec![sessions],
        None,
    );
    assert!(!evidence.complete);
    assert_eq!(standalone.input_tokens, 50 + 70 + 9);
    assert_eq!(
        standalone.total_tokens_for_provider("pi"),
        50 + 5 + 70 + 7 + 9 + 1
    );
}

#[test]
fn malformed_unsupported_backend_row_marks_only_standalone_history_incomplete() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let codex = r#"{"id":"codex-1","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#;
    let malformed = r#"{"id":"bedrock-bad","role":"assistant","provider":"amazon-bedrock","model":"claude","timestamp":"2026-07-20T13:00:00Z","usage":{"input":"many"}}"#;
    std::fs::write(
        sessions.join("mixed.jsonl"),
        format!("{codex}\n{malformed}\n"),
    )
    .unwrap();
    let cutoff = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let mut native = CostSummary::default();
    let evidence = scan_roots_into(
        &mut native,
        cutoff,
        None,
        &mut HashSet::new(),
        vec![sessions.clone()],
        Some(PiMappedProvider::Codex),
    );
    assert!(evidence.complete);
    assert_eq!(native.input_tokens, 50);

    let scan = scan_pi_daily_from_roots(cutoff, None, vec![sessions]);
    assert!(!scan.history_coverage_established);
    assert_eq!(scan.tokens.values().sum::<u64>(), 55);
}

#[test]
fn dedupes_shared_entry_ids_across_files() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let header = r#"{"type":"session","id":"session-1"}"#;
    let line = r#"{"id":"shared-1","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#;
    for name in ["a.jsonl", "b.jsonl"] {
        let mut f = File::create(sessions.join(name)).unwrap();
        writeln!(f, "{header}").unwrap();
        writeln!(f, "{line}").unwrap();
    }

    // Point home at temp so roots resolve under .omp
    let home = dir.path().to_path_buf();
    // Manually walk the sessions we created via for_each
    let mut seen = HashSet::new();
    let mut summary = CostSummary::default();
    let mut total = 0u32;
    for name in ["a.jsonl", "b.jsonl"] {
        total += for_each_pi_entry(
            &sessions.join(name),
            DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            Some(PiMappedProvider::Codex),
            &mut seen,
            |entry| apply_entry(&mut summary, &entry),
        )
        .counted;
    }
    assert_eq!(seen.len(), 1);
    assert_eq!(summary.input_tokens, 50);
    assert_eq!(total, 1); // second file deduped
    let _ = home;
}

#[test]
fn same_message_id_in_distinct_sessions_is_not_deduped() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let line = r#"{"id":"reused-message","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#;
    for session in ["session-a", "session-b"] {
        let mut f = File::create(sessions.join(format!("{session}.jsonl"))).unwrap();
        writeln!(f, "{{\"type\":\"session\",\"id\":\"{session}\"}}").unwrap();
        writeln!(f, "{line}").unwrap();
    }

    let cutoff = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut seen = HashSet::new();
    let mut summary = CostSummary::default();
    let mut total = 0u32;
    for session in ["session-a", "session-b"] {
        total += for_each_pi_entry(
            &sessions.join(format!("{session}.jsonl")),
            cutoff,
            Some(PiMappedProvider::Codex),
            &mut seen,
            |entry| apply_entry(&mut summary, &entry),
        )
        .counted;
    }
    assert_eq!(total, 2);
    assert_eq!(summary.input_tokens, 100);
}

#[test]
fn configured_roots_include_explicit_pi_and_omp_profile_sources() {
    let home = tempdir().unwrap();
    let cwd = home.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut environment = EnvMap::new();
    environment.insert(
        "PI_CODING_AGENT_SESSION_DIR".to_string(),
        "pi-sessions".to_string(),
    );
    environment.insert("PI_CONFIG_DIR".to_string(), "config".to_string());
    environment.insert("OMP_PROFILE".to_string(), "work".to_string());

    let roots = pi_compatible_session_roots_for(home.path(), &cwd, &environment);
    let pi_root = cwd.join("pi-sessions");
    let omp_root = home
        .path()
        .join("config")
        .join("profiles")
        .join("work")
        .join("agent")
        .join("sessions");
    assert!(roots.iter().any(|root| root == &pi_root));
    assert!(roots.iter().any(|root| root == &omp_root));

    let duplicate = concat!(
        r#"{"type":"session","id":"session-1"}"#,
        "\n",
        r#"{"id":"shared","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#,
        "\n"
    );
    let unique = concat!(
        r#"{"type":"session","id":"session-2"}"#,
        "\n",
        r#"{"id":"unique","role":"assistant","provider":"anthropic","model":"claude-sonnet-4-6","timestamp":"2026-07-20T13:00:00Z","usage":{"input":70,"output":7}}"#,
        "\n"
    );
    std::fs::create_dir_all(&pi_root).unwrap();
    std::fs::create_dir_all(&omp_root).unwrap();
    std::fs::write(pi_root.join("session.jsonl"), duplicate).unwrap();
    std::fs::write(omp_root.join("session.jsonl"), duplicate).unwrap();
    std::fs::write(omp_root.join("unique.jsonl"), unique).unwrap();

    let scan = scan_pi_daily_from_roots(
        DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        None,
        roots,
    );
    assert!(scan.history_coverage_established);
    assert_eq!(scan.tokens.values().sum::<u64>(), 132);
}

#[test]
fn malformed_usage_input_keeps_valid_rows_but_marks_source_incomplete() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let valid = r#"{"id":"valid","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":11,"output":3}}"#;
    std::fs::write(
        sessions.join("mixed.jsonl"),
        format!("{valid}\n{{\"role\":\"assistant\",\"usage\":\n"),
    )
    .unwrap();

    let scan = scan_pi_daily_from_roots(
        DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        None,
        vec![sessions.clone()],
    );
    assert!(!scan.history_coverage_established);
    assert_eq!(scan.tokens.values().sum::<u64>(), 14);

    let mut summary = CostSummary::default();
    let mut seen = HashSet::new();
    let evidence = scan_roots_into(
        &mut summary,
        DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        None,
        &mut seen,
        vec![sessions],
        None,
    );
    assert!(!evidence.complete);
    assert_eq!(summary.input_tokens, 11);
}

#[test]
fn standalone_daily_scan_dedupes_pi_and_omp_roots() {
    let dir = tempdir().unwrap();
    let pi_sessions = dir.path().join(".pi").join("agent").join("sessions");
    let omp_sessions = dir.path().join(".omp").join("agent").join("sessions");
    std::fs::create_dir_all(&pi_sessions).unwrap();
    std::fs::create_dir_all(&omp_sessions).unwrap();
    let codex = r#"{"id":"shared","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":50,"output":5}}"#;
    let claude = r#"{"id":"claude-only","role":"assistant","provider":"anthropic","model":"claude-sonnet-4-6","timestamp":"2026-07-20T13:00:00Z","usage":{"input":70,"output":7}}"#;
    std::fs::write(
        pi_sessions.join("one.jsonl"),
        format!("{codex}\n{claude}\n"),
    )
    .unwrap();
    std::fs::write(omp_sessions.join("one.jsonl"), format!("{codex}\n")).unwrap();

    let scan = scan_pi_daily_from_roots(
        DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        None,
        vec![pi_sessions, omp_sessions],
    );
    assert!(scan.history_coverage_established);
    assert_eq!(scan.tokens.values().sum::<u64>(), 132);
    assert_eq!(scan.tokens.len(), 1);
}

#[test]
fn daily_tokens_count_cache_like_the_window_total() {
    let dir = tempdir().unwrap();
    let sessions = dir.path().join("agent").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let codex = r#"{"id":"cached-codex","role":"assistant","provider":"openai-codex","model":"gpt-5","timestamp":"2026-07-20T12:00:00Z","usage":{"input":100,"output":20,"cacheRead":10,"cacheWrite":5}}"#;
    let claude = r#"{"id":"cached-claude","role":"assistant","provider":"anthropic","model":"claude-sonnet-4-6","timestamp":"2026-07-20T13:00:00Z","usage":{"input":40,"output":4,"cacheRead":300,"cacheWrite":30}}"#;
    std::fs::write(
        sessions.join("cached.jsonl"),
        format!("{codex}\n{claude}\n"),
    )
    .unwrap();
    let cutoff = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let scan = scan_pi_daily_from_roots(cutoff, None, vec![sessions.clone()]);
    assert!(scan.history_coverage_established);
    let daily_total = scan.tokens.values().sum::<u64>();
    assert_eq!(daily_total, 135 + 374);

    let mut summary = CostSummary::default();
    let mut seen = HashSet::new();
    let evidence = scan_roots_into(&mut summary, cutoff, None, &mut seen, vec![sessions], None);
    assert!(evidence.complete);
    assert_eq!(summary.total_tokens_for_provider("pi"), daily_total);
}

#[test]
fn session_roots_include_pi_and_omp() {
    let dir = tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let roots = pi_compatible_session_roots_for(&home, &home, &EnvMap::new());
    assert!(
        roots
            .iter()
            .any(|p| p.ends_with(".pi/agent/sessions") || p.ends_with(".pi\\agent\\sessions"))
    );
    assert!(
        roots
            .iter()
            .any(|p| p.ends_with(".omp/agent/sessions") || p.ends_with(".omp\\agent\\sessions"))
    );
}
