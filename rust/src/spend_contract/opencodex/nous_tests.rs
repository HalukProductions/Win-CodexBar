//! Nous Portal ledger rows (upstream 0.68.0 `OpenCodexNousUsageTests`).

use super::super::CustomRates;
use super::*;
use crate::core::ModelsDevPricingSnapshot;

/// Sanitized Hermes extractor rows, verbatim from the upstream fixture. Their
/// redacted request ids collide, so each row is aggregated on its own.
const FIXTURE: &str = concat!(
    r#"{"requestId":"nous-XXXXXXXXXXXXXXXX","timestamp":1777362346.46,"provider":"nous","model":"anthropic/claude-sonnet-4.6","usageStatus":"estimated","usage":{"inputTokens":22411,"outputTokens":5,"cacheReadInputTokens":0,"cacheCreationInputTokens":0,"reasoningOutputTokens":0,"totalTokens":22416},"surface":"hermes-gateway","_meta":{"apiCalls":1,"hermesEstimatedCostUSD":0.067,"costSource":"provider_models_api"}}"#,
    "\n",
    r#"{"requestId":"nous-XXXXXXXXXXXXXXXX","timestamp":1788143082.41,"provider":"nous","model":"z-ai/glm-5.3-flash","usageStatus":"estimated","usage":{"inputTokens":28341,"outputTokens":77,"cacheReadInputTokens":3520,"cacheCreationInputTokens":0,"reasoningOutputTokens":66,"totalTokens":31998},"surface":"hermes-gateway","_meta":{"apiCalls":1,"hermesEstimatedCostUSD":0.002,"costSource":"provider_models_api"}}"#,
    "\n",
    r#"{"requestId":"nous-XXXXXXXXXXXXXXXX","timestamp":1785982323.12,"provider":"nous","model":"deepseek/deepseek-v4-flash-0731","usageStatus":"estimated","usage":{"inputTokens":754741,"outputTokens":28710,"cacheReadInputTokens":7528192,"cacheCreationInputTokens":0,"reasoningOutputTokens":17239,"totalTokens":8303882},"surface":"hermes-gateway","_meta":{"apiCalls":32,"hermesEstimatedCostUSD":0.008,"costSource":"provider_models_api"}}"#,
    "\n",
);

const UNREPORTED: &str = r#"{"requestId":"nous-synthetic-unreported","timestamp":1777362346.46,"provider":"nous","model":"fixture-model","usageStatus":"unreported","usage":{"inputTokens":10,"outputTokens":2,"cacheReadInputTokens":3,"cacheCreationInputTokens":4,"reasoningOutputTokens":1,"totalTokens":15},"surface":"hermes-gateway","conversationID":"synthetic-session","_meta":{"apiCalls":1,"hermesEstimatedCostUSD":0,"costSource":null}}"#;

fn fixture_entries() -> Vec<OpenCodexEntry> {
    FIXTURE
        .lines()
        .map(|line| parse_line(line).expect("fixture row parses"))
        .collect()
}

/// One 2/8/0.5/3 override under `key`, trimmed and lowercased as the loader
/// stores it.
fn custom_key(key: &str) -> CustomPricing {
    CustomPricing {
        entries: HashMap::from([(
            key.to_string(),
            CustomRates {
                input: Some(2.0),
                output: Some(8.0),
                cache_read: Some(0.5),
                cache_write: Some(3.0),
            },
        )]),
    }
}

fn custom_pricing(model: &str) -> CustomPricing {
    custom_key(&format!("nous/{model}"))
}

const SONNET_2_8: &str = r#""anthropic/claude-sonnet-4.6":{"id":"anthropic/claude-sonnet-4.6","cost":{"input":2,"output":8}}"#;

fn assert_cost(cost: Option<f64>, expected: f64) {
    let cost = cost.expect("priced");
    assert!((cost - expected).abs() < 1e-10, "{cost} vs {expected}");
}

