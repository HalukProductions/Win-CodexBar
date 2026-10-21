//! Token account commands and DTOs.
//!
//! Token values themselves are never exposed to the frontend; only masked
//! metadata flows through the bridge. Active-account selection is clamped
//! against the current list size before each response.

use codexbar::core::{
    ProviderAccountData, ProviderId, TokenAccount, TokenAccountStore, TokenAccountSupport,
};
use codexbar::providers::claude::cost_account;
use serde::Serialize;

/// Bridge-friendly token account support descriptor for a provider.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountSupportBridge {
    pub provider_id: String,
    pub display_name: String,
    pub title: String,
    pub subtitle: String,
    pub placeholder: String,
}

/// Bridge-friendly token account (token value is never exposed).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountBridge {
    pub id: String,
    pub label: String,
    pub added_at: String,
    pub last_used: Option<String>,
    pub is_active: bool,
    /// Claude config directory whose history this account owns for local
    /// cost estimates.
    pub claude_config_directory: Option<String>,
    /// Whether that directory currently exists.
    pub claude_config_directory_available: bool,
}

/// Bridge-friendly provider token accounts snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTokenAccountsBridge {
    pub provider_id: String,
    pub support: TokenAccountSupportBridge,
    pub accounts: Vec<TokenAccountBridge>,
    pub active_index: usize,
}

fn format_token_account_date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%b %d, %Y").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn build_provider_token_accounts(
    provider_id: ProviderId,
    support: &TokenAccountSupport,
    accounts: Vec<TokenAccount>,
    active_index: usize,
) -> ProviderTokenAccountsBridge {
    let support_bridge = TokenAccountSupportBridge {
        provider_id: provider_id.cli_name().to_string(),
        display_name: provider_id.display_name().to_string(),
        title: support.title.to_string(),
        subtitle: support.subtitle.to_string(),
        placeholder: support.placeholder.to_string(),
    };
    let account_bridges: Vec<TokenAccountBridge> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| TokenAccountBridge {
            id: a.id.to_string(),
            label: a.label.clone(),
            added_at: format_token_account_date(a.added_at),
            last_used: a.last_used.map(format_token_account_date),
            is_active: i == active_index,
            claude_config_directory: a.claude_config_directory.clone(),
            claude_config_directory_available: a
                .claude_config_directory
                .as_deref()
                .is_some_and(cost_account::is_config_directory_available),
        })
        .collect();
    ProviderTokenAccountsBridge {
        provider_id: provider_id.cli_name().to_string(),
        support: support_bridge,
        accounts: account_bridges,
        active_index,
    }
}

/// List all providers that support token accounts.
#[tauri::command]
pub fn get_token_account_providers() -> Vec<TokenAccountSupportBridge> {
    ProviderId::all()
        .iter()
        .filter_map(|&id| {
            TokenAccountSupport::for_provider(id).map(|s| TokenAccountSupportBridge {
                provider_id: id.cli_name().to_string(),
                display_name: id.display_name().to_string(),
                title: s.title.to_string(),
                subtitle: s.subtitle.to_string(),
                placeholder: s.placeholder.to_string(),
            })
        })
        .collect()
}

