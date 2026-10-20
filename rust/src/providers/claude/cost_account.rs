//! Claude config directories mapped to token accounts (upstream #4556).
//!
//! A Claude token account can declare the Claude config directory whose
//! `projects` history belongs to it. While that account is active, local cost
//! estimates scan only `<dir>\projects`: the ambient `CLAUDE_CONFIG_DIR` /
//! `%USERPROFILE%\.claude` root, claude-swap homes and Pi history are left
//! out. Unmapped accounts keep the combined history. The mapping is the
//! user's ownership declaration; nothing is inferred from transcripts, and it
//! never changes quota fetching or credential resolution.

use std::path::{Path, PathBuf};

use crate::core::{ProviderAccountData, ProviderId, TokenAccountStore};

/// Longest accepted directory path, in characters.
const MAX_CONFIG_DIRECTORY_LEN: usize = 4096;

/// The mapped config directory of the active Claude token account.
pub fn active_config_directory() -> Option<PathBuf> {
    active_config_directory_in(&TokenAccountStore::new())
}

/// [`active_config_directory`] against an explicit store.
pub fn active_config_directory_in(store: &TokenAccountStore) -> Option<PathBuf> {
    match store.load_provider(ProviderId::Claude) {
        Ok(data) => mapped_config_directory(&data),
        Err(error) => {
            tracing::debug!(%error, "Claude token accounts unreadable; using combined cost history");
            None
        }
    }
}

/// The active account's mapped directory, when it has a non-blank one.
pub fn mapped_config_directory(data: &ProviderAccountData) -> Option<PathBuf> {
    data.active_account()?
        .claude_config_directory
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

/// Validate a directory chosen in Settings. `None` or a blank value clears the
/// mapping; anything else must be one absolute single-line path.
pub fn normalize_config_directory(input: Option<&str>) -> Result<Option<String>, String> {
    let Some(trimmed) = input.map(str::trim).filter(|dir| !dir.is_empty()) else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_CONFIG_DIRECTORY_LEN {
        return Err("Claude config directory path is too long.".to_string());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("Claude config directory must be a single line.".to_string());
    }
    if !Path::new(trimmed).is_absolute() {
        return Err("Claude config directory must be an absolute path.".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

/// Whether a mapped directory currently exists as a directory.
pub fn is_config_directory_available(directory: &str) -> bool {
    Path::new(directory).is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::TokenAccount;

    fn account(label: &str, directory: Option<&str>) -> TokenAccount {
        let mut account = TokenAccount::new(label, format!("fixture-{label}"));
        account.claude_config_directory = directory.map(str::to_string);
        account
    }

    #[test]
    fn legacy_accounts_decode_without_a_directory_and_mapped_accounts_retain_it() {
        let legacy = serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "label": "Personal",
            "token": "fixture-token",
            "added_at": 1,
        });
        let decoded: TokenAccount = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.claude_config_directory, None);
        let encoded = serde_json::to_value(&decoded).unwrap();
        assert!(encoded.get("claude_config_directory").is_none());

        let mut mapped = account("Personal", Some(r"C:\fixtures\claude-personal"));
        mapped.mark_used();
        let round_trip: TokenAccount =
            serde_json::from_value(serde_json::to_value(&mapped).unwrap()).unwrap();
        assert_eq!(
            round_trip.claude_config_directory.as_deref(),
            Some(r"C:\fixtures\claude-personal")
        );
    }

    #[test]
    fn only_the_active_account_mapping_applies() {
        let mut data = ProviderAccountData::new();
        data.add_account(account("Personal", Some(r"C:\fixtures\personal")));
        data.add_account(account("Work", Some("  ")));
        data.add_account(account("Combined", None));

        data.set_active(0);
        assert_eq!(
            mapped_config_directory(&data),
            Some(PathBuf::from(r"C:\fixtures\personal"))
        );
        data.set_active(1);
        assert_eq!(mapped_config_directory(&data), None);
        data.set_active(2);
        assert_eq!(mapped_config_directory(&data), None);
        assert_eq!(mapped_config_directory(&ProviderAccountData::new()), None);
    }

    #[test]
    fn the_store_mapping_follows_the_active_account_and_survives_token_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenAccountStore::with_path(dir.path().join("token-accounts.json"));
        assert_eq!(active_config_directory_in(&store), None);

        let mapped = dir.path().join("personal");
        let mut data = ProviderAccountData::new();
        data.add_account(account("Personal", mapped.to_str()));
        data.add_account(account("Combined", None));
        data.set_active(0);
        store.save_provider(ProviderId::Claude, &data).unwrap();
        assert_eq!(active_config_directory_in(&store), Some(mapped.clone()));

        // A refreshed credential keeps the declared history directory.
        let mut reloaded = store.load_provider(ProviderId::Claude).unwrap();
        reloaded.accounts[0].token = "fixture-refreshed".to_string();
        store.save_provider(ProviderId::Claude, &reloaded).unwrap();
        assert_eq!(active_config_directory_in(&store), Some(mapped));

        let mut switched = store.load_provider(ProviderId::Claude).unwrap();
        switched.set_active(1);
        store.save_provider(ProviderId::Claude, &switched).unwrap();
        assert_eq!(active_config_directory_in(&store), None);
    }

    #[test]
    fn chosen_directories_must_be_absolute_single_line_paths() {
        assert_eq!(normalize_config_directory(None), Ok(None));
        assert_eq!(normalize_config_directory(Some("   ")), Ok(None));
        assert_eq!(
            normalize_config_directory(Some(r"  C:\Users\fixture\.claude-work  ")),
            Ok(Some(r"C:\Users\fixture\.claude-work".to_string()))
        );
        assert!(normalize_config_directory(Some(r"relative\claude")).is_err());
        assert!(normalize_config_directory(Some("C:\\a\nb")).is_err());
        assert!(normalize_config_directory(Some(&format!("C:\\{}", "a".repeat(5000)))).is_err());
    }

    #[test]
    fn availability_requires_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(is_config_directory_available(dir.path().to_str().unwrap()));
        assert!(!is_config_directory_available(file.to_str().unwrap()));
        assert!(!is_config_directory_available(
            dir.path().join("missing").to_str().unwrap()
        ));
    }
}