fn aggregate_one(entry: &OpenCodexEntry, custom: &CustomPricing) -> ImportedSpendSource {
    aggregate(vec![entry.clone()], entry.timestamp, 7, custom).expect("source")
}

#[test]
fn fixture_rows_keep_the_ledger_schema_and_ignore_extractor_meta() {
    let entries = fixture_entries();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.provider.as_str())
            .collect::<Vec<_>>(),
        ["nous"; 3]
    );
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.model.as_str())
            .collect::<Vec<_>>(),
        [
            "anthropic/claude-sonnet-4.6",
            "z-ai/glm-5.3-flash",
            "deepseek/deepseek-v4-flash-0731"
        ]
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.usage_status == "estimated")
    );
    assert!(entries.iter().all(|entry| entry.conversation_id.is_none()));
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.timestamp.timestamp())
            .collect::<Vec<_>>(),
        [1_777_362_346, 1_788_143_082, 1_785_982_323]
    );
    // Totals come from `usage.totalTokens` when the row has no top-level total.
    assert_eq!(
        entries
            .iter()
            .map(OpenCodexEntry::resolved_total_tokens)
            .collect::<Vec<_>>(),
        [Some(22_416), Some(31_998), Some(8_303_882)]
    );
}

#[test]
fn nous_rows_route_to_the_nous_subscription_whatever_vendor_the_model_names() {
    for entry in fixture_entries() {
        assert_eq!(
            route_entry(&entry),
            RouteTarget::Subscription("nous"),
            "{}",
            entry.model
        );
    }
    assert_eq!(
        route_entry(&entry_for("NOUS ", "openai/gpt-5")),
        RouteTarget::Subscription("nous")
    );
}

#[test]
fn custom_nous_pricing_prices_each_fixture_row_with_input_excluding_cache() {
    let expected = [0.044_862, 0.059_058, 5.503_258];
    for (entry, expected) in fixture_entries().iter().zip(expected) {
        let source = aggregate_one(entry, &custom_pricing(&entry.model));
        let cost = source.known_cost_usd.expect("priced");
        assert!(
            (cost - expected).abs() < 1e-10,
            "{}: {cost} vs {expected}",
            entry.model
        );
        assert_eq!(source.display_name, "OpenCodex");
        assert_eq!(source.token_mix.input_tokens, entry.input_tokens);
        assert_eq!(source.coverage.estimated, 1);
        assert_eq!(source.coverage.priced, 0);
        assert_eq!(source.provenance, CostProvenance::ListPriceEstimate);
        assert_eq!(source.models[0].model, entry.model);
        assert!(source.models[0].custom_pricing);
        assert_eq!(
            source.models[0].total_tokens,
            Some(entry.resolved_total_tokens().unwrap())
        );
        // A ledger row without conversationId is its own session.
        assert_eq!(source.conversation_count, 1);
    }
}

fn catalog(nous_models: &str) -> ModelsDevPricingSnapshot {
    ModelsDevPricingSnapshot::from_catalog_json_for_tests(&format!(
        r#"{{"nous":{{"models":{{{nous_models}}}}},
            "anthropic":{{"models":{{"claude-sonnet-4.6":{{
                "id":"claude-sonnet-4.6","cost":{{"input":20,"output":80}}}}}}}}}}"#
    ))
    .expect("catalog")
}

#[test]
fn nous_catalog_price_requires_the_exact_nous_model_id() {
    let entry = &fixture_entries()[0];
    let priced = catalog(
        r#""anthropic/claude-sonnet-4.6":{"id":"anthropic/claude-sonnet-4.6","cost":{"input":2,"output":8}}"#,
    );
    let cost = entry_cost(entry, &CustomPricing::default(), &priced).expect("nous catalog price");
    assert!((cost - 0.044_862).abs() < 1e-10, "{cost}");

    // Only another vendor's rates exist for the bare model id: stay unpriced.
    let vendor_only = catalog(
        r#""claude-sonnet-4.6":{"id":"claude-sonnet-4.6","cost":{"input":20,"output":80}}"#,
    );
    assert_eq!(
        entry_cost(entry, &CustomPricing::default(), &vendor_only),
        None
    );
}

