//! OpenCodex rows priced by their recorded billing route (upstream 0.60.4
//! `OpenCodexProviderPricingTests`).
//!
//! Windows reads one custom-pricing file, the upstream application overlay.
//! It keeps the historical convention that input includes cache reads and
//! writes, so overlay cases expect the upstream application figures (for
//! example 0.00122, where an upstream caller override bills 0.00142).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::core::{
    models_dev_cache_path_for_tests, pricing_snapshot_for_tests,
    refresh_exact_pricing_targets_for_tests, save_catalog_json_for_tests,
};

const NOW_SECONDS: u64 = 2_000_000_000;

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(2_000_000_000, 0)
        .single()
        .expect("fixture time")
}

fn system_now() -> SystemTime {
    UNIX_EPOCH + StdDuration::from_secs(NOW_SECONDS)
}

fn row(provider: &str, model: &str) -> OpenCodexEntry {
    OpenCodexEntry {
        request_id: "request".to_string(),
        timestamp: now(),
        provider: provider.to_string(),
        model: model.to_string(),
        usage_status: "reported".to_string(),
        conversation_id: None,
        input_tokens: Some(100),
        output_tokens: Some(10),
        cache_read_tokens: Some(20),
        cache_creation_tokens: None,
        reasoning_tokens: None,
        total_tokens: Some(110),
    }
}

fn rate_json(rate: Option<f64>) -> String {
    rate.map_or_else(|| "null".to_string(), |rate| rate.to_string())
}

/// The upstream fixture catalog. OpenRouter prices `router_model`.
fn catalog_json(router_model: &str, router_input: f64, cache_write: Option<f64>) -> String {
    let cache_write = rate_json(cache_write);
    format!(
        r#"{{
          "openai":{{"models":{{
            "gpt-5.4":{{"id":"gpt-5.4","cost":{{"input":2,"output":8,"cache_read":0.2}}}},
            "gpt-fixture":{{"id":"gpt-fixture","cost":{{"input":2,"output":8,"cache_read":0.2}}}}
          }}}},
          "anthropic":{{"models":{{"fixture":{{"id":"fixture","cost":{{"input":1,"output":2}}}}}}}},
          "opencode-go":{{"models":{{"gpt-5.4":{{"id":"gpt-5.4","cost":{{"input":20,"output":80,"cache_read":2}}}}}}}},
          "xai":{{"models":{{"grok-fixture":{{"id":"grok-fixture","cost":{{"input":30,"output":60,"cache_read":3}}}}}}}},
          "openrouter":{{"models":{{"{router_model}":{{"id":"{router_model}","cost":{{"input":{router_input},"output":40,"cache_read":1,"cache_write":{cache_write}}}}}}}}}
        }}"#
    )
}

fn snapshot_of(json: &str) -> ModelsDevPricingSnapshot {
    ModelsDevPricingSnapshot::from_catalog_json_for_tests(json).expect("catalog")
}

fn catalog() -> ModelsDevPricingSnapshot {
    snapshot_of(&catalog_json("openai/gpt-5.4", 10.0, None))
}

fn custom(json: &str) -> CustomPricing {
    CustomPricing::parse(json.as_bytes())
}

fn priced(
    entries: Vec<OpenCodexEntry>,
    custom: &CustomPricing,
    snapshot: &ModelsDevPricingSnapshot,
) -> ImportedSpendSource {
    aggregate_with_pricing(entries, now(), 7, custom, Some(snapshot)).expect("source")
}

fn assert_cost(source: &ImportedSpendSource, expected: f64) {
    let cost = source.known_cost_usd.expect("priced");
    assert!((cost - expected).abs() < 1e-10, "{cost} vs {expected}");
    assert_eq!(source.daily[0].cost_usd, source.known_cost_usd);
    assert_eq!(source.models[0].cost_usd, source.known_cost_usd);
    assert_eq!(source.coverage.unpriced, 0);
}

fn assert_unpriced(source: &ImportedSpendSource) {
    assert_eq!(source.known_cost_usd, None);
    assert_eq!(source.daily[0].cost_usd, None);
    assert_eq!(source.models[0].cost_usd, None);
    assert_eq!(source.coverage.unpriced, 1);
}

fn pair(provider: &str, model: &str) -> (String, String) {
    (provider.to_string(), model.to_string())
}

