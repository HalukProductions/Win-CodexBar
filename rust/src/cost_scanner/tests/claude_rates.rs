//! Claude pricing as the scanner applies it.

use super::*;

/// Scanner Claude pricing without the per-scan memo, as an oracle.
struct ClaudePricing;

impl ClaudePricing {
    /// Undated usage, priced at current rates.
    fn cost_usd_with_cache_ttl(
        model: &str,
        input: u64,
        cache_create: u64,
        cache_create_1h: u64,
        cache_read: u64,
        output: u64,
    ) -> f64 {
        Self::cost_usd_with_cache_ttl_at(
            model,
            None,
            input,
            cache_create,
            cache_create_1h,
            cache_read,
            output,
        )
    }

    /// Built-in rows first, then the models.dev snapshot, with Sonnet as the fallback for
    /// unpriced IDs. Same resolution order as the memo, without the memo.
    fn cost_usd_with_cache_ttl_at(
        model: &str,
        pricing_date: Option<chrono::DateTime<chrono::Utc>>,
        input: u64,
        cache_create: u64,
        cache_create_1h: u64,
        cache_read: u64,
        output: u64,
    ) -> f64 {
        let resolve = |model: &str| {
            let normalized = CostUsagePricing::normalize_claude_model(model);
            CostUsagePricing::resolve_claude_pricing(model, &normalized, None).or_else(|| {
                let snapshot = crate::core::pricing_snapshot();
                CostUsagePricing::resolve_claude_pricing(model, &normalized, Some(&snapshot))
            })
        };
        let billable = resolve(model).or_else(|| resolve(FALLBACK_CLAUDE_MODEL));
        billable
            .map(|pricing| {
                CostUsagePricing::claude_cost_usd_u64_from_resolution_with_one_hour_writes(
                    pricing,
                    pricing_date,
                    input,
                    cache_read,
                    cache_create,
                    cache_create_1h,
                    output,
                )
            })
            .unwrap_or(0.0)
    }
}

#[test]
fn test_unknown_model_falls_back_to_sonnet() {
    // Unknown/retired Claude IDs fall back to Sonnet 4.6 base pricing
    // ($3/1M input, $15/1M output). 100k tokens stay under the 200k tier.
    let cost =
        ClaudePricing::cost_usd_with_cache_ttl("claude-3-5-sonnet", 100_000, 0, 0, 0, 100_000);
    // 100k * $3/M + 100k * $15/M = 0.30 + 1.50 = 1.80
    assert!((cost - 1.80).abs() < 0.001);
}

#[test]
fn records_unknown_claude_model_while_using_fallback_cost() {
    let event: ClaudeEvent = serde_json::from_str(
        r#"{"type":"assistant","timestamp":"2026-01-15T10:00:00Z","requestId":"req_unknown","message":{"id":"msg_unknown","model":"claude-retired-unknown","usage":{"input_tokens":100000,"output_tokens":100000}}}"#,
    )
    .unwrap();
    let record = claude_usage_record_from_event(&event).expect("usage record");
    let mut summary = CostSummary::default();

    add_claude_record_to_summary(&mut summary, &record);

    assert!(summary.total_cost_usd > 0.0);
    assert!(summary.unknown_models.contains("claude-retired-unknown"));
}

#[test]
fn claude_scan_pricing_resolver_reuses_positive_and_negative_resolution() {
    let unknown = format!("claude-scan-unknown-{}", std::process::id());
    let mut resolver = ClaudeScanPricingResolver::default();

    assert!(resolver.is_known("claude-sonnet-4-6"));
    assert!(!resolver.is_known(&unknown));
    assert!(resolver.is_known("claude-sonnet-4-6"));
    assert!(!resolver.is_known(&unknown));
    assert_eq!(resolver.normalization_cache_misses, 2);
    assert_eq!(resolver.resolution_cache_misses, 2);
    assert_eq!(resolver.resolutions.len(), 2);

    let mut cost_resolver = ClaudeScanPricingResolver::default();
    let resolved_unknown =
        cost_resolver.cost_usd_with_cache_ttl(&unknown, None, 100, 20, 10, 30, 40);
    let fallback =
        ClaudePricing::cost_usd_with_cache_ttl(FALLBACK_CLAUDE_MODEL, 100, 20, 10, 30, 40);
    assert!((resolved_unknown - fallback).abs() < f64::EPSILON);
}

