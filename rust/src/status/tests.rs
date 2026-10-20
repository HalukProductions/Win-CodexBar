use chrono::{TimeZone, Utc};

use super::*;
use crate::core::ProviderId;

const GEMINI: &str = "npdyhgECDJ6tB66MxXyo";

#[test]
fn statuspage_feed_keeps_indicator_description_and_date() {
    let body = br#"{
      "page": {
        "id": "yh6f0r4529hb",
        "name": "Claude",
        "url": "https://status.claude.com",
        "time_zone": "Etc/UTC",
        "updated_at": "2026-09-12T10:00:00.125Z"
      },
      "status": {"indicator": "major", "description": "Service unavailable"}
    }"#;
    let status = parse_statuspage_status(body).unwrap();
    assert_eq!(status.indicator, StatusIndicator::Major);
    assert_eq!(status.description.as_deref(), Some("Service unavailable"));
    assert_eq!(
        status.updated_at,
        Some(
            Utc.with_ymd_and_hms(2026, 9, 12, 10, 0, 0).unwrap()
                + chrono::Duration::milliseconds(125)
        )
    );
    assert_eq!(status.indicator.cli_label(), "Major outage");
}

#[test]
fn statuspage_raw_indicators_map_like_upstream() {
    let cases = [
        ("none", StatusIndicator::Operational),
        ("minor", StatusIndicator::Minor),
        ("major", StatusIndicator::Major),
        ("critical", StatusIndicator::Critical),
        ("maintenance", StatusIndicator::Maintenance),
        ("new-provider-state", StatusIndicator::Unknown),
    ];
    for (raw, expected) in cases {
        let body = format!(r#"{{"status":{{"indicator":"{raw}"}}}}"#);
        let status = parse_statuspage_status(body.as_bytes()).unwrap();
        assert_eq!(status.indicator, expected, "raw indicator {raw}");
        assert_eq!(status.description, None);
        assert_eq!(status.updated_at, None);
    }
}

#[test]
fn statuspage_bad_date_fails_the_feed() {
    let body = br#"{"page":{"updated_at":"bad-date"},"status":{"indicator":"none"}}"#;
    assert!(parse_statuspage_status(body).is_err());
}

#[test]
fn workspace_feed_selects_worst_active_incident() {
    let body = br#"[
      {
        "id": "inc-1",
        "begin": "2025-12-02T09:00:00+00:00",
        "end": null,
        "affected_products": [{"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}],
        "most_recent_update": {
          "when": "2025-12-02T10:00:00+00:00",
          "status": "SERVICE_INFORMATION",
          "text": "**Summary**\nMinor issue.\n"
        }
      },
      {
        "id": "inc-2",
        "begin": "2025-12-02T11:00:00+00:00",
        "end": null,
        "affected_products": [{"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}],
        "most_recent_update": {
          "when": "2025-12-02T12:00:00+00:00",
          "status": "SERVICE_OUTAGE",
          "text": "**Summary**\nGemini API error.\n"
        }
      }
    ]"#;
    let status = parse_google_workspace_status(body, GEMINI).unwrap();
    assert_eq!(status.indicator, StatusIndicator::Critical);
    assert_eq!(status.description.as_deref(), Some("Gemini API error."));
    assert_eq!(
        status.updated_at,
        Some(Utc.with_ymd_and_hms(2025, 12, 2, 12, 0, 0).unwrap())
    );
}

#[test]
fn workspace_feed_ignores_resolved_incidents() {
    let body = br#"[
      {
        "id": "inc-3",
        "begin": "2025-12-02T08:00:00+00:00",
        "end": "2025-12-02T09:00:00+00:00",
        "affected_products": [{"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}],
        "most_recent_update": {
          "when": "2025-12-02T09:00:00+00:00",
          "status": "AVAILABLE",
          "text": "**Summary**\nResolved.\n"
        }
      }
    ]"#;
    let status = parse_google_workspace_status(body, GEMINI).unwrap();
    assert_eq!(status.indicator, StatusIndicator::Operational);
    assert_eq!(status.description, None);
    assert_eq!(status.updated_at, None);
}

#[test]
fn workspace_feed_prefers_currently_affected_products_and_falls_back_to_severity() {
    let body = br#"[
      {
        "id": "other-product",
        "begin": "2025-12-02T08:00:00+00:00",
        "end": null,
        "affected_products": [{"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}],
        "currently_affected_products": [{"title": "Gmail", "id": "gmail"}],
        "most_recent_update": {"status": "SERVICE_OUTAGE", "text": "Gmail down"}
      },
      {
        "id": "gemini-medium",
        "begin": "2025-12-02T08:30:00+00:00",
        "modified": "2025-12-02T08:45:00Z",
        "end": null,
        "severity": "medium",
        "affected_products": [{"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}],
        "updates": [
          {"text": "older"},
          {"text": "**Description**\n- See [the dashboard](https://example.invalid/x) for details\n"}
        ]
      }
    ]"#;
    let status = parse_google_workspace_status(body, GEMINI).unwrap();
    assert_eq!(status.indicator, StatusIndicator::Major);
    assert_eq!(
        status.description.as_deref(),
        Some("See the dashboard for details")
    );
    assert_eq!(
        status.updated_at,
        Some(Utc.with_ymd_and_hms(2025, 12, 2, 8, 45, 0).unwrap())
    );
}

#[test]
fn status_sources_match_upstream_descriptors() {
    assert_eq!(
        status_source(ProviderId::Claude),
        Some(StatusSource::Statuspage("https://status.claude.com"))
    );
    assert_eq!(
        status_source(ProviderId::ClaudePlatform),
        Some(StatusSource::Statuspage("https://status.claude.com"))
    );
    assert_eq!(
        status_source(ProviderId::Codex),
        Some(StatusSource::Statuspage("https://status.openai.com"))
    );
    assert_eq!(
        status_source(ProviderId::Copilot),
        Some(StatusSource::Statuspage("https://www.githubstatus.com"))
    );
    assert_eq!(
        status_source(ProviderId::Gemini),
        Some(StatusSource::GoogleWorkspace("npdyhgECDJ6tB66MxXyo"))
    );
    assert_eq!(
        status_source(ProviderId::Antigravity),
        Some(StatusSource::GoogleWorkspace("npdyhgECDJ6tB66MxXyo"))
    );
    assert_eq!(status_source(ProviderId::Zai), None);
    let polled = ProviderId::all()
        .iter()
        .filter(|id| status_source(**id).is_some())
        .count();
    assert_eq!(polled, 12);
}

#[test]
fn summary_falls_back_to_cli_label() {
    let mut status = ProviderStatus {
        indicator: StatusIndicator::Minor,
        description: None,
        updated_at: None,
    };
    assert_eq!(status.summary(), "Partial outage");
    status.description = Some("Elevated errors".into());
    assert_eq!(status.summary(), "Elevated errors");
}

#[test]
fn parse_errors_never_quote_the_feed() {
    let body = br#"{"status": {"indicator": 42, "description": "token=abc123"}}"#;
    for source in [
        StatusSource::Statuspage("https://status.example.invalid"),
        StatusSource::GoogleWorkspace("product"),
    ] {
        assert_eq!(
            parse_source(source, body),
            Err("invalid status feed".to_string())
        );
    }
}
