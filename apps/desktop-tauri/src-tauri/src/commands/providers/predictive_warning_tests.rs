use super::*;

fn limit_reset_snapshot() -> ProviderUsageSnapshot {
    let metadata = codexbar::core::instantiate_provider(ProviderId::Codex)
        .metadata()
        .clone();
    let result = codexbar::core::ProviderFetchResult {
        usage: codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(10.0)),
        cost: None,
        wayfinder_usage: None,
        open_ai_api_usage: None,
        throughput: None,
        inventory: Vec::new(),
        reset_credits: None,
        display_details: Vec::new(),
        source_label: "OAuth".to_string(),
        has_successful_claude_cli_quota: false,
        pace_authoritative: true,
        quota_estimated: false,
        account_identity: None,
        last_good_owner: None,
    };
    let mut snapshot =
        ProviderUsageSnapshot::from_fetch_result(ProviderId::Codex, &metadata, &result, None, None);
    snapshot.updated_at = "2026-05-01T12:00:00Z".to_string();
    snapshot.account_email = Some("owner@example.com".to_string());
    snapshot.primary.window_minutes = Some(300);
    snapshot.primary.resets_at = Some("2026-05-01T15:00:00Z".to_string());
    let mut weekly = snapshot.primary.clone();
    weekly.used_percent = 40.0;
    weekly.window_minutes = Some(10080);
    weekly.resets_at = Some("2026-05-07T00:00:00Z".to_string());
    snapshot.secondary = Some(weekly);
    snapshot
}

fn limit_reset_identity(
    snapshot: &ProviderUsageSnapshot,
    token_account_id: Option<uuid::Uuid>,
) -> super::super::warning_identity::WarningIdentity {
    super::super::warning_identity::WarningIdentity::new(
        ProviderId::Codex,
        &snapshot.source_label,
        snapshot.account_email.as_deref(),
        snapshot.account_organization.as_deref(),
        token_account_id,
    )
}

fn empty_snapshot() -> ProviderUsageSnapshot {
    let metadata = codexbar::core::instantiate_provider(ProviderId::Claude)
        .metadata()
        .clone();
    ProviderUsageSnapshot::from_error(
        ProviderId::Claude,
        &metadata,
        "unused".to_string(),
        codexbar::core::ProviderStateKind::Unknown,
    )
}

#[test]
fn quota_notification_account_identity_prefers_token_then_email() {
    let account_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let mut snapshot = empty_snapshot();
    snapshot.account_email = Some("Person@Example.com".to_string());
    snapshot.account_organization = Some("Acme Org".to_string());
    snapshot.plan_name = Some("Pro".to_string());

    assert_eq!(
        quota_notification_account_identity(&snapshot, Some(account_id)),
        "token-account:aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
    );
    assert_eq!(
        quota_notification_account_identity(&snapshot, None),
        "person@example.com"
    );

    snapshot.account_email = None;
    assert_eq!(
        quota_notification_account_identity(&snapshot, None),
        "org:acme org"
    );

    snapshot.account_organization = None;
    snapshot.source_label = "oauth".to_string();
    assert_eq!(
        quota_notification_account_identity(&snapshot, None),
        "claude:oauth:unknown"
    );

    snapshot.plan_name = None;
    snapshot.source_label = "cli (reduced fidelity)".to_string();
    assert_eq!(
        quota_notification_account_identity(&snapshot, None),
        "claude:cli:unknown"
    );
}

/// The forecast scope key and the notification identity must never disagree.
/// If they did, one account would be seen as two identities and its burn history
/// would be split, silently halving the sample count behind every forecast.
#[test]
fn forecast_account_key_matches_notification_identity() {
    use crate::commands::bridge::forecast_account_key;

    let token = uuid::Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let mut usage = codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(1.0));
    let mut snapshot = empty_snapshot();

    for (email, org) in [
        (Some("Person@Example.com"), Some("Acme Org")),
        (Some("Person@Example.com"), None),
        (None, Some("Acme Org")),
        (None, None),
    ] {
        usage.account_email = email.map(str::to_string);
        usage.account_organization = org.map(str::to_string);
        snapshot.account_email = usage.account_email.clone();
        snapshot.account_organization = usage.account_organization.clone();

        for tok in [Some(token), None] {
            assert_eq!(
                forecast_account_key(&usage, tok).unwrap_or_default(),
                quota_notification_account_identity(&snapshot, tok),
                "identity drift for email={email:?} org={org:?} token={tok:?}"
            );
        }
    }
}

