use super::*;

#[test]
fn tokens_per_minute_serializes_camel_case_and_reads_both_spellings() {
    let mut settings = Settings::default();
    settings.set_provider_metric(ProviderId::OpenAIApi, MetricPreference::TokensPerMinute);

    let json = serde_json::to_string(&settings).expect("serialize settings");
    assert!(
        json.contains(r#""openaiapi":"tokensPerMinute""#),
        "bridge spelling expected in {json}"
    );

    for spelling in ["tokensPerMinute", "tokensperminute"] {
        let loaded: Settings = serde_json::from_str(&format!(
            r#"{{ "provider_metrics": {{ "openaiapi": "{spelling}" }} }}"#
        ))
        .expect("metric spelling parses");
        assert_eq!(
            loaded.get_provider_metric(ProviderId::OpenAIApi),
            MetricPreference::TokensPerMinute,
            "{spelling}"
        );
    }
}

#[test]
fn monthly_plan_serializes_camel_case_and_keeps_the_lowercase_alias() {
    let mut settings = Settings::default();
    settings.set_provider_metric(ProviderId::Mistral, MetricPreference::MonthlyPlan);

    let json = serde_json::to_string(&settings).expect("serialize settings");
    assert!(
        json.contains(r#""mistral":"monthlyPlan""#),
        "bridge spelling expected in {json}"
    );

    for spelling in ["monthlyPlan", "monthlyplan"] {
        let loaded: Settings = serde_json::from_str(&format!(
            r#"{{ "provider_metrics": {{ "mistral": "{spelling}" }} }}"#
        ))
        .expect("metric spelling parses");
        assert_eq!(
            loaded.get_provider_metric(ProviderId::Mistral),
            MetricPreference::MonthlyPlan,
            "{spelling}"
        );
    }
}

#[test]
fn camel_case_metric_keeps_the_rest_of_the_settings_file() {
    // A metric value the file parser does not know must not reset every other
    // setting: the whole document used to fall back to defaults.
    let loaded: Settings = serde_json::from_str(
        r#"{
            "refresh_interval_secs": 60,
            "provider_metrics": { "openaiapi": "tokensPerMinute" }
        }"#,
    )
    .expect("settings file parses");
    assert_eq!(loaded.refresh_interval_secs, 60);
    assert_eq!(
        loaded.get_provider_metric(ProviderId::OpenAIApi),
        MetricPreference::TokensPerMinute
    );
}

#[test]
fn unknown_metric_values_are_dropped_without_resetting_the_file() {
    let loaded: Settings = serde_json::from_str(
        r#"{
            "refresh_interval_secs": 60,
            "provider_metrics": {
                "cursor": "extra:",
                "codex": "bogus",
                "gemini": 7,
                "claude": "weekly"
            }
        }"#,
    )
    .expect("settings file parses");
    assert_eq!(loaded.refresh_interval_secs, 60);
    assert_eq!(
        loaded.get_provider_metric(ProviderId::Claude),
        MetricPreference::Weekly
    );
    assert_eq!(
        loaded.provider_metrics.len(),
        1,
        "{:?}",
        loaded.provider_metrics
    );
}

#[test]
fn named_extra_round_trips_through_the_settings_file() {
    let mut settings = Settings::default();
    settings.set_provider_metric(
        ProviderId::Cursor,
        MetricPreference::NamedExtra("cursor-grok-bot".to_string()),
    );

    let json = serde_json::to_string(&settings).expect("serialize settings");
    assert!(
        json.contains(r#""cursor":"extra:cursor-grok-bot""#),
        "bridge spelling expected in {json}"
    );
    let loaded: Settings = serde_json::from_str(&json).expect("settings file parses");
    assert_eq!(
        loaded.get_provider_metric(ProviderId::Cursor),
        MetricPreference::NamedExtra("cursor-grok-bot".to_string())
    );
}

#[test]
fn named_extra_keys_parse_and_reject_an_empty_id() {
    assert_eq!(
        MetricPreference::from_key("extra:cursor-grok-bot"),
        Some(MetricPreference::NamedExtra("cursor-grok-bot".to_string()))
    );
    assert_eq!(MetricPreference::from_key("extra:"), None);
    assert_eq!(MetricPreference::from_key("extra:  "), None);
    assert_eq!(MetricPreference::from_key("grok"), None);
    for metric in MetricPreference::all() {
        assert_eq!(
            MetricPreference::from_key(&metric.key()).as_ref(),
            Some(metric)
        );
        assert_eq!(metric.named_extra_id(), None);
    }
    assert_eq!(
        MetricPreference::NamedExtra("x".to_string()).named_extra_id(),
        Some("x")
    );
}
