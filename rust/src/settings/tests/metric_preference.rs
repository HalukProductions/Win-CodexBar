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
