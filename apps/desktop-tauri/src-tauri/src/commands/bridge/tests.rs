use super::*;

fn snapshot_window_with(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
    reset_description: Option<String>,
) -> RateWindowSnapshot {
    RateWindowSnapshot {
        used_percent,
        remaining_percent: 100.0 - used_percent,
        window_minutes,
        resets_at: resets_at.map(|dt| dt.to_rfc3339()),
        reset_description,
        ..Default::default()
    }
}

#[test]
fn tray_status_prefers_relative_reset_countdown() {
    let window = snapshot_window_with(
        13.0,
        Some(300),
        Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
        Some("Jun 10 at 3:00PM".to_string()),
    );

    let label = compact_tray_status_label(&window, Language::English, true);

    assert!(label.starts_with("13% used • Resets in 2h "));
    assert!(label.ends_with('m'));
    assert!(!label.contains("Jun 10"));
}

#[test]
fn tray_status_normalizes_fallback_reset_description() {
    let window = snapshot_window_with(8.0, Some(300), None, Some("2h 05m".to_string()));

    assert_eq!(
        compact_tray_status_label(&window, Language::English, true),
        "8% used • Resets in 2h 05m"
    );
}

#[test]
fn credit_balance_detail_crosses_the_bridge_and_is_not_a_reset_phrase() {
    let rw = RateWindow::with_details(25.0, None, None, Some("750 / 1000 credits left".into()))
        .with_description_as_detail();
    let window = RateWindowSnapshot::from_rate_window(&rw);

    assert!(window.description_is_detail);
    assert_eq!(
        window.reset_description.as_deref(),
        Some("750 / 1000 credits left")
    );
    let json = serde_json::to_value(&window).unwrap();
    assert_eq!(json["descriptionIsDetail"], true);
    assert_eq!(
        compact_tray_status_label(&window, Language::English, true),
        "25% used"
    );

    let plain = RateWindowSnapshot::from_rate_window(&RateWindow::new(10.0));
    let json = serde_json::to_value(&plain).unwrap();
    assert_eq!(json["descriptionIsDetail"], false);
}

#[test]
fn japanese_tray_status_label_has_no_english_reset_text() {
    use codexbar::settings::Language;

    let window = snapshot_window_with(
        13.0,
        Some(300),
        Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
        None,
    );

    let label = compact_tray_status_label(&window, Language::Japanese, true);

    assert!(label.contains("リセットまで"), "{label}");
    assert!(!label.to_ascii_lowercase().contains("resets in"), "{label}");
    assert!(label.contains("13%"), "{label}");
}

#[test]
fn japanese_tray_status_strips_english_fallback_reset_prefix() {
    use codexbar::settings::Language;

    let window = snapshot_window_with(8.0, Some(300), None, Some("Resets in 2h 05m".to_string()));

    let label = compact_tray_status_label(&window, Language::Japanese, true);

    assert!(label.contains("リセットまで"), "{label}");
    assert!(!label.to_ascii_lowercase().contains("resets in"), "{label}");
    assert!(label.contains("2時間 05分"), "{label}");
}

#[test]
fn tray_status_label_names_remaining_or_used_percent() {
    let window = snapshot_window_with(13.0, Some(300), None, None);

    assert_eq!(
        compact_tray_status_label(&window, Language::English, false),
        "87% left"
    );
    assert_eq!(
        compact_tray_status_label(&window, Language::English, true),
        "13% used"
    );
    assert_eq!(
        compact_tray_status_label(&window, Language::Japanese, false),
        "87% 残り"
    );

    let exhausted = RateWindowSnapshot {
        is_exhausted: true,
        ..snapshot_window_with(100.0, Some(300), None, None)
    };
    assert_eq!(
        compact_tray_status_label(&exhausted, Language::English, false),
        "0% left"
    );
}

