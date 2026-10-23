//! Finish step for the GitHub Copilot device-code login.
//!
//! The desktop shell and `codexbar config login --provider copilot` both call
//! these functions, so an account saved from either surface is the same: it is
//! labelled from the GitHub identity and plan, matched to an existing account
//! by stable identity, and written to the token-account store.

use super::CopilotApi;
use super::device_flow::{CopilotDeviceFlow, DeviceCodeResponse, DeviceFlowError};
use crate::core::{ProviderId, TokenAccount, TokenAccountError, TokenAccountStore};

/// Why a Copilot device login did not finish.
#[derive(Debug, thiserror::Error)]
pub enum CopilotLoginError {
    #[error("GitHub device login failed: {0}")]
    Flow(#[from] DeviceFlowError),
    #[error("{0}")]
    Store(#[from] TokenAccountError),
}

/// The account a successful login saved. The token is never included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedCopilotAccount {
    pub label: String,
    pub login: Option<String>,
    pub plan: Option<String>,
    pub identity_id: Option<u64>,
    /// Whether the provider was turned on after the save. The save itself
    /// leaves this false; `config login` sets it once it enables the provider.
    pub enabled: bool,
}

/// Asks GitHub for a device code. The caller shows `verification_uri` and
/// `user_code` to the user.
pub async fn start_device_login(
    flow: &CopilotDeviceFlow,
) -> Result<DeviceCodeResponse, CopilotLoginError> {
    Ok(flow.start_flow().await?)
}

/// Polls until the user approves the device code, then saves the account.
/// Polling uses the device flow's backoff, including persistent `slow_down`.
pub async fn finish_device_login(
    flow: &CopilotDeviceFlow,
    api: &CopilotApi,
    store: &TokenAccountStore,
    device: &DeviceCodeResponse,
) -> Result<SavedCopilotAccount, CopilotLoginError> {
    let token = flow
        .wait_for_token(&device.device_code, device.interval, device.expires_in)
        .await?;
    save_device_token(api, store, token).await
}

async fn save_device_token(
    api: &CopilotApi,
    store: &TokenAccountStore,
    token: String,
) -> Result<SavedCopilotAccount, CopilotLoginError> {
    let identity = api.fetch_identity_with_token(&token, None).await.ok();
    let plan = api
        .fetch_usage_with_token(&token, None, None)
        .await
        .ok()
        .and_then(|usage| usage.login_method);

    let login = identity.as_ref().map(|identity| identity.login.clone());
    let label = match (login.as_deref(), plan.as_deref()) {
        (Some(login), Some(plan)) => format!("{login} ({plan})"),
        (Some(login), None) => login.to_string(),
        (None, Some(plan)) => plan.to_string(),
        (None, None) => "GitHub Copilot".to_string(),
    };

    let mut data = store.load_provider(ProviderId::Copilot)?;
    let identity_id = identity.as_ref().and_then(|identity| identity.id);
    let existing_index =
        find_existing_copilot_account(api, &data.accounts, identity_id, login.as_deref()).await;

    let saved = SavedCopilotAccount {
        label: label.clone(),
        login: login.clone(),
        plan,
        identity_id,
        enabled: false,
    };

    if let Some(index) = existing_index {
        data.accounts[index].token = token;
        data.accounts[index].label = label;
        data.accounts[index].external_identifier = identity_id.map(copilot_external_identifier);
        data.set_active(index);
    } else {
        let mut account = TokenAccount::new(label, token);
        account.external_identifier = identity_id.map(copilot_external_identifier);
        account.mark_used();
        data.add_account(account);
        data.set_active(data.accounts.len().saturating_sub(1));
    }

    store.save_provider(ProviderId::Copilot, &data)?;
    Ok(saved)
}

async fn find_existing_copilot_account(
    api: &CopilotApi,
    accounts: &[TokenAccount],
    identity_id: Option<u64>,
    login: Option<&str>,
) -> Option<usize> {
    if let Some(identity_id) = identity_id
        && let Some(index) = find_copilot_account_by_identity(accounts, identity_id)
    {
        return Some(index);
    }

    let mut label_fallback = None;
    for (index, account) in accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| account.external_identifier.is_none())
    {
        if let Some(identity_id) = identity_id {
            match api.fetch_identity_with_token(&account.token, None).await {
                Ok(resolved_identity) if resolved_identity.id == Some(identity_id) => {
                    return Some(index);
                }
                Ok(resolved_identity) if resolved_identity.id.is_some() => continue,
                _ => {}
            }
        }

        if label_fallback.is_none() && copilot_label_matches_login(account, login) {
            label_fallback = Some(index);
        }
    }

    label_fallback
}

fn find_copilot_account_by_identity(accounts: &[TokenAccount], identity_id: u64) -> Option<usize> {
    let external_identifier = copilot_external_identifier(identity_id);
    accounts.iter().position(|account| {
        account.external_identifier.as_deref() == Some(external_identifier.as_str())
    })
}

fn copilot_external_identifier(identity_id: u64) -> String {
    format!("github:user:{identity_id}")
}

