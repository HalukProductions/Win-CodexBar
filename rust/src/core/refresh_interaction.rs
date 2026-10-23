//! Whether the provider fetch running on this task was started by the user or
//! by an automatic refresh.
//!
//! Upstream scopes this per task (`ProviderInteractionContext`). The Windows
//! shell sets it around each provider fetch: non-forced refreshes (the
//! scheduled timer, the auto-resume probe, and stale-gated launch and poll
//! refreshes) are `Background`; forced refreshes (the Refresh action, tray
//! Refresh, saved settings or account changes) are `UserInitiated`. Outside a
//! scope, such as the CLI, the value is `UserInitiated`.

use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshInteraction {
    #[default]
    UserInitiated,
    Background,
}

tokio::task_local! {
    static CURRENT: RefreshInteraction;
}

/// The interaction of the fetch running on this task. `UserInitiated` outside a scope.
pub fn current_refresh_interaction() -> RefreshInteraction {
    CURRENT
        .try_with(|interaction| *interaction)
        .unwrap_or_default()
}

/// Run `future` with `interaction` as the current refresh interaction.
pub async fn scope_refresh_interaction<F: Future>(
    interaction: RefreshInteraction,
    future: F,
) -> F::Output {
    CURRENT.scope(interaction, future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn defaults_to_user_initiated_outside_a_scope() {
        assert_eq!(
            current_refresh_interaction(),
            RefreshInteraction::UserInitiated
        );
    }

    #[tokio::test]
    async fn background_scope_is_visible_across_awaits_and_ends_with_the_scope() {
        let seen = scope_refresh_interaction(RefreshInteraction::Background, async {
            tokio::task::yield_now().await;
            current_refresh_interaction()
        })
        .await;

        assert_eq!(seen, RefreshInteraction::Background);
        assert_eq!(
            current_refresh_interaction(),
            RefreshInteraction::UserInitiated
        );
    }
}