#[test]
fn claude_scan_pricing_resolver_preserves_tiered_and_cache_ttl_pricing() {
    let mut resolver = ClaudeScanPricingResolver::default();
    let cases = [
        ("claude-sonnet-4-6", 240_000, 0, 0, 0, 0),
        ("claude-fable-5", 100, 30, 20, 20, 5),
    ];

    for (model, input, cache_create, cache_create_1h, cache_read, output) in cases {
        let actual = resolver.cost_usd_with_cache_ttl(
            model,
            None,
            input,
            cache_create,
            cache_create_1h,
            cache_read,
            output,
        );
        let expected = ClaudePricing::cost_usd_with_cache_ttl(
            model,
            input,
            cache_create,
            cache_create_1h,
            cache_read,
            output,
        );
        assert!((actual - expected).abs() < f64::EPSILON, "{model}");
    }
}

#[test]
fn claude_scan_resolver_applies_gpt_proxy_long_context_boundary() {
    let snapshot = crate::core::ModelsDevPricingSnapshot::from_catalog_json_for_tests(
        r#"{
            "openai": {"models": {"gpt-5.6-sol": {"id": "gpt-5.6-sol", "cost": {
                "input": 2, "output": 4, "cache_read": 0.25, "cache_write": 3,
                "context_over_200k": {"input": 7, "output": 11, "cache_read": 0.5, "cache_write": 9}
            }}}}
        }"#,
    )
    .expect("pricing fixture");
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(snapshot);

    let short = resolver.cost_usd_with_cache_ttl("gpt-5.6-sol", None, 262_000, 0, 0, 10_000, 13);
    let long = resolver.cost_usd_with_cache_ttl("gpt-5.6-sol", None, 262_001, 0, 0, 10_000, 13);

    assert!((short - 0.526552).abs() < 1e-12);
    assert!((long - 1.83915).abs() < 1e-12);
}

#[test]
fn claude_scan_pricing_resolver_bounds_normalization_memo() {
    let mut resolver = ClaudeScanPricingResolver::default();
    for index in 0..(ClaudeScanPricingResolver::MEMO_ENTRY_LIMIT + 8) {
        let model = format!("claude-memo-{index}");
        assert_eq!(resolver.normalize(&model), model);
    }
    assert_eq!(
        resolver.normalized_models.len(),
        ClaudeScanPricingResolver::MEMO_ENTRY_LIMIT
    );

    let misses = resolver.normalization_cache_misses;
    assert_eq!(resolver.normalize("claude-memo-0"), "claude-memo-0");
    assert_eq!(resolver.normalization_cache_misses, misses);
    assert_eq!(resolver.normalize("claude-memo-1024"), "claude-memo-1024");
    assert_eq!(resolver.normalization_cache_misses, misses + 1);
}

#[test]
fn claude_fable_5_prices_five_minute_and_one_hour_cache_writes() {
    // (input, cache write, 1h share of it, cache read, output, expected USD).
    for (input, cache_create, cache_create_1h, cache_read, output, expected) in [
        (
            100,
            10,
            0,
            20,
            5,
            (100.0 / 1_000_000.0) * 10.00
                + (10.0 / 1_000_000.0) * 12.50
                + (20.0 / 1_000_000.0) * 1.00
                + (5.0 / 1_000_000.0) * 50.00,
        ),
        (
            100,
            30,
            20,
            20,
            5,
            (100.0 / 1_000_000.0) * 10.00
                + (10.0 / 1_000_000.0) * 12.50
                + (20.0 / 1_000_000.0) * 20.00
                + (20.0 / 1_000_000.0) * 1.00
                + (5.0 / 1_000_000.0) * 50.00,
        ),
    ] {
        let cost = ClaudePricing::cost_usd_with_cache_ttl(
            "claude-fable-5",
            input,
            cache_create,
            cache_create_1h,
            cache_read,
            output,
        );
        assert!((cost - expected).abs() < f64::EPSILON, "{cache_create_1h}");
    }
}