#[test]
fn provider_timeout_error_message_is_plain_timeout() {
    // The elapsed-timeout path reuses the provider-error path, which
    // redacts the message; the user must still see "Timeout".
    assert_eq!(
        codexbar::logging::safe_error_message(codexbar::core::ProviderError::Timeout),
        "Timeout"
    );
}

/// Codex OAuth now publishes the auth.json email. Ambient Codex moves from
/// the unscoped key (process-local history only) to the lowercased email;
/// a managed token account still wins, and the header-only token-account
/// label never feeds any scoping key.
#[test]
fn codex_email_scopes_forecast_and_warnings_but_label_does_not() {
    use crate::commands::bridge::forecast_account_key;

    let token = uuid::Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
    let mut usage = codexbar::core::UsageSnapshot::new(codexbar::core::RateWindow::new(1.0));
    let mut snapshot = empty_snapshot();
    snapshot.provider_id = "codex".to_string();

    assert_eq!(forecast_account_key(&usage, None), None);
    assert_eq!(quota_notification_account_identity(&snapshot, None), "");

    snapshot.account_label = Some("Work account".to_string());
    assert_eq!(quota_notification_account_identity(&snapshot, None), "");

    usage.account_email = Some("Parity.User@Example.com".to_string());
    snapshot.account_email = usage.account_email.clone();
    assert_eq!(
        forecast_account_key(&usage, None).as_deref(),
        Some("parity.user@example.com")
    );
    assert_eq!(
        quota_notification_account_identity(&snapshot, None),
        "parity.user@example.com"
    );
    assert_eq!(
        forecast_account_key(&usage, Some(token)).as_deref(),
        Some("token-account:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
    );
}

#[test]
fn header_label_trims_and_drops_blank_labels() {
    assert_eq!(
        header_label("  parity-github "),
        Some("parity-github".to_string())
    );
    assert_eq!(header_label("   "), None);
}

#[test]
fn limit_reset_sample_observes_session_and_weekly_lanes() {
    let snapshot = limit_reset_snapshot();
    let identity = limit_reset_identity(&snapshot, None);
    let sample = super::providers::limit_reset_sample(
        ProviderId::Codex,
        &snapshot,
        &identity,
        None,
        "owner@example.com",
        &Settings::default(),
    );

    let observed_at = chrono::DateTime::parse_from_rfc3339("2026-05-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let session = sample.session.expect("session observed");
    assert_eq!(session.used_percent, 10.0);
    assert_eq!(session.observed_at, observed_at);
    assert_eq!(
        session.reset_boundary,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-05-01T15:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc)
        )
    );
    assert_eq!(sample.weekly.expect("weekly observed").used_percent, 40.0);
    assert_eq!(sample.session_transition_account, "owner@example.com");
    let account = sample.account.expect("owner account");
    assert_eq!(account.key, "owner@example.com");
    assert_eq!(account.label.as_deref(), Some("owner@example.com"));
}

#[test]
fn limit_reset_sample_prefers_token_label_and_hides_personal_info() {
    let snapshot = limit_reset_snapshot();
    let identity = limit_reset_identity(&snapshot, Some(uuid::Uuid::nil()));
    let label = |settings: &Settings| {
        super::providers::limit_reset_sample(
            ProviderId::Codex,
            &snapshot,
            &identity,
            Some("Work"),
            "",
            settings,
        )
        .account
        .and_then(|account| account.label)
    };
    assert_eq!(label(&Settings::default()).as_deref(), Some("Work"));
    let hidden = Settings {
        hide_personal_info: true,
        ..Settings::default()
    };
    assert_eq!(label(&hidden), None);
}

#[test]
fn limit_reset_sample_skips_non_reset_lanes() {
    let mut snapshot = limit_reset_snapshot();
    snapshot.primary.window_minutes = Some(1440);
    if let Some(weekly) = snapshot.secondary.as_mut() {
        weekly.is_informational = true;
    }
    let identity = limit_reset_identity(&snapshot, None);
    let sample = super::providers::limit_reset_sample(
        ProviderId::Codex,
        &snapshot,
        &identity,
        None,
        "",
        &Settings::default(),
    );
    assert!(sample.session.is_none());
    assert!(sample.weekly.is_none());

    let mut snapshot = limit_reset_snapshot();
    snapshot.updated_at = "not a date".to_string();
    let identity = limit_reset_identity(&snapshot, None);
    let sample = super::providers::limit_reset_sample(
        ProviderId::Codex,
        &snapshot,
        &identity,
        None,
        "",
        &Settings::default(),
    );
    assert!(sample.session.is_none());
    assert!(sample.weekly.is_none());
}
