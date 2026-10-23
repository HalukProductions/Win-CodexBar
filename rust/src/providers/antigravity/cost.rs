use std::collections::{BTreeMap, HashSet};
use std::future::Future;

use chrono::{DateTime, Utc};

use crate::core::CostUsagePricing;
use crate::spend_contract::LocalCostEstimate;

/// Antigravity records routing variants of a vendor model that bill at the base model's public
/// price. The alias stays provider-local so shared Claude pricing keeps unknown variants unpriced.
const ROUTING_VARIANT_SUFFIXES: [&str; 3] = ["-tiered", "-low", "-thinking"];

/// Gemini 3.1 Pro is catalogued only as `gemini-3.1-pro-preview`. Antigravity records it under
/// product aliases and effort tiers that name no catalogued model (upstream 0.70.0 #4094;
/// ccusage's Antigravity adapter maps the same IDs). Keys match case-insensitively.
const PRICING_MODEL_ALIASES: [(&str, &str); 5] = [
    ("gemini-pro-default", "gemini-3.1-pro-preview"),
    ("gemini-pro-agent", "gemini-3.1-pro-preview"),
    ("gemini-3.1-pro", "gemini-3.1-pro-preview"),
    ("gemini-3.1-pro-high", "gemini-3.1-pro-preview"),
    ("gemini-3.1-pro-low", "gemini-3.1-pro-preview"),
];

/// models.dev entries worth refreshing for unpriced history, grouped by
/// models.dev provider: each unpriced model and, for a routing variant or a
/// product alias, its pricing base model, routed the way a rescan prices them
/// (Gemini models through `google`, Claude models through `anthropic`, GPT
/// models through `openai`).
fn refresh_targets(estimate: &LocalCostEstimate) -> BTreeMap<&'static str, HashSet<String>> {
    let mut targets = BTreeMap::<&'static str, HashSet<String>>::new();
    for model in &estimate.unpriced_models {
        for name in [Some(model.as_str()), pricing_base_model(model)]
            .into_iter()
            .flatten()
        {
            for (provider, model_id) in CostUsagePricing::claude_models_dev_pricing_targets(name) {
                targets.entry(provider).or_default().insert(model_id);
            }
        }
    }
    targets
}

/// One bounded models.dev refresh for the unpriced models of a scan, or None
/// when nothing recorded lacks a price: empty or fully priced history never
/// downloads. The refresh resolves to true when a rescan can now price at
/// least one of those models.
pub(super) fn unpriced_model_pricing_refresh(
    estimate: &LocalCostEstimate,
) -> Option<impl Future<Output = bool> + Send + use<>> {
    let targets = refresh_targets(estimate);
    (!targets.is_empty()).then(|| refresh_pricing_targets(targets))
}

async fn refresh_pricing_targets(targets: BTreeMap<&'static str, HashSet<String>>) -> bool {
    for (provider, model_ids) in &targets {
        if crate::core::refresh_unknown_models_if_needed(provider, model_ids).await {
            return true;
        }
    }
    // An earlier provider group may refresh the shared catalog without
    // resolving its own models while a later group's models became priced.
    let snapshot = crate::core::pricing_snapshot();
    targets.iter().any(|(provider, model_ids)| {
        model_ids
            .iter()
            .any(|model_id| snapshot.lookup(provider, model_id).is_some())
    })
}