#[test]
fn claude_mythos_models_price_from_the_builtin_table() {
    // (model, cache read price per million): Mythos 5.1 bills cache reads at $0.25/M,
    // Mythos 5 at $1/M. Both bill $10 in, $12.50 cache write, $50 out per million.
    for (model, cache_read_per_million) in [("claude-mythos-5", 1.00), ("claude-mythos-5-1", 0.25)]
    {
        let cost = ClaudePricing::cost_usd_with_cache_ttl(model, 100, 10, 0, 20, 5);
        let expected = (100.0 / 1_000_000.0) * 10.00
            + (10.0 / 1_000_000.0) * 12.50
            + (20.0 / 1_000_000.0) * cache_read_per_million
            + (5.0 / 1_000_000.0) * 50.00;
        assert!((cost - expected).abs() < 1e-12, "{model}");
    }
}

#[test]
fn claude_scan_pricing_follows_the_canonical_table() {
    // Undated Sonnet 4.6 bills its full context at the standard $3/M: 240k = 0.72.
    // Sonnet 4.5 keeps the 200k tier for the whole request: 240k @ $6/M = 1.44. Opus 4.5-4.8
    // bill $5 in + $25 out; opus-4-8 must resolve through the canonical table.
    // Legacy Opus 4.0/4.1 stay at $15 + $75 (retired IDs absent from the
    // table, e.g. `claude-3-opus-...`, fall back to Sonnet). Haiku 4.5 bills
    // $1 + $5, not the Haiku 3 rate.
    for (model, input, output, expected) in [
        ("claude-sonnet-4-6", 240_000, 0, 0.72),
        ("claude-sonnet-4-5", 240_000, 0, 1.44),
        ("claude-opus-4-5", 1_000_000, 1_000_000, 30.00),
        ("claude-opus-4-6", 1_000_000, 1_000_000, 30.00),
        ("claude-opus-4-7", 1_000_000, 1_000_000, 30.00),
        ("claude-opus-4-8", 1_000_000, 1_000_000, 30.00),
        ("claude-opus-4-20250514", 1_000_000, 1_000_000, 90.00),
        ("claude-opus-4-1", 1_000_000, 1_000_000, 90.00),
        ("claude-haiku-4-5", 1_000_000, 1_000_000, 6.00),
    ] {
        let cost = ClaudePricing::cost_usd_with_cache_ttl(model, input, 0, 0, 0, output);
        assert!(
            (cost - expected).abs() < 0.001,
            "{model} should bill ${expected}, got {cost}"
        );
    }
}

/// Built-in Sonnet 4 and 4.5 rows, per million tokens: base input $3, cache read $0.30, 5-minute
/// cache write $3.75, output $15; above 200k: input $6, cache read $0.60, cache write
/// $7.50, output $22.50. One tier decision covers the whole request (input + cache read +
/// all cache writes > 200k), and the 1h share bills at twice the selected input rate.
#[test]
fn builtin_long_context_tier_is_chosen_for_the_whole_request() {
    // (input, cache write, 1h share of it, cache read, output, expected USD).
    for (input, cache_create, cache_create_1h, cache_read, output, expected) in [
        // 199,999 total stays below: 0.30 + 29,999 * $3.75 + 20k * $6 + 0.015 + 0.15.
        (100_000, 49_999, 20_000, 50_000, 10_000, 0.697_496_25),
        // Exactly 200k is not above the boundary.
        (100_000, 50_000, 20_000, 50_000, 10_000, 0.6975),
        // 210k crosses only because of cache reads and writes; every bucket, output
        // included, takes the tier: 0.60 + 0.375 + 0.036 + 0.225. The old per-bucket
        // split (each count against 200k on its own) gave 0.6555.
        (100_000, 50_000, 0, 60_000, 10_000, 1.236),
        // One-hour writes push the total to 210k: 120k * $6 + 90k * $12. The old path
        // gave 120k * $3 + 90k * $6 = 0.90.
        (120_000, 90_000, 90_000, 0, 0, 1.80),
        // Mixed TTLs above the tier: 0.72 + 60k * $7.50 + 30k * $12 + 1k * $22.50.
        (120_000, 90_000, 30_000, 0, 1_000, 1.5525),
        // Input alone above 200k bills all of it at the tier, not just the excess.
        (240_000, 0, 0, 0, 0, 1.44),
    ] {
        for model in ["claude-sonnet-4-5", "claude-sonnet-4-20250514"] {
            let cost = ClaudePricing::cost_usd_with_cache_ttl(
                model,
                input,
                cache_create,
                cache_create_1h,
                cache_read,
                output,
            );
            assert!(
                (cost - expected).abs() < 1e-9,
                "{model} {input}/{cache_create}/{cache_create_1h}/{cache_read}/{output}: {cost}"
            );
        }
    }
}

