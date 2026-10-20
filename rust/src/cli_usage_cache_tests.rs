use chrono::{Duration, TimeZone, Utc};

use super::*;
use crate::core::RateWindow;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap()
}

fn usage_at(updated_at: DateTime<Utc>) -> UsageSnapshot {
    let mut usage = UsageSnapshot::new(RateWindow::with_details(
        42.0,
        Some(300),
        Some(Utc.with_ymd_and_hms(2026, 10, 20, 15, 0, 0).unwrap()),
        None,
    ))
    .with_secondary(RateWindow::with_details(10.0, Some(10080), None, None));
    usage.updated_at = updated_at;
    usage.account_email = Some("person@example.com".to_string());
    usage.account_organization = Some("Example Org".to_string());
    usage.login_method = Some("Example Plan".to_string());
    usage
}

fn open_gate() -> FallbackGate {
    FallbackGate {
        opted_in: true,
        default_account_fetch: true,
        dpapi_failure_observed: true,
    }
}

#[test]
fn round_trip_keeps_windows_and_drops_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CACHE_FILE_NAME);
    let fetched = now() - Duration::seconds(90);
    let entry = Entry::new(ProviderId::Codex, &usage_at(fetched)).unwrap();
    publish(&path, Some(vec![entry])).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    for secret in ["person@example.com", "Example Org", "Example Plan"] {
        assert!(!raw.contains(secret), "identity leaked into cache: {raw}");
    }

    let cache = CliUsageCache::load(&path).expect("cache loads");
    let hit = cache.hit(ProviderId::Codex, now()).expect("fresh hit");
    assert_eq!(hit.age_seconds, 90);
    assert_eq!(hit.usage.primary.used_percent, 42.0);
    assert_eq!(hit.usage.primary.window_minutes, Some(300));
    assert_eq!(hit.usage.secondary.as_ref().unwrap().used_percent, 10.0);
    assert_eq!(hit.usage.updated_at, fetched);
    assert!(hit.usage.account_email.is_none());
    assert!(cache.hit(ProviderId::Claude, now()).is_none());
}

#[test]
fn spend_descriptions_never_reach_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CACHE_FILE_NAME);
    let fetched = now() - Duration::seconds(60);
    let resets = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
    // Bedrock/OpenRouter shape: quota windows whose reset_description holds a
    // dollar amount ("Monthly spend $X", "$X remaining").
    let mut usage = UsageSnapshot::new(RateWindow::with_details(
        42.0,
        Some(300),
        Some(resets),
        Some("Monthly spend $12.34".to_string()),
    ))
    .with_secondary(RateWindow::with_details(
        10.0,
        Some(10080),
        None,
        Some("$56.78 remaining".to_string()),
    ))
    .with_extra_rate_window(
        "credits",
        "Credits",
        RateWindow::with_details(5.0, None, None, Some("Credits $9.01 left".to_string())),
    );
    usage.updated_at = fetched;
    let entry = Entry::new(ProviderId::OpenRouter, &usage).unwrap();
    publish(&path, Some(vec![entry])).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains('$'), "cost leaked into cache: {raw}");
    assert!(
        !raw.contains("reset_description"),
        "spend wording leaked into cache: {raw}"
    );

    // The reset time survives, and reading the file back re-strips (it has
    // nothing left to strip) without breaking the hit.
    let cache = CliUsageCache::load(&path).expect("cache loads");
    let hit = cache.hit(ProviderId::OpenRouter, now()).expect("fresh hit");
    assert_eq!(hit.usage.primary.resets_at, Some(resets));
    assert_eq!(hit.usage.primary.used_percent, 42.0);
    assert!(hit.usage.primary.reset_description.is_none());
    assert!(
        hit.usage
            .secondary
            .as_ref()
            .unwrap()
            .reset_description
            .is_none()
    );
    assert!(
        hit.usage.extra_rate_windows[0]
            .window
            .reset_description
            .is_none()
    );
}

#[test]
fn file_shape_is_stable() {
    let fetched = Utc.with_ymd_and_hms(2026, 10, 20, 11, 58, 30).unwrap();
    let mut usage = UsageSnapshot::new(RateWindow::with_details(42.0, Some(300), None, None));
    usage.updated_at = fetched;
    let cache = CliUsageCache::new(vec![Entry::new(ProviderId::Claude, &usage).unwrap()]);
    assert_eq!(
        serde_json::to_value(&cache).unwrap(),
        serde_json::json!({
            "version": 1,
            "entries": [{
                "provider": "claude",
                "usage": {
                    "primary": {
                        "used_percent": 42.0,
                        "window_minutes": 300,
                        "is_informational": false,
                    },
                    "updated_at": "2026-10-20T11:58:30Z",
                },
            }],
        })
    );
}

#[test]
fn informational_only_snapshots_are_not_shared() {
    let usage = UsageSnapshot::new(RateWindow::informational("No quota"));
    assert!(Entry::new(ProviderId::Codex, &usage).is_none());
}