pub(super) fn estimate_cost_usd(
    pricing_date: DateTime<Utc>,
    model: Option<&str>,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> Option<f64> {
    estimate_cost_usd_with(
        model,
        input,
        cache_read,
        cache_write,
        output,
        |model, input, cache_read, cache_write, output| {
            CostUsagePricing::claude_cost_usd_at_date(
                model,
                Some(pricing_date),
                input,
                cache_read,
                cache_write,
                output,
            )
        },
    )
}

/// Prices the exact recorded model first, so an explicitly catalogued variant keeps its own
/// price, then the pricing base model of a known routing variant or product alias. The recorded
/// model name itself is never rewritten; only the price lookup resolves.
fn estimate_cost_usd_with(
    model: Option<&str>,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    price: impl Fn(&str, i32, i32, i32, i32) -> Option<f64>,
) -> Option<f64> {
    let model = model.map(str::trim).filter(|value| !value.is_empty())?;
    let input = i32::try_from(input).ok()?;
    let cache_read = i32::try_from(cache_read).ok()?;
    let cache_write = i32::try_from(cache_write).ok()?;
    let output = i32::try_from(output).ok()?;
    let resolve = |candidate: &str| {
        price(candidate, input, cache_read, cache_write, output)
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
    };
    resolve(model).or_else(|| pricing_base_model(model).and_then(resolve))
}

/// Pricing base for a recorded model: a product alias's catalogued model, or the base of a
/// routing variant (itself resolved through the alias table, so `gemini-3.1-pro-thinking`
/// prices as `gemini-3.1-pro-preview`). Matches case-insensitively and keeps the recorded
/// spelling of a non-alias base.
fn pricing_base_model(model: &str) -> Option<&str> {
    if let Some(alias) = pricing_model_alias(model) {
        return Some(alias);
    }
    let lowered = model.to_ascii_lowercase();
    let suffix = ROUTING_VARIANT_SUFFIXES
        .iter()
        .find(|suffix| lowered.ends_with(*suffix))?;
    let base = model
        .get(..model.len() - suffix.len())
        .filter(|base| !base.is_empty())?;
    Some(pricing_model_alias(base).unwrap_or(base))
}

fn pricing_model_alias(model: &str) -> Option<&'static str> {
    PRICING_MODEL_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(model))
        .map(|(_, catalogued)| *catalogued)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        estimate_cost_usd, pricing_base_model, refresh_targets, unpriced_model_pricing_refresh,
    };
    use crate::spend_contract::LocalCostEstimate;
    use chrono::{DateTime, TimeZone, Utc};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap()
    }

    fn ids(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn refresh_targets_route_unpriced_models_and_their_routing_base_by_vendor() {
        let mut estimate = LocalCostEstimate::default();
        estimate.record_list_price(Some("claude-future-9-thinking"), None);
        estimate.record_list_price(Some("claude-future-9-thinking"), None);
        estimate.record_list_price(Some("gemini-future-pro-low"), None);
        estimate.record_list_price(Some("gpt-future"), None);
        estimate.record_list_price(None, None);
        let targets = refresh_targets(&estimate);
        assert_eq!(
            targets.keys().copied().collect::<Vec<_>>(),
            ["anthropic", "google", "openai"]
        );
        assert_eq!(
            targets["anthropic"],
            ids(&["claude-future-9-thinking", "claude-future-9"])
        );
        assert_eq!(
            targets["google"],
            ids(&["gemini-future-pro-low", "gemini-future-pro"])
        );
        assert_eq!(targets["openai"], ids(&["gpt-future"]));
        assert!(refresh_targets(&LocalCostEstimate::default()).is_empty());
    }

    #[test]
    fn pricing_refresh_is_offered_only_for_named_unpriced_models() {
        assert!(unpriced_model_pricing_refresh(&LocalCostEstimate::default()).is_none());

        let mut priced = LocalCostEstimate::default();
        priced.record_list_price(Some("claude-sonnet-4-6"), Some(0.25));
        assert!(unpriced_model_pricing_refresh(&priced).is_none());

        let mut unnamed = priced.clone();
        unnamed.record_list_price(None, None);
        assert_eq!(unnamed.coverage.unpriced, 1);
        assert!(unpriced_model_pricing_refresh(&unnamed).is_none());

        let mut unpriced = priced;
        unpriced.record_list_price(Some("gemini-future-pro"), None);
        assert!(unpriced_model_pricing_refresh(&unpriced).is_some());
    }

    #[test]
    fn prices_known_models_and_provider_local_routing_variants() {
        let direct = estimate_cost_usd(at(), Some("claude-sonnet-4-6"), 1_000, 200, 100, 500)
            .expect("known public price");
        let routed = estimate_cost_usd(
            at(),
            Some("claude-sonnet-4-6-thinking"),
            1_000,
            200,
            100,
            500,
        )
        .expect("routing suffix uses the base public price");
        assert!(direct > 0.0);
        assert_eq!(direct, routed);
    }

    #[test]
    fn routing_suffixes_match_case_insensitively() {
        let direct = estimate_cost_usd(at(), Some("claude-sonnet-4-6"), 1_000, 200, 100, 500)
            .expect("known public price");
        for model in [
            "claude-sonnet-4-6-Thinking",
            "claude-sonnet-4-6-LOW",
            "claude-sonnet-4-6-Tiered",
        ] {
            assert_eq!(
                estimate_cost_usd(at(), Some(model), 1_000, 200, 100, 500),
                Some(direct),
                "{model}"
            );
        }
        assert_eq!(
            pricing_base_model("Claude-Sonnet-4-6-THINKING"),
            Some("Claude-Sonnet-4-6")
        );
        assert_eq!(pricing_base_model("-thinking"), None);
        assert_eq!(pricing_base_model("claude-sonnet-4-6"), None);
    }

    #[test]
    fn unknown_or_oversized_pricing_inputs_fail_closed() {
        assert_eq!(estimate_cost_usd(at(), Some("unknown"), 1, 2, 3, 4), None);
        assert_eq!(
            estimate_cost_usd(at(), Some("unknown-thinking"), 1, 2, 3, 4),
            None
        );
        assert_eq!(estimate_cost_usd(at(), Some("  "), 1, 2, 3, 4), None);
        assert_eq!(estimate_cost_usd(at(), None, 1, 2, 3, 4), None);
        assert_eq!(
            estimate_cost_usd(
                at(),
                Some("claude-sonnet-4-6"),
                i32::MAX as u64 + 1,
                0,
                0,
                0
            ),
            None
        );
    }
}