#[test]
fn builtin_public_claude_cost_uses_the_whole_request_tier() {
    // 150k + 40k + 20k = 210k: 150k * $6 + 40k * $0.60 + 20k * $7.50 + 1k * $22.50.
    let cost =
        CostUsagePricing::claude_cost_usd("claude-sonnet-4-5", 150_000, 40_000, 20_000, 1_000)
            .expect("built-in pricing");
    assert!((cost - 1.0965).abs() < 1e-9, "{cost}");
}

#[test]
fn builtin_untiered_rows_ignore_the_long_context_boundary() {
    // Opus 4.5 has no tier: 300k * $5 + 100k 1h * $10 + 10k * $25.
    let cost = ClaudePricing::cost_usd_with_cache_ttl(
        "claude-opus-4-5",
        300_000,
        100_000,
        100_000,
        0,
        10_000,
    );
    assert!((cost - 2.75).abs() < 1e-9, "{cost}");
}

fn full_context_cutoff() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_773_360_000, 0).expect("cutoff")
}

/// Opus 4.6 and Sonnet 4.6 bill the 200k tier only for usage dated strictly before
/// 2026-03-13T00:00:00Z (upstream `claudeHistoricalLongContext`). The row is 150k input +
/// 40k cache read + 20k cache write (10k of it one-hour) + 1k output: 210k crosses 200k.
#[test]
fn opus_and_sonnet_4_6_use_the_long_context_tier_only_before_the_cutoff() {
    let before = Some(full_context_cutoff() - chrono::Duration::seconds(1));
    let at = Some(full_context_cutoff());
    let after = Some(full_context_cutoff() + chrono::Duration::days(30));
    // (model, before-cutoff USD, at/after-cutoff and undated USD).
    for (model, tiered, standard) in [
        // Tier: 0.90 + 0.024 + 10k * $7.50 + 10k * $12 + 1k * $22.50.
        // Standard: 0.45 + 0.012 + 10k * $3.75 + 10k * $6 + 1k * $15.
        ("claude-sonnet-4-6", 1.1415, 0.5745),
        // Tier: 1.50 + 0.04 + 10k * $12.50 + 10k * $20 + 1k * $37.50.
        // Standard: 0.75 + 0.02 + 10k * $6.25 + 10k * $10 + 1k * $25.
        ("claude-opus-4-6", 1.9025, 0.9575),
        ("claude-opus-4-6-20260205", 1.9025, 0.9575),
    ] {
        for (date, expected) in [
            (before, tiered),
            (at, standard),
            (after, standard),
            (None, standard),
        ] {
            let cost = ClaudePricing::cost_usd_with_cache_ttl_at(
                model, date, 150_000, 20_000, 10_000, 40_000, 1_000,
            );
            assert!(
                (cost - expected).abs() < 1e-9,
                "{model} at {date:?}: {cost}"
            );
        }
    }
}