#[test]
fn same_model_is_priced_by_its_recorded_provider() {
    let none = CustomPricing::default();
    // Upstream prices a direct `gpt-5.4` from the catalog; Windows keeps its
    // bundled Codex rate, so an unbundled OpenAI model stands in.
    let direct = priced(vec![row("openai", "gpt-fixture")], &none, &catalog());
    assert_cost(&direct, 0.000_244);
    let router = priced(vec![row("openrouter", "openai/gpt-5.4")], &none, &catalog());
    assert_cost(&router, 0.001_42);
}

#[test]
fn unqualified_subscription_model_uses_its_own_catalog_and_legacy_route_still_works() {
    let none = CustomPricing::default();
    let bare = priced(vec![row("opencode-go", "gpt-5.4")], &none, &catalog());
    let legacy = priced(
        vec![row("openai", "opencode-go/gpt-5.4")],
        &none,
        &catalog(),
    );
    assert_cost(&bare, 0.002_84);
    assert_eq!(bare.known_cost_usd, legacy.known_cost_usd);
}

#[test]
fn unknown_route_cannot_borrow_openai_prices_or_subscription_attribution() {
    let none = CustomPricing::default();
    // Neither the bundled Codex rates (empty catalog) nor the catalog's own
    // OpenAI entry may price another route's `openai/` namespace.
    let openai_only = snapshot_of(&catalog_json("other-model", 10.0, None));
    for provider in ["openrouter", "private-proxy", "xai", "google"] {
        let entry = row(provider, "openai/gpt-5.4");
        for snapshot in [snapshot_of("{}"), openai_only.clone()] {
            assert_unpriced(&priced(vec![entry.clone()], &none, &snapshot));
        }
        assert_eq!(route_entry(&entry), RouteTarget::Unknown, "{provider}");
    }
}

#[test]
fn router_namespace_does_not_fall_through_to_a_bare_model_in_the_same_catalog() {
    let snapshot = snapshot_of(&catalog_json("gpt-5.4", 10.0, None));
    assert_unpriced(&priced(
        vec![row("openrouter", "openai/gpt-5.4")],
        &CustomPricing::default(),
        &snapshot,
    ));
}

