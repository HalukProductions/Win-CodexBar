use crate::state::AppState;
use codexbar::core::ProviderId;

use super::PROVIDER_CACHE_STALE_AFTER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderRefreshOutcome {
    Skipped {
        reason: ProviderRefreshSkipReason,
    },
    Published {
        generation: u64,
    },
    Superseded {
        generation: u64,
        current_generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderRefreshSkipReason {
    NoEnabledProviders,
    Active { generation: u64 },
    InputSuperseded { expected: u64, current: u64 },
    CacheFresh { generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProviderRefreshReservation {
    Reserved { generation: u64 },
    Skipped(ProviderRefreshSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProviderRefreshCompletion {
    Published { error_count: usize },
    Superseded { current_generation: u64 },
}

pub(crate) fn is_provider_cache_fresh(
    updated_at: Option<std::time::Instant>,
    stale_after: std::time::Duration,
) -> bool {
    updated_at
        .map(|updated| updated.elapsed() <= stale_after)
        .unwrap_or(false)
}

pub(super) fn reserve_provider_refresh(
    state: &mut AppState,
    force: bool,
    provider_ids: &[ProviderId],
    expected_generation: u64,
) -> ProviderRefreshReservation {
    if state.is_refreshing {
        return ProviderRefreshReservation::Skipped(ProviderRefreshSkipReason::Active {
            generation: state.provider_refresh_generation,
        });
    }
    if state.provider_refresh_generation != expected_generation {
        return ProviderRefreshReservation::Skipped(ProviderRefreshSkipReason::InputSuperseded {
            expected: expected_generation,
            current: state.provider_refresh_generation,
        });
    }
    if provider_cache_can_skip_refresh(state, force, provider_ids) {
        return ProviderRefreshReservation::Skipped(ProviderRefreshSkipReason::CacheFresh {
            generation: state.provider_refresh_generation,
        });
    }

    if force {
        // An explicit refresh replaces seeded snapshots with live data, so
        // later background refreshes must keep that data current.
        state
            .seeded_provider_ids
            .retain(|id| !provider_ids.contains(id));
    }
    state.provider_refresh_generation = state.provider_refresh_generation.wrapping_add(1);
    let generation = state.provider_refresh_generation;
    state.is_refreshing = true;
    state.provider_refresh_started_at = Some(std::time::Instant::now());
    ProviderRefreshReservation::Reserved { generation }
}

/// Providers a refresh should fetch. A non-forced refresh skips providers
/// whose proof-seeded snapshot is still cached, so seeding one provider does
/// not leave the others unrefreshed or the seed replaced by a live fetch.
pub(super) fn unpinned_refresh_ids(
    state: &AppState,
    force: bool,
    provider_ids: &[ProviderId],
) -> Vec<ProviderId> {
    if force {
        return provider_ids.to_vec();
    }
    provider_ids
        .iter()
        .copied()
        .filter(|id| !is_pinned_seed(state, *id))
        .collect()
}

fn is_pinned_seed(state: &AppState, id: ProviderId) -> bool {
    state.seeded_provider_ids.contains(&id)
        && state
            .provider_cache
            .iter()
            .any(|snapshot| snapshot.provider_id == id.cli_name())
}

fn provider_cache_can_skip_refresh(
    state: &AppState,
    force: bool,
    provider_ids: &[ProviderId],
) -> bool {
    let cache_has_all = provider_ids.iter().all(|id| {
        state
            .provider_cache
            .iter()
            .any(|snapshot| snapshot.provider_id == id.cli_name())
    });
    !force
        && cache_has_all
        && provider_ids.iter().all(|id| {
            is_provider_cache_fresh(
                state.provider_cache_updated_at_by_provider.get(id).copied(),
                PROVIDER_CACHE_STALE_AFTER,
            )
        })
}

pub(super) fn complete_provider_refresh(
    state: &mut AppState,
    generation: u64,
) -> ProviderRefreshCompletion {
    if state.provider_refresh_generation != generation {
        return ProviderRefreshCompletion::Superseded {
            current_generation: state.provider_refresh_generation,
        };
    }
    state.is_refreshing = false;
    state.provider_refresh_started_at = None;
    state.provider_cache_updated_at = Some(std::time::Instant::now());
    ProviderRefreshCompletion::Published {
        error_count: state
            .provider_cache
            .iter()
            .filter(|snapshot| snapshot.error.is_some())
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_codex_seed() -> AppState {
        let mut state = AppState::new();
        state.provider_cache.push(
            crate::proof_harness::parse_seed_usage_snapshot(
                r#"{"providerId":"codex","primary":{"usedPercent":25.0}}"#,
            )
            .unwrap(),
        );
        state.seeded_provider_ids.insert(ProviderId::Codex);
        state
    }

    #[test]
    fn seeded_providers_are_pinned_per_provider() {
        let state = state_with_codex_seed();
        let enabled = [ProviderId::Codex, ProviderId::Claude];
        assert_eq!(
            unpinned_refresh_ids(&state, false, &enabled),
            vec![ProviderId::Claude]
        );
        assert_eq!(
            unpinned_refresh_ids(&state, false, &[ProviderId::Codex]),
            Vec::<ProviderId>::new()
        );
        // A forced refresh fetches every requested provider.
        assert_eq!(unpinned_refresh_ids(&state, true, &enabled), enabled);

        // An unseeded cache entry is not pinned.
        let mut unseeded = state_with_codex_seed();
        unseeded.seeded_provider_ids.clear();
        assert_eq!(
            unpinned_refresh_ids(&unseeded, false, &enabled),
            enabled.to_vec()
        );

        // A seed whose snapshot left the cache is no longer pinned.
        let mut evicted = state_with_codex_seed();
        evicted.provider_cache.clear();
        assert_eq!(
            unpinned_refresh_ids(&evicted, false, &enabled),
            enabled.to_vec()
        );
    }

    #[test]
    fn fully_seeded_refresh_skips_as_cache_fresh() {
        let mut state = state_with_codex_seed();
        let generation = state.provider_refresh_generation;
        let ids = unpinned_refresh_ids(&state, false, &[ProviderId::Codex]);
        assert_eq!(
            reserve_provider_refresh(&mut state, false, &ids, generation),
            ProviderRefreshReservation::Skipped(ProviderRefreshSkipReason::CacheFresh {
                generation
            })
        );
        assert!(!state.is_refreshing);
    }

    #[test]
    fn partially_seeded_refresh_reserves_only_unseeded_providers() {
        let mut state = state_with_codex_seed();
        let generation = state.provider_refresh_generation;
        let ids = unpinned_refresh_ids(&state, false, &[ProviderId::Codex, ProviderId::Claude]);
        assert_eq!(ids, vec![ProviderId::Claude]);
        assert!(matches!(
            reserve_provider_refresh(&mut state, false, &ids, generation),
            ProviderRefreshReservation::Reserved { .. }
        ));
        assert!(state.seeded_provider_ids.contains(&ProviderId::Codex));
    }

    #[test]
    fn forced_refresh_unpins_the_providers_it_fetches() {
        let mut state = state_with_codex_seed();
        let generation = state.provider_refresh_generation;
        assert!(matches!(
            reserve_provider_refresh(&mut state, true, &[ProviderId::Codex], generation),
            ProviderRefreshReservation::Reserved { .. }
        ));
        assert!(state.seeded_provider_ids.is_empty());
    }

    #[test]
    fn stale_inputs_cannot_reserve_a_new_generation_after_invalidation() {
        let mut state = AppState::new();
        let expected_generation = state.provider_refresh_generation;
        state.provider_refresh_generation = state.provider_refresh_generation.wrapping_add(1);

        assert_eq!(
            reserve_provider_refresh(&mut state, true, &[ProviderId::Codex], expected_generation,),
            ProviderRefreshReservation::Skipped(ProviderRefreshSkipReason::InputSuperseded {
                expected: expected_generation,
                current: state.provider_refresh_generation,
            })
        );
        assert!(!state.is_refreshing);
    }

    #[test]
    fn matching_generation_reserves_the_next_generation() {
        let mut state = AppState::new();
        let expected_generation = state.provider_refresh_generation;

        let ProviderRefreshReservation::Reserved { generation } =
            reserve_provider_refresh(&mut state, true, &[ProviderId::Codex], expected_generation)
        else {
            panic!("refresh should be reserved");
        };

        assert_eq!(generation, expected_generation.wrapping_add(1));
        assert!(state.is_refreshing);
    }

    #[test]
    fn superseded_generation_cannot_publish_or_release_its_successor() {
        let mut state = AppState::new();
        let initial_generation = state.provider_refresh_generation;
        let ProviderRefreshReservation::Reserved {
            generation: first_generation,
        } = reserve_provider_refresh(&mut state, true, &[ProviderId::Claude], initial_generation)
        else {
            panic!("first refresh should be reserved");
        };

        state.provider_refresh_generation = state.provider_refresh_generation.wrapping_add(1);
        state.is_refreshing = false;
        let successor_input_generation = state.provider_refresh_generation;
        let ProviderRefreshReservation::Reserved {
            generation: successor_generation,
        } = reserve_provider_refresh(
            &mut state,
            true,
            &[ProviderId::Claude],
            successor_input_generation,
        )
        else {
            panic!("successor refresh should be reserved");
        };

        assert_eq!(
            complete_provider_refresh(&mut state, first_generation),
            ProviderRefreshCompletion::Superseded {
                current_generation: successor_generation
            }
        );
        assert!(state.is_refreshing);

        assert!(matches!(
            complete_provider_refresh(&mut state, successor_generation),
            ProviderRefreshCompletion::Published { .. }
        ));
        assert!(!state.is_refreshing);
    }

    #[test]
    fn public_outcome_preserves_generation_ownership_and_skip_reason() {
        assert_eq!(
            ProviderRefreshOutcome::Published { generation: 42 },
            ProviderRefreshOutcome::Published { generation: 42 }
        );
        assert_eq!(
            ProviderRefreshOutcome::Superseded {
                generation: 41,
                current_generation: 42,
            },
            ProviderRefreshOutcome::Superseded {
                generation: 41,
                current_generation: 42,
            }
        );
        assert_eq!(
            ProviderRefreshOutcome::Skipped {
                reason: ProviderRefreshSkipReason::Active { generation: 42 },
            },
            ProviderRefreshOutcome::Skipped {
                reason: ProviderRefreshSkipReason::Active { generation: 42 },
            }
        );
    }
}