#[test]
fn historical_rates_keep_base_pricing_below_the_threshold() {
    let before = Some(full_context_cutoff() - chrono::Duration::seconds(1));
    // 100k + 1k output stays under 200k: 100k * $3 + 1k * $15 on either side of the cutoff.
    for date in [before, Some(full_context_cutoff()), None] {
        let cost = ClaudePricing::cost_usd_with_cache_ttl_at(
            "claude-sonnet-4-6",
            date,
            100_000,
            0,
            0,
            0,
            1_000,
        );
        assert!((cost - 0.315).abs() < 1e-9, "{date:?}: {cost}");
    }
}

#[test]
fn models_without_historical_rates_ignore_the_cutoff() {
    let before = Some(full_context_cutoff() - chrono::Duration::seconds(1));
    for (model, expected) in [
        // Opus 4.5 never had a tier: the standard Opus 4.6 amount on both sides.
        ("claude-opus-4-5", 0.9575),
        // Sonnet 4.5 keeps its current 200k tier after the cutoff too.
        ("claude-sonnet-4-5", 1.1415),
    ] {
        for date in [before, Some(full_context_cutoff()), None] {
            let cost = ClaudePricing::cost_usd_with_cache_ttl_at(
                model, date, 150_000, 20_000, 10_000, 40_000, 1_000,
            );
            assert!(
                (cost - expected).abs() < 1e-9,
                "{model} at {date:?}: {cost}"
            );
        }
    }
}