#[test]
fn partial_custom_price_remains_unknown_instead_of_silently_falling_through() {
    let partial = custom(r#"{"openrouter/openai/gpt-5.4":{"input":1}}"#);
    let source = priced(
        vec![row("openrouter", "openai/gpt-5.4")],
        &partial,
        &catalog(),
    );
    assert_unpriced(&source);
    assert!(source.models[0].custom_pricing);
}

#[test]
fn override_entries_without_a_usable_rate_never_block_the_catalog() {
    // Upstream drops an entry with no usable rate, so the row keeps its
    // catalog price and is not marked custom-priced.
    for json in [
        r#"{"openai/gpt-5.4":{}}"#,
        r#"{"openai/gpt-5.4":{"input":-1,"output":"free"}}"#,
    ] {
        let source = priced(
            vec![row("openrouter", "openai/gpt-5.4")],
            &custom(json),
            &catalog(),
        );
        assert_cost(&source, 0.001_42);
        assert!(!source.models[0].custom_pricing, "{json}");
    }
}

#[test]
fn custom_pricing_counts_cached_tokens_once_and_partial_usage_remains_unknown() {
    let overlay = custom(r#"{"openrouter/openai/gpt-5.4":{"input":10,"output":40,"cacheRead":1}}"#);
    let source = priced(
        vec![row("openrouter", "openai/gpt-5.4")],
        &overlay,
        &catalog(),
    );
    assert_cost(&source, 0.001_22);
    assert!(source.models[0].custom_pricing);

    let mut partial = row("openrouter", "openai/gpt-5.4");
    partial.request_id = "partial".to_string();
    partial.input_tokens = None;
    partial.output_tokens = None;
    partial.cache_read_tokens = None;
    partial.total_tokens = Some(500);
    let unpriced = priced(vec![partial], &CustomPricing::default(), &catalog());
    assert_eq!(unpriced.daily[0].total_tokens, Some(500));
    assert_eq!(unpriced.models[0].total_tokens, Some(500));
    assert_unpriced(&unpriced);
}

#[test]
fn custom_pricing_follows_canonical_provider_alias_after_explicit_observed_override() {
    let entry = row("kimi-coding", "kimi-coding/k3");
    let canonical = custom(r#"{"kimi-for-coding/k3":{"input":3,"output":6,"cacheRead":0.3}}"#);
    assert_cost(
        &priced(vec![entry.clone()], &canonical, &catalog()),
        0.000_306,
    );

    let explicit = custom(
        r#"{
          "kimi-coding/k3":{"input":1,"output":2,"cacheRead":0.1},
          "kimi-for-coding/k3":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_cost(&priced(vec![entry], &explicit, &catalog()), 0.000_102);
}

#[test]
fn raw_provider_custom_pricing_wins_before_normalized_targets_without_filling_missing_rates() {
    let entry = row("x-ai", "x-ai/grok-fixture");
    let explicit = custom(
        r#"{
          "x-ai/grok-fixture":{"input":1,"output":2,"cacheRead":0.1},
          "xai/grok-fixture":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_cost(
        &priced(vec![entry.clone()], &explicit, &catalog()),
        0.000_102,
    );

    let free = custom(r#"{"x-ai/grok-fixture":{"input":0,"output":0,"cacheRead":0}}"#);
    assert_cost(&priced(vec![entry.clone()], &free, &catalog()), 0.0);

    let incomplete = custom(
        r#"{
          "x-ai/grok-fixture":{"input":1},
          "xai/grok-fixture":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_unpriced(&priced(vec![entry], &incomplete, &catalog()));
}

#[test]
fn legacy_openai_transport_keeps_recorded_application_price_before_routed_price() {
    let entry = row("openai", "opencode-go/gpt-5.4");
    let application = custom(
        r#"{
          "openai/opencode-go/gpt-5.4":{"input":1,"output":2,"cacheRead":0.1},
          "gpt-5.4":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_cost(
        &priced(vec![entry.clone()], &application, &catalog()),
        0.000_102,
    );

    // Without the recorded key, the routed catalog identity's override applies.
    let routed = custom(r#"{"gpt-5.4":{"input":3,"output":6,"cacheRead":0.3}}"#);
    assert_cost(&priced(vec![entry], &routed, &catalog()), 0.000_306);
}

#[test]
fn legacy_recorded_application_zero_and_incomplete_rates_block_routed_fallback() {
    let entry = row("openai", "opencode-go/gpt-5.4");
    let free = custom(
        r#"{
          "openai/opencode-go/gpt-5.4":{"input":0,"output":0,"cacheRead":0},
          "gpt-5.4":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_cost(&priced(vec![entry.clone()], &free, &catalog()), 0.0);

    let incomplete = custom(
        r#"{
          "openai/opencode-go/gpt-5.4":{"input":1},
          "gpt-5.4":{"input":3,"output":6,"cacheRead":0.3}
        }"#,
    );
    assert_unpriced(&priced(vec![entry], &incomplete, &catalog()));
}

#[test]
fn bare_application_overrides_retain_precedence_over_provider_qualified_overrides() {
    for (provider, model) in [("openai", "gpt-5.4"), ("openai", "opencode-go/gpt-5.4")] {
        let application = custom(&format!(
            r#"{{
              "{model}":{{"input":0,"output":0,"cacheRead":0}},
              "{provider}/{model}":{{"input":3,"output":6,"cacheRead":0.3}}
            }}"#
        ));
        assert_cost(
            &priced(vec![row(provider, model)], &application, &catalog()),
            0.0,
        );
    }
}

#[test]
fn generic_catalog_row_with_consumed_cache_tokens_and_no_cache_rate_stays_unpriced() {
    let snapshot = snapshot_of(
        r#"{
          "anthropic":{"models":{"fixture":{"id":"fixture","cost":{"input":1,"output":2}}}},
          "openai":{"models":{"fixture":{"id":"fixture","cost":{"input":1,"output":2}}}},
          "xai":{"models":{"grok-fixture":{"id":"grok-fixture","cost":{"input":2,"output":8}}}}
        }"#,
    );
    assert_unpriced(&priced(
        vec![row("xai", "grok-fixture")],
        &CustomPricing::default(),
        &snapshot,
    ));
}

#[test]
fn cache_creation_is_priced_once_and_requires_an_explicit_rate() {
    let mut entry = row("openrouter", "openai/gpt-5.4");
    entry.request_id = "cache-creation".to_string();
    entry.cache_creation_tokens = Some(30);
    entry.total_tokens = Some(160);
    let snapshot = snapshot_of(
        r#"{"openrouter":{"models":{"openai/gpt-5.4":{"id":"openai/gpt-5.4",
            "cost":{"input":10,"output":40,"cache_read":1,"cache_write":5}}}}}"#,
    );
    let none = CustomPricing::default();
    assert_cost(&priced(vec![entry.clone()], &none, &snapshot), 0.001_57);

    // The overlay takes cache reads and writes out of the inclusive input.
    let overlay = custom(
        r#"{"openrouter/openai/gpt-5.4":{"input":10,"output":40,"cacheRead":1,"cacheWrite":5}}"#,
    );
    assert_cost(&priced(vec![entry.clone()], &overlay, &snapshot), 0.001_07);

    let unpriced = priced(vec![entry], &none, &catalog());
    assert_eq!(unpriced.daily[0].total_tokens, Some(160));
    assert_unpriced(&unpriced);
}

#[test]
fn cache_only_rows_preserve_complete_free_and_missing_cache_prices() {
    let mut entry = row("openrouter", "openai/gpt-5.4");
    entry.request_id = "cache-only".to_string();
    entry.input_tokens = Some(0);
    entry.output_tokens = Some(0);
    entry.cache_read_tokens = None;
    entry.cache_creation_tokens = Some(30);
    entry.total_tokens = None;
    for (rate, expected) in [
        (Some(5.0), Some(0.000_15)),
        (Some(0.0), Some(0.0)),
        (None, None),
    ] {
        let snapshot = snapshot_of(&catalog_json("openai/gpt-5.4", 10.0, rate));
        let overlay = custom(&format!(
            r#"{{"openrouter/openai/gpt-5.4":{{"input":10,"output":40,"cacheRead":1,"cacheWrite":{}}}}}"#,
            rate_json(rate)
        ));
        for custom in [CustomPricing::default(), overlay] {
            let source = priced(vec![entry.clone()], &custom, &snapshot);
            assert_eq!(source.daily[0].total_tokens, Some(30), "{rate:?}");
            match expected {
                Some(expected) => assert_cost(&source, expected),
                None => assert_unpriced(&source),
            }
        }
    }
}

#[test]
fn independent_cache_conversion_overflow_stays_unpriced() {
    let mut entry = row("openrouter", "openai/gpt-5.4");
    entry.request_id = "cache-overflow".to_string();
    entry.input_tokens = Some(u64::MAX);
    entry.output_tokens = Some(0);
    entry.cache_read_tokens = None;
    entry.cache_creation_tokens = Some(1);
    entry.total_tokens = None;
    let snapshot = snapshot_of(&catalog_json("openai/gpt-5.4", 10.0, Some(5.0)));
    assert_unpriced(&priced(vec![entry], &CustomPricing::default(), &snapshot));
}

#[test]
fn direct_and_legacy_routed_rows_keep_the_historical_application_convention() {
    for model in ["gpt-5.4", "opencode-go/gpt-5.4"] {
        let overlay = custom(&format!(
            r#"{{"{model}":{{"input":10,"output":40,"cacheRead":1}}}}"#
        ));
        assert_cost(
            &priced(vec![row("openai", model)], &overlay, &catalog()),
            0.001_22,
        );
    }
}

#[test]
fn refresh_targets_cover_only_imported_rows_that_read_the_catalog() {
    let mut future = row("opencode-go", "future-model");
    future.timestamp = now() + Duration::seconds(1);
    let mut unsupported = row("opencode-go", "unsupported-model");
    unsupported.usage_status = "unsupported".to_string();
    let entries = [
        // Not imported on Windows: unknown and token-only routes.
        row("openrouter", "openai/gpt-5.4"),
        row("opencode-free", "free-model"),
        // Never read the catalog: bundled Codex rates and model-less rows.
        row("openai", "gpt-5.4"),
        row("openai", "unknown"),
        // Never priced: unsupported status, or not yet recorded.
        unsupported,
        future,
        row("openai", "gpt-fixture"),
        row("opencode-go", "gpt-5.4"),
        row("openai", "opencode-go/gpt-5.4"),
        row("kimi-coding", "kimi-coding/k3"),
    ];
    let targets: Vec<_> = pricing_targets(&entries, now())
        .into_iter()
        .map(|target| (target.provider_id, target.model_id))
        .collect();
    assert_eq!(
        targets,
        vec![
            pair("kimi-coding", "k3"),
            pair("kimi-for-coding", "k3"),
            pair("openai", "gpt-fixture"),
            pair("opencode-go", "gpt-5.4"),
        ]
    );
}

/// A plausible catalog whose OpenCode Go entry is `go_model` at `go_input`.
fn go_catalog_json(go_model: &str, go_input: f64) -> String {
    format!(
        r#"{{
          "openai":{{"models":{{"gpt-5.4":{{"id":"gpt-5.4","cost":{{"input":2,"output":8,"cache_read":0.2}}}}}}}},
          "anthropic":{{"models":{{"fixture":{{"id":"fixture","cost":{{"input":1,"output":2}}}}}}}},
          "opencode-go":{{"models":{{"{go_model}":{{"id":"{go_model}","cost":{{"input":{go_input},"output":80,"cache_read":2}}}}}}}}
        }}"#
    )
}