fn copilot_label_matches_login(account: &TokenAccount, login: Option<&str>) -> bool {
    login.is_some_and(|login| {
        account.label == login || account.label.starts_with(&format!("{login} ("))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ProviderAccountData;
    use crate::providers::copilot::endpoints::GitHubEndpoints;
    use crate::providers::copilot::test_server::{FakeGitHub, ok};
    use uuid::Uuid;

    // interval 0 keeps the polling sleep instant in these tests.
    const DEVICE_JSON: &str = r#"{"device_code":"device-example","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#;
    const TOKEN_JSON: &str =
        r#"{"access_token":"gho_example_token","token_type":"bearer","scope":"read:user"}"#;
    const USER_JSON: &str = r#"{"login":"octocat","id":42,"name":"Example Octocat"}"#;
    const USAGE_JSON: &str = r#"{"copilot_plan":"individual","quota_snapshots":{}}"#;

    fn github_fake(token_reply: &'static str) -> FakeGitHub {
        FakeGitHub::start(vec![
            ("/login/device/code", vec![ok(DEVICE_JSON)]),
            ("/login/oauth/access_token", vec![ok(token_reply)]),
            ("/user", vec![ok(USER_JSON)]),
            ("/copilot_internal/user", vec![ok(USAGE_JSON)]),
        ])
    }

    fn clients(fake: &FakeGitHub) -> (CopilotDeviceFlow, CopilotApi) {
        let endpoints = GitHubEndpoints::with_origin(&fake.origin).expect("loopback origin");
        (
            CopilotDeviceFlow::with_endpoints(&endpoints),
            CopilotApi::with_endpoints(endpoints),
        )
    }

    fn store_in(dir: &tempfile::TempDir) -> TokenAccountStore {
        TokenAccountStore::with_path(dir.path().join("token-accounts.json"))
    }

    fn seed(store: &TokenAccountStore, accounts: Vec<TokenAccount>) {
        let mut data = ProviderAccountData::new();
        for account in accounts {
            data.add_account(account);
        }
        store
            .save_provider(ProviderId::Copilot, &data)
            .expect("seed store");
    }

    #[tokio::test]
    async fn saves_a_new_account_with_a_stable_identifier() {
        let fake = github_fake(TOKEN_JSON);
        let (flow, api) = clients(&fake);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(&dir);

        let device = start_device_login(&flow).await.expect("device code");
        let saved = finish_device_login(&flow, &api, &store, &device)
            .await
            .expect("login");

        assert_eq!(saved.label, "octocat (Copilot Individual)");
        assert_eq!(saved.login.as_deref(), Some("octocat"));
        assert_eq!(saved.plan.as_deref(), Some("Copilot Individual"));
        assert_eq!(saved.identity_id, Some(42));

        let data = store.load_provider(ProviderId::Copilot).expect("load");
        assert_eq!(data.accounts.len(), 1);
        assert_eq!(data.active_index, 0);
        let account = &data.accounts[0];
        assert_eq!(account.label, "octocat (Copilot Individual)");
        assert_eq!(account.token, "gho_example_token");
        assert_eq!(
            account.external_identifier.as_deref(),
            Some("github:user:42")
        );
        assert!(account.last_used.is_some());
    }

    #[tokio::test]
    async fn relogin_updates_the_matching_account_in_place() {
        let fake = github_fake(TOKEN_JSON);
        let (flow, api) = clients(&fake);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(&dir);

        let existing_id = Uuid::new_v4();
        let mut existing = TokenAccount::new("octocat (old)", "gho_old_token");
        existing.id = existing_id;
        existing.external_identifier = Some("github:user:42".to_string());
        seed(&store, vec![existing]);

        let device = start_device_login(&flow).await.expect("device code");
        finish_device_login(&flow, &api, &store, &device)
            .await
            .expect("login");

        let data = store.load_provider(ProviderId::Copilot).expect("load");
        assert_eq!(data.accounts.len(), 1);
        assert_eq!(data.accounts[0].id, existing_id);
        assert_eq!(data.accounts[0].token, "gho_example_token");
        assert_eq!(data.accounts[0].label, "octocat (Copilot Individual)");
    }

    #[tokio::test]
    async fn adopts_a_legacy_account_with_the_same_github_identity() {
        let fake = github_fake(TOKEN_JSON);
        let (flow, api) = clients(&fake);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(&dir);

        let legacy_id = Uuid::new_v4();
        let mut legacy = TokenAccount::new("Work account", "gho_legacy_token");
        legacy.id = legacy_id;
        seed(&store, vec![legacy]);

        let device = start_device_login(&flow).await.expect("device code");
        finish_device_login(&flow, &api, &store, &device)
            .await
            .expect("login");

        let data = store.load_provider(ProviderId::Copilot).expect("load");
        assert_eq!(data.accounts.len(), 1, "legacy account is reused");
        assert_eq!(data.accounts[0].id, legacy_id);
        assert_eq!(
            data.accounts[0].external_identifier.as_deref(),
            Some("github:user:42")
        );
    }

    #[test]
    fn stored_identity_precedes_legacy_label_match() {
        let legacy = TokenAccount::new("octocat (Pro)", "old-token");
        let mut identified = TokenAccount::new("Renamed account", "known-token");
        identified.external_identifier = Some(copilot_external_identifier(123));

        let accounts = vec![legacy.clone(), identified];
        assert_eq!(find_copilot_account_by_identity(&accounts, 123), Some(1));
        assert!(copilot_label_matches_login(&legacy, Some("octocat")));
    }

    #[tokio::test]
    async fn denied_login_saves_nothing() {
        let fake = github_fake(r#"{"error":"access_denied"}"#);
        let (flow, api) = clients(&fake);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(&dir);

        let device = start_device_login(&flow).await.expect("device code");
        let error = finish_device_login(&flow, &api, &store, &device)
            .await
            .expect_err("denied");

        assert!(matches!(
            error,
            CopilotLoginError::Flow(DeviceFlowError::AccessDenied)
        ));
        assert_eq!(
            error.to_string(),
            "GitHub device login failed: Access denied by user"
        );
        assert!(
            !dir.path().join("token-accounts.json").exists(),
            "a denied login must not create the store"
        );
    }
}