#[test]
fn tray_status_label_relocalizes_without_refetch() {
    let window = snapshot_window_with(
        13.0,
        Some(300),
        Some(chrono::Utc::now() + chrono::Duration::minutes(125)),
        None,
    );

    let english = compact_tray_status_label(&window, Language::English, true);
    let japanese = compact_tray_status_label(&window, Language::Japanese, true);

    assert!(english.contains("Resets in"), "{english}");
    assert!(japanese.contains("リセットまで"), "{japanese}");
    assert!(
        !japanese.to_ascii_lowercase().contains("resets in"),
        "{japanese}"
    );
}

#[test]
fn secondary_label_key_crosses_the_bridge_only_when_the_provider_sets_it() {
    let now = chrono::Utc::now();
    let monthly = || {
        RateWindow::with_details(
            25.0,
            Some(43_200),
            Some(now + chrono::Duration::days(9)),
            None,
        )
    };
    let bridge = |id: ProviderId, usage: codexbar::core::UsageSnapshot| {
        let metadata = codexbar::core::instantiate_provider(id).metadata().clone();
        let result = ProviderFetchResult::new(usage, "web");
        serde_json::to_value(ProviderUsageSnapshot::from_fetch_result(
            id, &metadata, &result, None, None,
        ))
        .unwrap()
    };

    let notion = bridge(
        ProviderId::Notion,
        codexbar::core::UsageSnapshot::new(monthly())
            .with_secondary(monthly())
            .with_secondary_label_key(codexbar::locale::LocaleKey::ProviderLabelMonthly),
    );
    assert_eq!(notion["secondaryLabelKey"], "ProviderLabelMonthly");
    assert_eq!(notion["secondaryLabel"], "Monthly");

    let augment = bridge(
        ProviderId::Augment,
        codexbar::core::UsageSnapshot::new(monthly()).with_secondary(monthly()),
    );
    assert!(augment.get("secondaryLabelKey").is_none());
    assert_eq!(augment["secondaryLabel"], "Monthly");
}

#[test]
fn provider_card_wording_crosses_the_bridge() {
    let regen =
        RateWindowSnapshot::from_rate_window(&RateWindow::new(10.0).with_regenerating_reset());
    let hidden = RateWindowSnapshot::from_rate_window(
        &RateWindow::informational("Balance: $1.00").hidden_on_card(),
    );
    let regen_json = serde_json::to_value(&regen).unwrap();
    let hidden_json = serde_json::to_value(&hidden).unwrap();
    assert_eq!(regen_json["resetRegenerates"], true);
    assert!(regen_json.get("hiddenOnCard").is_none());
    assert_eq!(hidden_json["hiddenOnCard"], true);
    assert!(hidden_json.get("resetRegenerates").is_none());

    let metadata = codexbar::core::instantiate_provider(ProviderId::LithosAI)
        .metadata()
        .clone();
    let cost = codexbar::core::CostSnapshot::new(0.0, "USD", "Prepaid credits")
        .with_balance(4.0)
        .with_card_title(codexbar::locale::LocaleKey::CreditsTitle);
    let result = codexbar::core::ProviderFetchResult::new(
        codexbar::core::UsageSnapshot::new(RateWindow::new(0.0)),
        "web",
    )
    .with_cost(cost);
    let snapshot = ProviderUsageSnapshot::from_fetch_result(
        ProviderId::LithosAI,
        &metadata,
        &result,
        None,
        None,
    );
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["cost"]["titleKey"], "CreditsTitle");

    let plain = codexbar::core::ProviderFetchResult::new(
        codexbar::core::UsageSnapshot::new(RateWindow::new(0.0)),
        "web",
    )
    .with_cost(codexbar::core::CostSnapshot::new(1.0, "USD", "This month"));
    let json = serde_json::to_value(ProviderUsageSnapshot::from_fetch_result(
        ProviderId::LithosAI,
        &metadata,
        &plain,
        None,
        None,
    ))
    .unwrap();
    assert!(json["cost"].get("titleKey").is_none());
}