#[test]
fn nous_catalog_cache_reads_are_added_back_to_the_inclusive_input() {
    let entry = &fixture_entries()[1];
    let snapshot = catalog(
        r#""z-ai/glm-5.3-flash":{"id":"z-ai/glm-5.3-flash","cost":{"input":2,"output":8,"cache_read":0.5}}"#,
    );
    let cost = entry_cost(entry, &CustomPricing::default(), &snapshot).expect("priced");
    assert!((cost - 0.059_058).abs() < 1e-10, "{cost}");
}

#[test]
fn nous_catalog_cache_writes_use_the_catalog_cache_write_rate() {
    let mut entry = parse_line(UNREPORTED).expect("row parses");
    entry.usage_status = "estimated".to_string();
    let snapshot = catalog(
        r#""fixture-model":{"id":"fixture-model","cost":{"input":2,"output":8,"cache_read":0.5,"cache_write":3}}"#,
    );
    let cost = entry_cost(&entry, &CustomPricing::default(), &snapshot).expect("priced");
    assert!((cost - 0.000_049_5).abs() < 1e-12, "{cost}");
}

#[test]
fn nous_custom_pricing_accepts_the_bare_model_key() {
    // Upstream `rates(providerID:model:)` matches the unqualified key too.
    let entry = &fixture_entries()[0];
    let custom = custom_key("anthropic/claude-sonnet-4.6");
    assert_cost(entry_cost(entry, &custom, &catalog("")), 0.044_862);
    assert!(aggregate_one(entry, &custom).models[0].custom_pricing);
}

#[test]
fn nous_rows_need_both_input_and_output_to_be_priced() {
    let entry = &fixture_entries()[0];
    let custom = custom_pricing(&entry.model);
    let snapshot = catalog(SONNET_2_8);
    assert_cost(entry_cost(entry, &custom, &snapshot), 0.044_862);
    assert_cost(
        entry_cost(entry, &CustomPricing::default(), &snapshot),
        0.044_862,
    );

    let mut no_input = entry.clone();
    no_input.input_tokens = None;
    let mut no_output = entry.clone();
    no_output.output_tokens = None;
    for partial in [&no_input, &no_output] {
        assert_eq!(entry_cost(partial, &custom, &snapshot), None);
        assert_eq!(
            entry_cost(partial, &CustomPricing::default(), &snapshot),
            None
        );
    }
}

#[test]
fn nous_catalog_never_bills_a_consumed_cache_lane_at_the_input_rate() {
    // Row 1 reads 3,520 cached tokens; its catalog entry has no cache-read rate.
    let entry = &fixture_entries()[1];
    let no_cache_read = catalog(
        r#""z-ai/glm-5.3-flash":{"id":"z-ai/glm-5.3-flash","cost":{"input":2,"output":8}}"#,
    );
    assert_eq!(
        entry_cost(entry, &CustomPricing::default(), &no_cache_read),
        None
    );

    let mut entry = parse_line(UNREPORTED).expect("row parses");
    entry.usage_status = "estimated".to_string();
    let no_cache_write = catalog(
        r#""fixture-model":{"id":"fixture-model","cost":{"input":2,"output":8,"cache_read":0.5}}"#,
    );
    assert_eq!(
        entry_cost(&entry, &CustomPricing::default(), &no_cache_write),
        None
    );
    // Without cache writes the same entry prices: 10 input, 3 cached, 2 output.
    entry.cache_creation_tokens = Some(0);
    let cost = entry_cost(&entry, &CustomPricing::default(), &no_cache_write).expect("priced");
    assert!((cost - 0.000_037_5).abs() < 1e-12, "{cost}");
}

