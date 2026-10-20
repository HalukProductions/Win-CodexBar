use codexbar::core::{ProviderFetchResult, ProviderId, instantiate_provider};
use codexbar::settings::Settings;

use super::super::ProviderUsageSnapshot;
use super::super::bridge::{
    NamedRateWindowSnapshot, ProviderInventoryItemSnapshot, SubscriptionMetadataSnapshot,
};
use super::super::usage_items::*;

fn claude_snapshot_with_sections(source_label: &str) -> ProviderUsageSnapshot {
    let metadata = instantiate_provider(ProviderId::Claude).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0)),
        cost: Some(codexbar::core::CostSnapshot::new(0.0, "USD", "Extra usage").with_balance(5.0)),
        wayfinder_usage: None,
        open_ai_api_usage: None,
        inventory: Vec::new(),
        reset_credits: None,
        display_details: Vec::new(),
        source_label: source_label.to_string(),
        has_successful_claude_cli_quota: false,
        pace_authoritative: true,
        quota_estimated: false,
        account_identity: None,
        last_good_owner: None,
        throughput: None,
    };
    let mut snapshot = ProviderUsageSnapshot::from_fetch_result(
        ProviderId::Claude,
        &metadata,
        &result,
        None,
        None,
    );
    snapshot.inventory = vec![ProviderInventoryItemSnapshot {
        id: "reset-credits".to_string(),
        title: "Limit Reset Credits".to_string(),
        available_count: 2,
        next_expires_at: None,
    }];
    snapshot.subscription = Some(SubscriptionMetadataSnapshot {
        starts_at: None,
        expires_at: None,
        renews_at: Some("2026-12-01T00:00:00+00:00".to_string()),
        renews_at_is_date_only: true,
        expires_at_is_date_only: false,
    });
    snapshot
}

#[test]
fn usage_item_descriptors_keep_hidden_claude_sections_restorable() {
    let mut settings = Settings::default();
    settings.set_hidden_usage_item_ids(
        ProviderId::Claude,
        vec![
            "section:claude-reset-credits".to_string(),
            "section:unknown".to_string(),
        ],
    );
    let items = usage_item_descriptors(None, &settings, ProviderId::Claude);
    let ids: Vec<(&str, &str, bool)> = items
        .iter()
        .map(|item| (item.id.as_str(), item.title.as_str(), item.available))
        .collect();
    assert_eq!(
        ids,
        [(
            "section:claude-reset-credits",
            "Limit reset credits (unavailable)",
            false
        )]
    );
    // Section IDs never become "metric:section:..." placeholders.
    assert!(!items.iter().any(|item| item.id.starts_with("metric:")));
}

#[test]
fn usage_item_descriptors_offer_independent_claude_sections() {
    let snapshot = claude_snapshot_with_sections("web");
    let items = usage_item_descriptors(Some(&snapshot), &Settings::default(), ProviderId::Claude);
    let sections: Vec<(&str, &str, bool, bool)> = items
        .iter()
        .filter(|item| item.id.starts_with("section:"))
        .map(|item| {
            (
                item.id.as_str(),
                item.title.as_str(),
                item.available,
                item.requires_cost_summary,
            )
        })
        .collect();
    assert_eq!(
        sections,
        [
            ("section:subscription", "Plan renewal / expiry", true, false),
            (
                "section:claude-reset-credits",
                "Limit reset credits",
                true,
                false
            ),
            ("section:claude-extra-usage", "Extra usage", true, true),
        ]
    );

    // Admin API spend is not Extra usage and has no Extra usage choice.
    let admin = claude_snapshot_with_sections("admin-api");
    let items = usage_item_descriptors(Some(&admin), &Settings::default(), ProviderId::Claude);
    assert!(
        !items
            .iter()
            .any(|item| item.id == "section:claude-extra-usage")
    );

    // The same sections are never offered for another provider.
    let items = usage_item_descriptors(Some(&snapshot), &Settings::default(), ProviderId::Codex);
    assert!(!items.iter().any(|item| item.id.starts_with("section:")));
}

#[test]
fn usage_item_descriptors_skip_card_hidden_windows() {
    let metadata = instantiate_provider(ProviderId::Codex).metadata().clone();
    let result = ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(
            codexbar::core::RateWindow::informational("$1.00").hidden_on_card(),
        )
        .with_secondary(codexbar::core::RateWindow::new(20.0)),
        cost: None,
        wayfinder_usage: None,
        inventory: Vec::new(),
        reset_credits: None,
        display_details: vec![
            codexbar::core::ProviderDisplayDetail::new("spend", "Spend", "$1.00")
                .expect("valid detail"),
        ],
        source_label: "api".to_string(),
        has_successful_claude_cli_quota: false,
        pace_authoritative: true,
        quota_estimated: false,
        account_identity: None,
        open_ai_api_usage: None,
        throughput: None,
        last_good_owner: None,
    };
    let mut snapshot =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Codex, &metadata, &result, None, None);
    let mut hidden_extra = snapshot.primary.clone();
    hidden_extra.hidden_on_card = true;
    snapshot.extra_rate_windows = vec![NamedRateWindowSnapshot {
        id: "hidden".to_string(),
        title: "Hidden".to_string(),
        window: hidden_extra,
        fallback_lane: false,
        icon_fallback: None,
    }];

    let items = usage_item_descriptors(Some(&snapshot), &Settings::default(), ProviderId::Codex);
    let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();

    assert_eq!(ids, ["detailSection:Spend", "metric:secondary"]);

    // A saved hidden ID for a card-hidden window gets no placeholder, while
    // one for a window the snapshot no longer reports still does.
    let mut settings = Settings::default();
    settings.set_hidden_usage_item_ids(
        ProviderId::Codex,
        vec![
            "metric:primary".to_string(),
            "metric:extra-hidden".to_string(),
            "metric:tertiary".to_string(),
        ],
    );
    let items = usage_item_descriptors(Some(&snapshot), &settings, ProviderId::Codex);
    let ids: Vec<(&str, bool)> = items
        .iter()
        .map(|item| (item.id.as_str(), item.available))
        .collect();
    assert_eq!(
        ids,
        [
            ("detailSection:Spend", true),
            ("metric:secondary", true),
            ("metric:tertiary", false),
        ]
    );
}