fn cached_cost(entries: &[OpenCodexEntry], root: &std::path::Path) -> Option<f64> {
    let snapshot = pricing_snapshot_for_tests(system_now(), root);
    aggregate_with_pricing(
        entries.to_vec(),
        now(),
        7,
        &CustomPricing::default(),
        Some(&snapshot),
    )
    .expect("source")
    .known_cost_usd
}

#[tokio::test]
async fn fresh_catalog_miss_refreshes_the_route_and_reprices() {
    let root = tempfile::tempdir().unwrap();
    let before = system_now() - StdDuration::from_secs(901);
    assert!(save_catalog_json_for_tests(
        &go_catalog_json("gpt-5.4-mini", 20.0),
        before,
        root.path()
    ));
    let entries = [row("opencode-go", "gpt-5.4")];
    assert_eq!(cached_cost(&entries, root.path()), None);

    let targets = pricing_targets(&entries, now());
    let calls = Arc::new(AtomicUsize::new(0));
    let response = Some(go_catalog_json("gpt-5.4", 20.0));
    refresh_exact_pricing_targets_for_tests(
        &targets,
        system_now(),
        root.path(),
        response.clone(),
        Arc::clone(&calls),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let cost = cached_cost(&entries, root.path()).expect("refreshed price");
    assert!((cost - 0.002_84).abs() < 1e-10, "{cost}");

    refresh_exact_pricing_targets_for_tests(
        &targets,
        system_now(),
        root.path(),
        response,
        Arc::clone(&calls),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_price_refresh_changes_costs_while_failure_preserves_the_last_good_rates() {
    let root = tempfile::tempdir().unwrap();
    let old = go_catalog_json("gpt-5.4", 5.0);
    let stale = system_now() - StdDuration::from_secs(90_000);
    assert!(save_catalog_json_for_tests(&old, stale, root.path()));
    let entries = [row("opencode-go", "gpt-5.4")];
    // A stale catalog prices nothing until it is refreshed.
    assert_eq!(cached_cost(&entries, root.path()), None);

    let targets = pricing_targets(&entries, now());
    let calls = Arc::new(AtomicUsize::new(0));
    refresh_exact_pricing_targets_for_tests(
        &targets,
        system_now(),
        root.path(),
        Some(go_catalog_json("gpt-5.4", 20.0)),
        Arc::clone(&calls),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let old_cost = priced(
        entries.to_vec(),
        &CustomPricing::default(),
        &snapshot_of(&old),
    )
    .known_cost_usd;
    let refreshed_cost = cached_cost(&entries, root.path());
    assert!(refreshed_cost.is_some());
    assert_ne!(old_cost, refreshed_cost);

    let cache_path = models_dev_cache_path_for_tests(root.path());
    let refreshed_bytes = std::fs::read(&cache_path).unwrap();
    let failures = Arc::new(AtomicUsize::new(0));
    refresh_exact_pricing_targets_for_tests(
        &targets,
        system_now() + StdDuration::from_secs(90_000),
        root.path(),
        None,
        Arc::clone(&failures),
    )
    .await;
    assert_eq!(failures.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&cache_path).unwrap(), refreshed_bytes);
}