#[test]
fn nous_catalog_ignores_dated_and_versioned_aliases() {
    let undated = catalog(SONNET_2_8);
    for alias in [
        "anthropic/claude-sonnet-4.6@20260101",
        "anthropic/claude-sonnet-4.6-20260101",
    ] {
        assert_eq!(
            entry_cost(
                &entry_for("nous", alias),
                &CustomPricing::default(),
                &undated
            ),
            None,
            "{alias}"
        );
    }
    let dated = catalog(
        r#""anthropic/claude-sonnet-4.6-20260101":{"id":"anthropic/claude-sonnet-4.6-20260101","cost":{"input":2,"output":8}}"#,
    );
    assert_eq!(
        entry_cost(&fixture_entries()[0], &CustomPricing::default(), &dated),
        None
    );
}

#[test]
fn self_prefixed_nous_models_resolve_to_the_catalog_id() {
    let entry = entry_for("nous", "Nous/anthropic/claude-sonnet-4.6");
    assert_cost(
        entry_cost(&entry, &CustomPricing::default(), &catalog(SONNET_2_8)),
        0.044_862,
    );

    // An override keyed by the catalog id prices the row and flags its model.
    let custom = custom_key("anthropic/claude-sonnet-4.6");
    assert_cost(entry_cost(&entry, &custom, &catalog("")), 0.044_862);
    assert!(aggregate_one(&entry, &custom).models[0].custom_pricing);
}

#[test]
fn legacy_openai_transport_nous_rows_are_priced_only_by_custom_overrides() {
    let entry = entry_for("openai", "nous/anthropic/claude-sonnet-4.6");
    assert_eq!(route_entry(&entry), RouteTarget::Subscription("nous"));
    // Upstream keeps their OpenAI pricing route, which has no Nous catalog.
    assert_eq!(
        entry_cost(&entry, &CustomPricing::default(), &catalog(SONNET_2_8)),
        None
    );

    let custom = custom_key("nous/anthropic/claude-sonnet-4.6");
    assert_cost(entry_cost(&entry, &custom, &catalog("")), 0.044_862);
    assert!(aggregate_one(&entry, &custom).models[0].custom_pricing);
}

#[test]
fn malformed_nous_model_ids_stay_unpriced() {
    let snapshot = catalog(concat!(
        r#""x":{"id":"x","cost":{"input":2,"output":8}},"#,
        r#""/x":{"id":"/x","cost":{"input":2,"output":8}},"#,
        r#""x/":{"id":"x/","cost":{"input":2,"output":8}}"#,
    ));
    assert!(
        entry_cost(
            &entry_for("nous", "nous/x"),
            &CustomPricing::default(),
            &snapshot
        )
        .is_some()
    );
    for model in ["nous/", "nous//x", "/x", "x/", "nous/x/"] {
        assert_eq!(
            entry_cost(
                &entry_for("nous", model),
                &CustomPricing::default(),
                &snapshot
            ),
            None,
            "{model}"
        );
    }
}

#[test]
fn unreported_rows_keep_tokens_without_dollars() {
    let entry = parse_line(UNREPORTED).expect("row parses");
    assert_eq!(
        entry.conversation_id, None,
        "conversationID is not the schema field"
    );
    let source = aggregate_one(&entry, &custom_pricing("fixture-model"));
    assert_eq!(source.known_cost_usd, None);
    assert_eq!(source.coverage.unpriced, 1);
    assert_eq!(source.coverage.estimated, 0);
    assert_eq!(source.daily[0].total_tokens, Some(15));
    assert_eq!(source.daily[0].cost_usd, None);
    assert_eq!(source.token_mix.cache_creation_tokens, Some(4));
    assert_eq!(source.token_mix.cache_read_tokens, Some(3));
}

fn entry_for(provider: &str, model: &str) -> OpenCodexEntry {
    let mut entry = fixture_entries().remove(0);
    entry.provider = provider.to_string();
    entry.model = model.to_string();
    entry
}
