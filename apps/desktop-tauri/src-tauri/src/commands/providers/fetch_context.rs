use super::*;

/// Build a `FetchContext` for a provider using persisted cookies/keys and its
/// active token account. The resolution itself lives in
/// `codexbar::core::build_fetch_context`, shared with `codexbar serve`.
pub(crate) fn build_fetch_context(
    id: ProviderId,
    settings: &Settings,
    cookies: &ManualCookies,
    api_keys: &ApiKeys,
    token_accounts: &HashMap<ProviderId, ProviderAccountData>,
) -> FetchContext {
    let active_account = token_accounts
        .get(&id)
        .and_then(|data| data.active_account());
    codexbar::core::build_fetch_context(id, settings, cookies, api_keys, active_account)
}

#[cfg(test)]
pub(crate) use codexbar::core::provider_cookie_domain;

const DEFAULT_PROVIDER_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);
const SLOW_PROVIDER_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);
const OPTIONAL_LITELLM_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
const MAX_CONTEXT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(65);

pub(crate) fn provider_fetch_timeout(id: ProviderId, ctx: &FetchContext) -> std::time::Duration {
    let provider_timeout = match id {
        ProviderId::Claude | ProviderId::Codex | ProviderId::Copilot => SLOW_PROVIDER_FETCH_TIMEOUT,
        ProviderId::LiteLLM if ctx.optional_details_enabled => OPTIONAL_LITELLM_FETCH_TIMEOUT,
        _ => DEFAULT_PROVIDER_FETCH_TIMEOUT,
    };
    let context_timeout = std::time::Duration::from_secs(ctx.web_timeout.saturating_add(5));
    provider_timeout.max(context_timeout.min(MAX_CONTEXT_FETCH_TIMEOUT))
}