#[test]
fn scanner_prices_claude_rows_at_their_event_timestamp() {
    let record_cost = |timestamp: Option<&str>| {
        let timestamp = timestamp
            .map(|ts| format!(r#""timestamp":"{ts}","#))
            .unwrap_or_default();
        let event: ClaudeEvent = serde_json::from_str(&format!(
            r#"{{"type":"assistant",{timestamp}"requestId":"req_cutoff","message":{{"id":"msg_cutoff","model":"claude-sonnet-4-6","usage":{{"input_tokens":210000,"output_tokens":0}}}}}}"#
        ))
        .unwrap();
        claude_usage_record_from_event(&event)
            .expect("usage record")
            .cost
            .expect("priced")
    };
    // 210k input: $6/M before the cutoff, $3/M from it and for undated rows.
    assert!((record_cost(Some("2026-03-12T23:59:59Z")) - 1.26).abs() < 1e-9);
    assert!((record_cost(Some("2026-03-13T00:00:00Z")) - 0.63).abs() < 1e-9);
    assert!((record_cost(None) - 0.63).abs() < 1e-9);
}

#[test]
fn dated_public_claude_cost_uses_historical_rates_before_the_cutoff() {
    let before = Some(full_context_cutoff() - chrono::Duration::seconds(1));
    let price = |date| {
        CostUsagePricing::claude_cost_usd_at_date("claude-opus-4-6", date, 210_000, 0, 0, 0)
            .expect("built-in pricing")
    };
    assert!((price(before) - 2.10).abs() < 1e-9);
    assert!((price(Some(full_context_cutoff())) - 1.05).abs() < 1e-9);
    assert!((price(None) - 1.05).abs() < 1e-9);
    let undated = CostUsagePricing::claude_cost_usd("claude-opus-4-6", 210_000, 0, 0, 0).unwrap();
    assert!((undated - 1.05).abs() < 1e-9);
}

/// Models.dev fixture with a 200k long-context tier. Per million tokens: base input $2,
/// cache read $0.25, cache write $3, output $4; above 200k: input $7, cache read $0.5,
/// cache write $9, output $11. The 1h share bills at twice the selected input rate, so it
/// is $4 at base and $14 above the threshold.
fn one_hour_tier_snapshot() -> crate::core::ModelsDevPricingSnapshot {
    crate::core::ModelsDevPricingSnapshot::from_catalog_json_for_tests(
        r#"{
            "anthropic": {"models": {"claude-one-hour-fixture": {"id": "claude-one-hour-fixture", "cost": {
                "input": 2, "output": 4, "cache_read": 0.25, "cache_write": 3,
                "context_over_200k": {"input": 7, "output": 11, "cache_read": 0.5, "cache_write": 9}
            }}}}
        }"#,
    )
    .expect("pricing fixture")
}

#[test]
fn models_dev_one_hour_writes_cross_the_long_context_threshold() {
    // The 5-minute remainder is 120k alone; the 210k total only crosses 200k because the
    // 90k one-hour writes count toward the tier.
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(one_hour_tier_snapshot());
    let cost = resolver.cost_usd_with_cache_ttl(
        "claude-one-hour-fixture",
        None,
        120_000,
        90_000,
        90_000,
        0,
        0,
    );
    // Tiered: 120k * $7 + 90k * (2 * $7) = 0.84 + 1.26 = 2.10.
    // Five-minute-only tiering would give 120k * $2 + 90k * $4 = 0.60.
    assert!((cost - 2.10).abs() < 1e-9, "{cost}");
}

#[test]
fn models_dev_one_hour_share_uses_the_tier_aware_input_rate() {
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(one_hour_tier_snapshot());
    let cost = resolver.cost_usd_with_cache_ttl(
        "claude-one-hour-fixture",
        None,
        120_000,
        90_000,
        30_000,
        0,
        0,
    );
    // Tiered: 120k * $7 + 60k 5-minute * $9 + 30k 1-hour * $14 = 0.84 + 0.54 + 0.42 = 1.80.
    // The old base-rate 1-hour share would give 0.54 for the same row.
    assert!((cost - 1.80).abs() < 1e-9, "{cost}");
}

#[test]
fn models_dev_cache_reads_count_toward_the_one_hour_tier() {
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(one_hour_tier_snapshot());
    let cost = resolver.cost_usd_with_cache_ttl(
        "claude-one-hour-fixture",
        None,
        100_000,
        60_000,
        60_000,
        50_000,
        0,
    );
    // Total 100k + 50k + 60k = 210k, tiered: 0.70 + 50k * $0.5 + 60k * $14 = 1.565.
    assert!((cost - 1.565).abs() < 1e-9, "{cost}");
}

#[test]
fn models_dev_one_hour_writes_below_the_threshold_keep_base_rates() {
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(one_hour_tier_snapshot());
    let cost = resolver.cost_usd_with_cache_ttl(
        "claude-one-hour-fixture",
        None,
        100_000,
        50_000,
        50_000,
        0,
        0,
    );
    // Total 150k stays under 200k: 100k * $2 + 50k * (2 * $2) = 0.20 + 0.20 = 0.40.
    assert!((cost - 0.40).abs() < 1e-9, "{cost}");
}

#[test]
fn models_dev_five_minute_writes_are_unchanged_when_no_one_hour_writes_exist() {
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(one_hour_tier_snapshot());
    let cost =
        resolver.cost_usd_with_cache_ttl("claude-one-hour-fixture", None, 120_000, 90_000, 0, 0, 0);
    // The total crosses 200k with or without the 1h split, so this row was already tiered:
    // 120k * $7 + 90k * $9 = 0.84 + 0.81 = 1.65.
    assert!((cost - 1.65).abs() < 1e-9, "{cost}");
}

#[test]
fn models_dev_gpt_proxy_one_hour_writes_use_the_bundled_boundary() {
    let snapshot = crate::core::ModelsDevPricingSnapshot::from_catalog_json_for_tests(
        r#"{
            "openai": {"models": {"gpt-5.6-sol": {"id": "gpt-5.6-sol", "cost": {
                "input": 2, "output": 4, "cache_read": 0.25, "cache_write": 3,
                "context_over_200k": {"input": 7, "output": 11, "cache_read": 0.5, "cache_write": 9}
            }}}}
        }"#,
    )
    .expect("pricing fixture");
    let mut resolver = ClaudeScanPricingResolver::with_snapshot(snapshot);

    // Bundled OpenAI boundary is 272k. The 5-minute-only total is 262k (no tier), but the
    // 272,001 total with 10,001 one-hour writes crosses it.
    let long =
        resolver.cost_usd_with_cache_ttl("gpt-5.6-sol", None, 262_000, 10_001, 10_001, 0, 13);
    // 262k * $7 + 10,001 * $14 + 13 * $11 = 1.834 + 0.140014 + 0.000143 = 1.974157.
    assert!((long - 1.974_157).abs() < 1e-9, "{long}");
}