#[test]
fn stale_and_future_dated_entries_are_ignored() {
    let at_limit = now() - Duration::seconds(MAXIMUM_AGE_SECONDS);
    let stale = now() - Duration::seconds(MAXIMUM_AGE_SECONDS + 1);
    let future = now() + Duration::seconds(5);
    for (fetched, fresh) in [(at_limit, true), (stale, false), (future, false)] {
        let cache = CliUsageCache::new(vec![
            Entry::new(ProviderId::Codex, &usage_at(fetched)).unwrap(),
        ]);
        assert_eq!(
            cache.hit(ProviderId::Codex, now()).is_some(),
            fresh,
            "fetched at {fetched}"
        );
    }
}

#[test]
fn corrupt_unsupported_and_non_file_caches_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("corrupt.json");
    std::fs::write(&corrupt, b"{not json").unwrap();
    assert!(CliUsageCache::load(&corrupt).is_none());

    let future_version = dir.path().join("v2.json");
    std::fs::write(&future_version, br#"{"version":2,"entries":[]}"#).unwrap();
    assert!(CliUsageCache::load(&future_version).is_none());

    let directory = dir.path().join("dir.json");
    std::fs::create_dir(&directory).unwrap();
    assert!(CliUsageCache::load(&directory).is_none());

    assert!(CliUsageCache::load(&dir.path().join("missing.json")).is_none());
}

#[test]
fn unknown_providers_in_the_file_do_not_break_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CACHE_FILE_NAME);
    let mut cache = CliUsageCache::new(vec![
        Entry::new(ProviderId::Codex, &usage_at(now())).unwrap(),
    ]);
    cache.entries.insert(
        0,
        Entry {
            provider: "provider-from-a-newer-app".to_string(),
            usage: usage_at(now()),
        },
    );
    cache.write(&path).unwrap();
    let loaded = CliUsageCache::load(&path).unwrap();
    assert!(loaded.hit(ProviderId::Codex, now()).is_some());
}

#[test]
fn withdrawing_removes_the_file_and_tolerates_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CACHE_FILE_NAME);
    publish(
        &path,
        Some(vec![
            Entry::new(ProviderId::Codex, &usage_at(now())).unwrap(),
        ]),
    )
    .unwrap();
    assert!(path.is_file());
    publish(&path, None).unwrap();
    assert!(!path.exists());
    publish(&path, None).unwrap();
}

#[test]
fn only_credential_access_failures_qualify() {
    assert!(is_credential_access_failure(&ProviderError::AuthRequired));
    assert!(is_credential_access_failure(&ProviderError::NoCookies));
    assert!(is_credential_access_failure(
        &ProviderError::OwnedTransport {
            owner: None,
            source: Box::new(ProviderError::NoCookies),
        }
    ));
    for other in [
        ProviderError::Timeout,
        ProviderError::Parse("bad body".to_string()),
        ProviderError::Other("API error 500".to_string()),
        ProviderError::OAuthExpired("expired".to_string()),
        ProviderError::OAuthRevoked("revoked".to_string()),
        ProviderError::OAuth("rejected".to_string()),
        ProviderError::NotInstalled("codex".to_string()),
        ProviderError::BrowserSignInRequired {
            message: "Sign in".to_string(),
            sign_in_url: "https://example.com/login".to_string(),
        },
    ] {
        assert!(!is_credential_access_failure(&other), "{other:?}");
    }
}

#[test]
fn fallback_needs_every_gate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CACHE_FILE_NAME);
    publish(
        &path,
        Some(vec![
            Entry::new(ProviderId::Codex, &usage_at(now() - Duration::seconds(30))).unwrap(),
        ]),
    )
    .unwrap();
    let error = ProviderError::AuthRequired;
    let hit = |gate: FallbackGate, error: &ProviderError| {
        fallback_hit(gate, ProviderId::Codex, error, Some(&path), now())
    };

    assert_eq!(hit(open_gate(), &error).unwrap().age_seconds, 30);
    assert!(
        hit(
            FallbackGate {
                opted_in: false,
                ..open_gate()
            },
            &error
        )
        .is_none()
    );
    assert!(
        hit(
            FallbackGate {
                default_account_fetch: false,
                ..open_gate()
            },
            &error
        )
        .is_none()
    );
    assert!(
        hit(
            FallbackGate {
                dpapi_failure_observed: false,
                ..open_gate()
            },
            &error
        )
        .is_none()
    );
    assert!(hit(open_gate(), &ProviderError::Timeout).is_none());
    assert!(
        fallback_hit(
            open_gate(),
            ProviderId::Codex,
            &error,
            Some(&dir.path().join("missing.json")),
            now()
        )
        .is_none()
    );
    assert!(fallback_hit(open_gate(), ProviderId::Codex, &error, None, now()).is_none());
}

#[test]
fn opt_in_follows_settings_unless_they_are_unreadable() {
    assert!(cli_opted_in(true, true));
    assert!(!cli_opted_in(true, false));
    assert!(cli_opted_in(false, false));
}