/// Load token accounts for a single provider.
#[tauri::command]
pub fn get_token_accounts(provider_id: String) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let store = TokenAccountStore::new();
    let data = store.load_provider(id).map_err(|e| e.to_string())?;
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Add a token account for a provider.
#[tauri::command]
pub fn add_token_account(
    app: tauri::AppHandle,
    provider_id: String,
    label: String,
    token: String,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    super::validate_single_line_secret(&token, "Token", super::MAX_COOKIE_HEADER_LEN)?;
    let label = super::sanitize_optional_label(Some(label))?
        .unwrap_or_else(|| id.display_name().to_string());
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    data.add_account(TokenAccount::new(label, token.trim()));
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    crate::auto_resume::clear(&app, id);
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Remove a token account by UUID string.
#[tauri::command]
pub fn remove_token_account(
    app: tauri::AppHandle,
    provider_id: String,
    account_id: String,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let uuid = uuid::Uuid::parse_str(&account_id).map_err(|e| e.to_string())?;
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    let before = mapped_cost_directory(id, &data);
    data.remove_account(uuid);
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    clear_costs_if_mapping_changed(id, before, &data);
    crate::auto_resume::clear(&app, id);
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Set the active token account for a provider by UUID string.
#[tauri::command]
pub fn set_active_token_account(
    app: tauri::AppHandle,
    provider_id: String,
    account_id: String,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let uuid = uuid::Uuid::parse_str(&account_id).map_err(|e| e.to_string())?;
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    let before = mapped_cost_directory(id, &data);
    data.set_active_by_id(uuid);
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    clear_costs_if_mapping_changed(id, before, &data);
    crate::auto_resume::clear(&app, id);
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Edit a saved token account in place. `label` replaces the saved label
/// unless it is blank. `token` replaces the saved credential only when it is
/// non-blank; a blank or missing value keeps the stored secret, which never
/// leaves the shell. Account id, timestamps, metadata and the active
/// selection are preserved.
#[tauri::command]
pub fn update_token_account(
    app: tauri::AppHandle,
    provider_id: String,
    account_id: String,
    label: String,
    token: Option<String>,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let uuid = uuid::Uuid::parse_str(&account_id).map_err(|e| e.to_string())?;
    let label = super::sanitize_optional_label(Some(label))?;
    let token = token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());
    if let Some(token) = token.as_deref() {
        super::validate_single_line_secret(token, "Token", super::MAX_COOKIE_HEADER_LEN)?;
    }
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    // Only a replaced credential on the active account changes what the next
    // fetch sends, so only that case drops pending auto-resume state.
    let replaces_active_credential = token.is_some()
        && data
            .active_account()
            .is_some_and(|account| account.id == uuid);
    data.update_account(uuid, label.as_deref(), token.as_deref())
        .map_err(|e| e.to_string())?;
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    if replaces_active_credential {
        crate::auto_resume::clear(&app, id);
    }
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Move a saved token account to `index` in the displayed list. The active
/// account is unchanged; it is re-resolved by id after the move.
#[tauri::command]
pub fn move_token_account(
    provider_id: String,
    account_id: String,
    index: usize,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let uuid = uuid::Uuid::parse_str(&account_id).map_err(|e| e.to_string())?;
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    data.move_account(uuid, index).map_err(|e| e.to_string())?;
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}

/// Claude local cost estimates follow the active account's mapped config
/// directory; other providers have no such mapping.
fn mapped_cost_directory(id: ProviderId, data: &ProviderAccountData) -> Option<std::path::PathBuf> {
    (id == ProviderId::Claude)
        .then(|| cost_account::mapped_config_directory(data))
        .flatten()
}

/// Drop memoized local cost summaries when the scanned Claude history changes.
fn clear_costs_if_mapping_changed(
    id: ProviderId,
    before: Option<std::path::PathBuf>,
    after: &ProviderAccountData,
) {
    if before != mapped_cost_directory(id, after) {
        crate::commands::clear_provider_local_usage_cache();
    }
}

/// Map a Claude token account to the Claude config directory whose
/// projects history its local cost estimates scan. None or a blank value
/// clears the mapping and restores the combined history.
#[tauri::command]
pub fn set_token_account_claude_config_dir(
    provider_id: String,
    account_id: String,
    directory: Option<String>,
) -> Result<ProviderTokenAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    if id != ProviderId::Claude {
        return Err(format!(
            "Provider {provider_id} does not support a Claude config directory"
        ));
    }
    let support = TokenAccountSupport::for_provider(id)
        .ok_or_else(|| format!("Provider {provider_id} does not support token accounts"))?;
    let uuid = uuid::Uuid::parse_str(&account_id).map_err(|e| e.to_string())?;
    let directory = cost_account::normalize_config_directory(directory.as_deref())?;
    let store = TokenAccountStore::new();
    let mut data = store.load_provider(id).map_err(|e| e.to_string())?;
    let before = mapped_cost_directory(id, &data);
    let account = data
        .accounts
        .iter_mut()
        .find(|account| account.id == uuid)
        .ok_or_else(|| "Token account not found".to_string())?;
    account.claude_config_directory = directory;
    store.save_provider(id, &data).map_err(|e| e.to_string())?;
    clear_costs_if_mapping_changed(id, before, &data);
    let active = data.clamped_active_index();
    Ok(build_provider_token_accounts(
        id,
        &support,
        data.accounts,
        active,
    ))
}