#[cfg(test)]
mod pricing_alias_tests {
    use std::collections::HashSet;

    use super::{estimate_cost_usd_with, pricing_base_model, refresh_targets};
    use crate::core::{CostUsagePricing, ModelsDevPricingSnapshot};
    use crate::spend_contract::LocalCostEstimate;

    /// Upstream AntigravityLocalReaderTests catalog: only the preview row is catalogued.
    const PREVIEW_CATALOG: &str = r#"{
        "google": {
            "id": "google",
            "name": "Google",
            "models": {
                "gemini-3.1-pro-preview": {
                    "id": "gemini-3.1-pro-preview",
                    "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                }
            }
        }
    }"#;

    /// Prices through the same built-in-then-models.dev resolution as production, against an
    /// injected catalog instead of the per-user models.dev cache.
    fn snapshot_price(
        snapshot: &ModelsDevPricingSnapshot,
    ) -> impl Fn(&str, i32, i32, i32, i32) -> Option<f64> + '_ {
        move |model, input, cache_read, cache_write, output| {
            let normalized = CostUsagePricing::normalize_claude_model(model);
            CostUsagePricing::resolve_claude_pricing(model, &normalized, Some(snapshot)).map(
                |resolution| {
                    CostUsagePricing::claude_cost_usd_from_resolution(
                        resolution,
                        input,
                        cache_read,
                        cache_write,
                        output,
                    )
                },
            )
        }
    }

    fn assert_close(actual: Option<f64>, expected: f64, model: &str) {
        let actual = actual.unwrap_or_else(|| panic!("{model} should be priced"));
        assert!(
            (actual - expected).abs() <= expected * 1e-12,
            "{model}: {actual} != {expected}"
        );
    }

    #[test]
    fn pricing_base_model_maps_gemini_pro_aliases_and_keeps_routing_bases() {
        // Upstream v0.70.0 AntigravityLocalReaderTests expectations.
        assert_eq!(
            pricing_base_model("gemini-3.8-flash-tiered"),
            Some("gemini-3.8-flash")
        );
        assert_eq!(
            pricing_base_model("gemini-3.1-pro-low"),
            Some("gemini-3.1-pro-preview")
        );
        assert_eq!(
            pricing_base_model("claude-opus-4-6-thinking"),
            Some("claude-opus-4-6")
        );
        assert_eq!(pricing_base_model("gemini-3.8-flash"), None);
        assert_eq!(pricing_base_model("-low"), None);
        for alias in [
            "gemini-pro-default",
            "gemini-pro-agent",
            "gemini-3.1-pro",
            "gemini-3.1-pro-high",
            "gemini-3.1-pro-thinking",
        ] {
            assert_eq!(
                pricing_base_model(alias),
                Some("gemini-3.1-pro-preview"),
                "{alias}"
            );
        }
        assert_eq!(
            pricing_base_model("Gemini-Pro-Default"),
            Some("gemini-3.1-pro-preview")
        );

        // An alias reached through a routing suffix resolves the same way, in any case.
        for variant in [
            "gemini-pro-agent-tiered",
            "GEMINI-3.1-PRO-HIGH-Thinking",
            "gemini-3.1-pro-low-low",
        ] {
            assert_eq!(
                pricing_base_model(variant),
                Some("gemini-3.1-pro-preview"),
                "{variant}"
            );
        }
        // Near misses stay unaliased: the catalogued row needs no base, other Pro names are
        // not inferred.
        assert_eq!(pricing_base_model("gemini-3.1-pro-preview"), None);
        assert_eq!(pricing_base_model("gemini-pro"), None);
        assert_eq!(pricing_base_model("gemini-3.1-pro-max"), None);
        assert_eq!(
            pricing_base_model("gemini-3-pro-thinking"),
            Some("gemini-3-pro")
        );
    }

    #[test]
    fn gemini_pro_product_aliases_price_from_the_catalogued_preview_model() {
        let snapshot =
            ModelsDevPricingSnapshot::from_catalog_json_for_tests(PREVIEW_CATALOG).unwrap();
        let price = snapshot_price(&snapshot);
        // Upstream fixture turn: system prompt 11 + input 100, cache read 50, output 30 +
        // reasoning 7, priced at input 1, cache read 0.2 and output 2 USD per million tokens.
        let expected = 111e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;

        for model in [
            "gemini-3.1-pro-preview",
            "gemini-pro-default",
            "Gemini-Pro-Default",
            "gemini-pro-agent",
            "gemini-3.1-pro",
            "gemini-3.1-pro-high",
            "gemini-3.1-pro-low",
            "gemini-3.1-pro-thinking",
            "gemini-pro-agent-tiered",
        ] {
            assert_close(
                estimate_cost_usd_with(Some(model), 111, 50, 0, 37, &price),
                expected,
                model,
            );
        }

        // Unknown models stay unpriced instead of borrowing the preview rate.
        for model in [
            "gemini-pro",
            "gemini-3.1-flash",
            "claude-fixture-9-thinking",
        ] {
            assert_eq!(
                estimate_cost_usd_with(Some(model), 111, 50, 0, 37, &price),
                None,
                "{model}"
            );
        }
    }

    #[test]
    fn an_explicitly_catalogued_alias_keeps_its_own_price() {
        let snapshot = ModelsDevPricingSnapshot::from_catalog_json_for_tests(
            r#"{
                "google": {
                    "id": "google",
                    "models": {
                        "gemini-3.1-pro-preview": {
                            "id": "gemini-3.1-pro-preview",
                            "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                        },
                        "gemini-3.1-pro": {
                            "id": "gemini-3.1-pro",
                            "cost": {"input": 3, "output": 6, "cache_read": 0.6}
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let price = snapshot_price(&snapshot);

        assert_close(
            estimate_cost_usd_with(Some("gemini-3.1-pro"), 111, 50, 0, 37, &price),
            111.0 * 3e-6 + 50.0 * 0.6e-6 + 37.0 * 6e-6,
            "gemini-3.1-pro",
        );
        assert_close(
            estimate_cost_usd_with(Some("gemini-pro-default"), 111, 50, 0, 37, &price),
            111e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6,
            "gemini-pro-default",
        );
    }

    #[test]
    fn unpriced_aliases_refresh_the_catalogued_preview_row() {
        let mut estimate = LocalCostEstimate::default();
        estimate.record_list_price(Some("gemini-pro-default"), None);
        estimate.record_list_price(Some("gemini-3.1-pro-high-thinking"), None);

        let targets = refresh_targets(&estimate);

        assert_eq!(targets.keys().copied().collect::<Vec<_>>(), ["google"]);
        assert_eq!(
            targets["google"],
            [
                "gemini-pro-default",
                "gemini-3.1-pro-high-thinking",
                "gemini-3.1-pro-preview",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn shared_claude_pricing_targets_stay_free_of_provider_local_bases() {
        // Upstream: the shared resolver keeps reporting an unknown variant as unpriced rather
        // than silently billing it at the base model's rate.
        let claude_targets =
            CostUsagePricing::claude_models_dev_pricing_targets("claude-fixture-9-thinking");
        assert!(!claude_targets.is_empty());
        assert!(
            !claude_targets
                .iter()
                .any(|(_, model_id)| model_id == "claude-fixture-9")
        );
        // The Gemini Pro alias table is Antigravity's; shared pricing never borrows it.
        assert_eq!(
            CostUsagePricing::claude_models_dev_pricing_targets("gemini-pro-default"),
            [("google", "gemini-pro-default".to_string())]
        );
    }
}
