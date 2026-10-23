//! Account-specific native tray menu construction and dispatch.
//!
//! Keep provider/account workflows out of the generic tray shell so adding a
//! new account action does not grow `tray_bridge.rs` into another controller.

use codexbar::codex_accounts::{CodexAccount, ordinals_by_id};
use codexbar::locale::{self, LocaleKey};
use codexbar::settings::{Language, Settings};
use std::collections::HashMap;
use tauri::AppHandle;
#[cfg(test)]
use uuid::Uuid;

use crate::tray_menu::TrayMenuEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccountMenuAction {
    AddCodexAccount,
    AddClaudeAccount,
    SaveClaudeAccount,
    CancelClaudeLogin,
    SwitchClaudeAccount(String),
    SwitchCodexAccount(String),
    AddGrokAccount,
    SaveGrokAccount,
    CancelGrokLogin,
    SwitchGrokAccount(String),
}

pub(crate) fn prepend_account_menus(
    spec: &mut Vec<TrayMenuEntry>,
    settings: &Settings,
    claude_usage: &HashMap<String, crate::commands::ClaudeAccountUsageState>,
) {
    let accounts = crate::commands::load_codex_accounts().unwrap_or_default();
    let active =
        codexbar::codex_accounts::CodexAccountManager::new().discover_ambient_account(&accounts);
    spec.insert(
        0,
        codex_accounts_menu(
            &accounts,
            active.as_ref(),
            settings.ui_language,
            settings.hide_personal_info,
        ),
    );

    let claude_accounts = crate::commands::claude_accounts_list().unwrap_or_default();
    spec.insert(
        1,
        claude_accounts_menu(
            &claude_accounts,
            settings.ui_language,
            settings.hide_personal_info,
            settings.show_as_used,
            claude_usage,
        ),
    );

    let grok_accounts = crate::commands::grok_accounts_list().unwrap_or_default();
    spec.insert(
        2,
        grok_accounts_menu(
            &grok_accounts,
            settings.ui_language,
            settings.hide_personal_info,
        ),
    );
}

pub(crate) fn resolve_action(id: &str) -> Option<AccountMenuAction> {
    match id {
        "add_codex_account" => Some(AccountMenuAction::AddCodexAccount),
        "add_claude_account" => Some(AccountMenuAction::AddClaudeAccount),
        "save_claude_account" => Some(AccountMenuAction::SaveClaudeAccount),
        "cancel_claude_login" => Some(AccountMenuAction::CancelClaudeLogin),
        "add_grok_account" => Some(AccountMenuAction::AddGrokAccount),
        "save_grok_account" => Some(AccountMenuAction::SaveGrokAccount),
        "cancel_grok_login" => Some(AccountMenuAction::CancelGrokLogin),
        _ if id.starts_with("switch_grok_account:") => {
            let id = id.strip_prefix("switch_grok_account:")?;
            (!id.is_empty()).then(|| AccountMenuAction::SwitchGrokAccount(id.to_string()))
        }
        _ if id.starts_with("switch_claude_account:") => {
            let id = id.strip_prefix("switch_claude_account:")?;
            (!id.is_empty()).then(|| AccountMenuAction::SwitchClaudeAccount(id.to_string()))
        }
        _ if id.starts_with("switch_codex_account:") => {
            let id = id.strip_prefix("switch_codex_account:")?;
            uuid::Uuid::parse_str(id).ok()?;
            Some(AccountMenuAction::SwitchCodexAccount(id.to_string()))
        }
        _ => None,
    }
}

pub(crate) fn handle_action(app: &AppHandle, action: AccountMenuAction) {
    match action {
        AccountMenuAction::AddCodexAccount => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                match crate::commands::codex_account_add(handle.clone()).await {
                    Ok(_) => show_codex_message(&handle, "Codex account added."),
                    Err(error) => show_codex_message(&handle, &error),
                }
            });
        }
        AccountMenuAction::SwitchCodexAccount(id) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                match crate::commands::codex_account_switch(handle.clone(), id).await {
                    Ok(result) => {
                        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                        if result.desktop_session_restore_path.is_some() {
                            let dialog_handle = handle.clone();
                            let restart = tauri::async_runtime::spawn_blocking(move || {
                                dialog_handle
                                    .dialog()
                                    .message("Account switched. Restart Codex Desktop to use it? This stops running desktop tasks.")
                                    .title("Codex Accounts")
                                    .buttons(MessageDialogButtons::OkCancelCustom(
                                        "Restart".into(),
                                        "Later".into(),
                                    ))
                                    .blocking_show()
                            })
                            .await
                            .unwrap_or(false);
                            if restart {
                                let restart_result =
                                    crate::commands::codex_account_restart_desktop(
                                        handle.clone(),
                                        result.switch_id.to_string(),
                                    )
                                    .await;
                                if let Err(error) = restart_result {
                                    show_codex_message(&handle, &error);
                                }
                            }
                        } else {
                            show_codex_message(&handle, "Codex account switched.");
                        }
                    }
                    Err(error) => show_codex_message(&handle, &error),
                }
            });
        }
        AccountMenuAction::CancelClaudeLogin => crate::commands::claude_account_cancel_login(),
        AccountMenuAction::CancelGrokLogin => crate::commands::grok_account_cancel_login(),
        action @ (AccountMenuAction::AddGrokAccount
        | AccountMenuAction::SaveGrokAccount
        | AccountMenuAction::SwitchGrokAccount(_)) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                use tauri_plugin_dialog::DialogExt;
                let (result, message) = match action {
                    AccountMenuAction::AddGrokAccount => (
                        crate::commands::grok_account_add(handle.clone()).await,
                        "Grok account added. Select it to switch.",
                    ),
                    AccountMenuAction::SaveGrokAccount => (
                        crate::commands::grok_account_save_current(handle.clone()).await,
                        "Current Grok account saved.",
                    ),
                    AccountMenuAction::SwitchGrokAccount(id) => (
                        crate::commands::grok_account_switch(handle.clone(), id).await,
                        "Grok account switched. Restart Grok CLI sessions to use it.",
                    ),
                    _ => unreachable!(),
                };
                handle
                    .dialog()
                    .message(result.err().unwrap_or_else(|| message.to_string()))
                    .title("Grok accounts")
                    .show(|_| {});
            });
        }
        action @ (AccountMenuAction::AddClaudeAccount
        | AccountMenuAction::SaveClaudeAccount
        | AccountMenuAction::SwitchClaudeAccount(_)) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                use tauri_plugin_dialog::DialogExt;
                let (result, message) = match action {
                    AccountMenuAction::AddClaudeAccount => (
                        crate::commands::claude_account_add(handle.clone(), None).await,
                        "Claude Code account added. Select it to switch.",
                    ),
                    AccountMenuAction::SaveClaudeAccount => (
                        crate::commands::claude_account_save_current(handle.clone()).await,
                        "Current Claude Code account saved.",
                    ),
                    AccountMenuAction::SwitchClaudeAccount(id) => (
                        crate::commands::claude_account_switch(handle.clone(), id)
                            .await
                            .map(|_| ()),
                        "Claude Code account switched. Reopen the Claude Code CLI to use it.",
                    ),
                    _ => unreachable!(),
                };
                handle
                    .dialog()
                    .message(result.err().unwrap_or_else(|| message.to_string()))
                    .title("Claude Code accounts")
                    .show(|_| {});
            });
        }
    }
}

fn show_codex_message(app: &AppHandle, message: &str) {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .message(message)
        .title("Codex Accounts")
        .show(|_| {});
}

fn codex_accounts_menu(
    accounts: &[CodexAccount],
    active: Option<&CodexAccount>,
    lang: Language,
    hide_personal_info: bool,
) -> TrayMenuEntry {
    let text = |key| locale::get_text(lang, key);
    let ordinals = ordinals_by_id(accounts);
    let mut children: Vec<_> = accounts
        .iter()
        .map(|account| {
            let is_active = active.is_some_and(|current| current.matches(account));
            let mut entry = TrayMenuEntry::check_item(
                format!("switch_codex_account:{}", account.id),
                codex_account_menu_label(account, lang, hide_personal_info, ordinals[&account.id]),
                is_active,
            );
            entry.disabled = is_active;
            entry
        })
        .collect();
    if children.is_empty() {
        children.push(TrayMenuEntry::status_row(
            "codex_accounts_empty",
            text(LocaleKey::CodexAccountsEmpty),
        ));
    }
    children.push(TrayMenuEntry::separator());
    children.push(TrayMenuEntry::item(
        "add_codex_account",
        text(LocaleKey::CodexAccountsAddButton),
    ));
    TrayMenuEntry::submenu(
        "codex_accounts",
        text(LocaleKey::CodexAccountsTitle),
        children,
    )
}

fn codex_account_menu_label(
    account: &CodexAccount,
    lang: Language,
    hide_personal_info: bool,
    ordinal: usize,
) -> String {
    account.privacy_safe_display_name(
        hide_personal_info,
        ordinal,
        &locale::get_text(lang, LocaleKey::Account),
    )
}

/// Number accounts 1..n in list order, so the first row shown is "Account 1".
/// Saved accounts keep their stored order across switches. Settings uses the
/// same rule (`buildAccountOrdinals`).
fn ordinals_in_list_order<'a>(ids: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut ordinals = HashMap::new();
    for id in ids {
        let next = ordinals.len() + 1;
        ordinals.entry(id).or_insert(next);
    }
    ordinals
}

/// "Account N": Hide Personal Info shows no part of an email (Mac
/// `PersonalInfoRedactor.AccountOrdinal`).
fn hidden_account_label(lang: Language, ordinal: usize) -> String {
    let word = locale::get_text(lang, LocaleKey::Account);
    let word = word.trim();
    let word = if word.is_empty() { "Account" } else { word };
    format!("{word} {ordinal}")
}

/// Session and weekly percentages for one Claude account row, from the last
/// successful check recorded for that account. No recorded check, no percentage.
/// A window whose reset time has passed is skipped: the stored percentage is
/// known to be stale (restored cache, failed checks, or refresh turned off).
fn claude_usage_summary(
    lang: Language,
    show_as_used: bool,
    state: Option<&crate::commands::ClaudeAccountUsageState>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let usage = state?.usage.as_ref()?;
    let suffix = locale::get_text(
        lang,
        if show_as_used {
            LocaleKey::PanelUsedSuffix
        } else {
            LocaleKey::PanelLeftSuffix
        },
    );
    let parts: Vec<String> = [
        (LocaleKey::ProviderSessionLabel, usage.five_hour.as_ref()),
        (LocaleKey::ProviderWeeklyLabel, usage.seven_day.as_ref()),
    ]
    .into_iter()
    .filter_map(|(key, window)| {
        window
            .filter(|window| !window_has_reset(window, now))
            .map(|window| {
                let used = window.used_percent.clamp(0.0, 100.0);
                let shown = if show_as_used { used } else { 100.0 - used };
                format!("{} {shown:.0}% {suffix}", locale::get_text(lang, key))
            })
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn window_has_reset(
    window: &crate::commands::ClaudeAccountWindow,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    window
        .resets_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| at <= now)
}

fn claude_accounts_menu(
    accounts: &[codexbar::providers::claude::accounts::ClaudeAccount],
    lang: Language,
    hide_personal_info: bool,
    show_as_used: bool,
    usage: &HashMap<String, crate::commands::ClaudeAccountUsageState>,
) -> TrayMenuEntry {
    let text = |key| locale::get_text(lang, key);
    let ordinals = ordinals_in_list_order(accounts.iter().map(|account| account.id.as_str()));
    let now = chrono::Utc::now();
    let mut children: Vec<_> = accounts
        .iter()
        .map(|account| {
            let label = if hide_personal_info {
                hidden_account_label(lang, ordinals[account.id.as_str()])
            } else {
                account.organization.as_ref().map_or_else(
                    || account.email.clone(),
                    |org| format!("{} ({org})", account.email),
                )
            };
            let label = match claude_usage_summary(lang, show_as_used, usage.get(&account.id), now)
            {
                Some(summary) => format!("{label} - {summary}"),
                None => label,
            };
            let mut entry = TrayMenuEntry::check_item(
                format!("switch_claude_account:{}", account.id),
                label,
                account.is_active,
            );
            entry.disabled = account.is_active || !account.is_saved;
            entry
        })
        .collect();
    if children.is_empty() {
        children.push(TrayMenuEntry::status_row(
            "claude_accounts_empty",
            text(LocaleKey::ClaudeAccountsEmpty),
        ));
    }
    children.push(TrayMenuEntry::separator());
    children.push(TrayMenuEntry::item(
        "add_claude_account",
        text(LocaleKey::CodexAccountsAddButton),
    ));
    children.push(TrayMenuEntry::item(
        "save_claude_account",
        text(LocaleKey::ClaudeAccountsSaveCurrent),
    ));
    children.push(TrayMenuEntry::item(
        "cancel_claude_login",
        text(LocaleKey::ClaudeAccountsCancelLogin),
    ));
    TrayMenuEntry::submenu(
        "claude_accounts",
        text(LocaleKey::ClaudeAccountsTitle),
        children,
    )
}

fn grok_accounts_menu(
    accounts: &[codexbar::providers::grok::accounts::GrokAccount],
    lang: Language,
    hide_personal_info: bool,
) -> TrayMenuEntry {
    let text = |key| locale::get_text(lang, key);
    let ordinals = ordinals_in_list_order(accounts.iter().map(|account| account.id.as_str()));
    let mut children: Vec<_> = accounts
        .iter()
        .map(|account| {
            let label = if hide_personal_info {
                hidden_account_label(lang, ordinals[account.id.as_str()])
            } else {
                account.email.clone()
            };
            let mut entry = TrayMenuEntry::check_item(
                format!("switch_grok_account:{}", account.id),
                label,
                account.is_active,
            );
            entry.disabled = account.is_active || !account.is_saved;
            entry
        })
        .collect();
    if children.is_empty() {
        children.push(TrayMenuEntry::status_row(
            "grok_accounts_empty",
            text(LocaleKey::GrokAccountsEmpty),
        ));
    }
    children.push(TrayMenuEntry::separator());
    children.push(TrayMenuEntry::item(
        "add_grok_account",
        text(LocaleKey::CodexAccountsAddButton),
    ));
    children.push(TrayMenuEntry::item(
        "save_grok_account",
        text(LocaleKey::GrokAccountsSaveCurrent),
    ));
    children.push(TrayMenuEntry::item(
        "cancel_grok_login",
        text(LocaleKey::GrokAccountsCancelLogin),
    ));
    TrayMenuEntry::submenu(
        "grok_accounts",
        text(LocaleKey::GrokAccountsTitle),
        children,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray_menu::tests::menu_contains;

    #[test]
    fn account_action_ids_are_typed_and_validated() {
        assert_eq!(
            resolve_action("add_claude_account"),
            Some(AccountMenuAction::AddClaudeAccount)
        );
        assert_eq!(
            resolve_action("save_claude_account"),
            Some(AccountMenuAction::SaveClaudeAccount)
        );
        assert_eq!(
            resolve_action("cancel_claude_login"),
            Some(AccountMenuAction::CancelClaudeLogin)
        );
        assert_eq!(
            resolve_action("switch_claude_account:a:org"),
            Some(AccountMenuAction::SwitchClaudeAccount("a:org".into()))
        );
        assert!(resolve_action("switch_claude_account:").is_none());
        assert!(resolve_action("switch_codex_account:not-a-uuid").is_none());
        assert_eq!(
            resolve_action("add_grok_account"),
            Some(AccountMenuAction::AddGrokAccount)
        );
        assert_eq!(
            resolve_action("switch_grok_account:user-two"),
            Some(AccountMenuAction::SwitchGrokAccount("user-two".into()))
        );
        assert!(resolve_action("switch_grok_account:").is_none());
    }

    #[test]
    fn account_submenu_actionable_ids_resolve_and_empty_rows_do_not() {
        use codexbar::codex_accounts::{CodexAccountSource, utc_now};
        use codexbar::providers::claude::accounts::ClaudeAccount;
        use codexbar::providers::grok::accounts::GrokAccount;
        let codex = CodexAccount::new(
            uuid::Uuid::new_v4(),
            Some("Work".into()),
            None,
            None,
            Some("Work".into()),
            std::path::PathBuf::from("work"),
            CodexAccountSource::ManagedByApp,
            utc_now(),
            utc_now(),
            None,
        );
        let claude = ClaudeAccount {
            id: "a:org".into(),
            email: "a@example.com".into(),
            organization: None,
            plan: None,
            is_active: false,
            is_saved: true,
        };
        let grok = GrokAccount {
            id: "user-a".into(),
            email: "a@example.com".into(),
            organization: None,
            plan: None,
            is_active: false,
            is_saved: true,
        };
        let menus = [
            codex_accounts_menu(
                std::slice::from_ref(&codex),
                Some(&codex),
                Language::English,
                false,
            ),
            codex_accounts_menu(&[], None, Language::English, false),
            claude_accounts_menu(
                std::slice::from_ref(&claude),
                Language::English,
                false,
                true,
                &HashMap::new(),
            ),
            claude_accounts_menu(&[], Language::English, false, true, &HashMap::new()),
            grok_accounts_menu(std::slice::from_ref(&grok), Language::English, false),
            grok_accounts_menu(&[], Language::English, false),
        ];

        let mut resolved = Vec::new();
        for menu in &menus {
            for id in crate::tray_menu::tests::actionable_ids(std::slice::from_ref(menu)) {
                assert!(
                    crate::tray_bridge::resolve_menu_action(&id).is_some(),
                    "account id {id} does not resolve to a menu action"
                );
                resolved.push(id);
            }
        }
        for expected in [
            "add_codex_account",
            "switch_claude_account:a:org",
            "add_claude_account",
            "save_claude_account",
            "cancel_claude_login",
            "switch_grok_account:user-a",
            "add_grok_account",
            "save_grok_account",
            "cancel_grok_login",
        ] {
            assert!(
                resolved.iter().any(|id| id == expected),
                "{expected} should be actionable"
            );
        }
        for empty_row in [
            "codex_accounts_empty",
            "claude_accounts_empty",
            "grok_accounts_empty",
        ] {
            assert!(
                crate::tray_bridge::resolve_menu_action(empty_row).is_none(),
                "{empty_row} is a disabled status row and must not dispatch"
            );
        }
    }

    #[test]
    fn codex_accounts_can_be_switched_or_added_from_tray() {
        use codexbar::codex_accounts::{CodexAccountSource, utc_now};
        let make = |name: &str| {
            CodexAccount::new(
                uuid::Uuid::new_v4(),
                Some(name.into()),
                None,
                None,
                Some(name.into()),
                std::path::PathBuf::from(name),
                CodexAccountSource::ManagedByApp,
                utc_now(),
                utc_now(),
                None,
            )
        };
        let first = make("Personal");
        let second = make("Work");
        let menu = codex_accounts_menu(
            &[first.clone(), second.clone()],
            Some(&first),
            Language::English,
            false,
        );
        assert_eq!(menu.children[0].checked, Some(true));
        assert!(menu.children[0].disabled);
        assert_eq!(
            menu.children[1].id.as_deref(),
            Some(format!("switch_codex_account:{}", second.id).as_str())
        );
        assert_eq!(menu.children[1].checked, Some(false));
        assert!(!menu.children[1].disabled);
        assert!(menu_contains(&menu.children, "add_codex_account"));
        let empty = codex_accounts_menu(&[], None, Language::English, false);
        assert!(menu_contains(&empty.children, "add_codex_account"));
        let mut email_account = second;
        email_account.source = CodexAccountSource::Ambient;
        email_account.nickname = None;
        email_account.email_hint = Some("private@example.com".into());
        let private = codex_accounts_menu(&[email_account.clone()], None, Language::English, true);
        assert!(!private.children[0].label.contains("private@example.com"));
        let visible = codex_accounts_menu(&[email_account], None, Language::English, false);
        assert_eq!(visible.children[0].label, "private@example.com");
    }

    #[test]
    fn hidden_codex_tray_labels_are_opaque_and_stable() {
        use codexbar::codex_accounts::{CodexAccountSource, utc_now};

        let make = |id: &str, nickname: Option<&str>, email: &str| {
            CodexAccount::new(
                Uuid::parse_str(id).unwrap(),
                nickname.map(str::to_string),
                Some(email.to_string()),
                None,
                None,
                std::path::PathBuf::from("C:/private-home"),
                CodexAccountSource::ManagedByApp,
                utc_now(),
                utc_now(),
                None,
            )
        };
        let mut with_nickname = make(
            "00000000-0000-0000-0000-000000000002",
            Some("Work"),
            "user@example.com",
        );
        let mut without_nickname = make(
            "00000000-0000-0000-0000-000000000001",
            None,
            "personal@example.com",
        );
        with_nickname.source = CodexAccountSource::ManagedByApp;
        without_nickname.source = CodexAccountSource::Ambient;

        let accounts = [with_nickname.clone(), without_nickname.clone()];
        let ordinals = ordinals_by_id(&accounts);
        assert_eq!(ordinals[&without_nickname.id], 1);
        assert_eq!(ordinals[&with_nickname.id], 2);
        assert_eq!(
            codex_account_menu_label(
                &with_nickname,
                Language::English,
                true,
                ordinals[&with_nickname.id],
            ),
            "Account 2"
        );
        assert_eq!(
            codex_account_menu_label(
                &without_nickname,
                Language::English,
                true,
                ordinals[&without_nickname.id],
            ),
            "Account 1"
        );

        let hidden =
            codex_accounts_menu(&accounts, Some(&without_nickname), Language::English, true);
        assert_eq!(hidden.children[0].label, "Account 2");
        assert_eq!(hidden.children[1].label, "Account 1");
        assert_eq!(hidden.children[0].checked, Some(false));
        assert!(!hidden.children[0].disabled);
        assert_eq!(hidden.children[1].checked, Some(true));
        assert!(hidden.children[1].disabled);
        assert_eq!(
            hidden.children[1].id.as_deref(),
            Some(format!("switch_codex_account:{}", without_nickname.id).as_str())
        );
        assert!(hidden.children[0].label.contains("Account"));
        assert!(!hidden.children[1].label.contains('@'));
        assert!(!hidden.children[1].label.contains("example.com"));
        assert!(!hidden.children[1].label.contains("personal"));

        let reversed = codex_accounts_menu(
            &[without_nickname.clone(), with_nickname.clone()],
            None,
            Language::English,
            true,
        );
        assert_eq!(reversed.children[0].label, "Account 1");
        assert_eq!(reversed.children[1].label, "Account 2");

        let visible = codex_accounts_menu(&[with_nickname], None, Language::English, false);
        assert_eq!(visible.children[0].label, "user@example.com — Work");
    }

    #[test]
    fn claude_menu_checks_current_account_and_routes_saved_accounts() {
        use codexbar::providers::claude::accounts::ClaudeAccount;
        let current = ClaudeAccount {
            id: "a:org".into(),
            email: "a@example.com".into(),
            organization: None,
            plan: None,
            is_active: true,
            is_saved: false,
        };
        let saved = ClaudeAccount {
            id: "b:org".into(),
            is_active: false,
            is_saved: true,
            ..current.clone()
        };
        let menu = claude_accounts_menu(
            &[current, saved],
            Language::English,
            false,
            true,
            &HashMap::new(),
        );
        assert_eq!(menu.id.as_deref(), Some("claude_accounts"));
        assert_eq!(menu.children[0].checked, Some(true));
        assert!(menu.children[0].disabled);
        assert_eq!(
            menu.children[1].id.as_deref(),
            Some("switch_claude_account:b:org")
        );
        assert!(!menu.children[1].disabled);
        assert!(menu_contains(&menu.children, "add_claude_account"));
        assert!(menu_contains(
            &claude_accounts_menu(&[], Language::English, false, true, &HashMap::new()).children,
            "add_claude_account"
        ));
    }

    #[test]
    fn claude_rows_show_each_accounts_last_recorded_usage() {
        use crate::commands::{ClaudeAccountUsage, ClaudeAccountUsageState, ClaudeAccountWindow};
        let accounts = [claude_menu_account("a:org"), claude_menu_account("b:org")];
        let mut usage = HashMap::new();
        usage.insert(
            "a:org".to_string(),
            ClaudeAccountUsageState {
                usage: Some(ClaudeAccountUsage {
                    five_hour: Some(ClaudeAccountWindow {
                        used_percent: 42.4,
                        resets_at: None,
                    }),
                    seven_day: Some(ClaudeAccountWindow {
                        used_percent: 10.0,
                        resets_at: None,
                    }),
                    updated_at: "2026-10-18T00:00:00+00:00".into(),
                }),
                ..Default::default()
            },
        );
        usage.insert(
            "b:org".to_string(),
            ClaudeAccountUsageState {
                usage_error: Some("Claude usage check failed. Will retry automatically.".into()),
                ..Default::default()
            },
        );
        let menu = claude_accounts_menu(&accounts, Language::English, false, true, &usage);
        let session = locale::get_text(Language::English, LocaleKey::ProviderSessionLabel);
        let weekly = locale::get_text(Language::English, LocaleKey::ProviderWeeklyLabel);
        assert_eq!(
            menu.children[0].label,
            format!("a:org@example.com - {session} 42% used, {weekly} 10% used")
        );
        // A row with no successful check recorded shows no percentage.
        assert_eq!(menu.children[1].label, "b:org@example.com");
    }

    #[test]
    fn claude_usage_summary_skips_windows_past_their_reset() {
        use crate::commands::{ClaudeAccountUsage, ClaudeAccountUsageState, ClaudeAccountWindow};
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-18T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let state = |five_hour: Option<&str>, seven_day: Option<&str>| ClaudeAccountUsageState {
            usage: Some(ClaudeAccountUsage {
                five_hour: Some(ClaudeAccountWindow {
                    used_percent: 42.0,
                    resets_at: five_hour.map(String::from),
                }),
                seven_day: Some(ClaudeAccountWindow {
                    used_percent: 10.0,
                    resets_at: seven_day.map(String::from),
                }),
                updated_at: "2026-10-18T00:00:00+00:00".into(),
            }),
            ..Default::default()
        };
        let session = locale::get_text(Language::English, LocaleKey::ProviderSessionLabel);
        let weekly = locale::get_text(Language::English, LocaleKey::ProviderWeeklyLabel);

        let expired_session = state(Some("2026-10-18T05:00:00Z"), Some("2026-10-20T00:00:00Z"));
        assert_eq!(
            claude_usage_summary(Language::English, true, Some(&expired_session), now),
            Some(format!("{weekly} 10% used"))
        );
        let both_expired = state(
            Some("2026-10-18T12:00:00Z"),
            Some("2026-10-17T00:00:00+02:00"),
        );
        assert_eq!(
            claude_usage_summary(Language::English, true, Some(&both_expired), now),
            None
        );
        // Missing or unparseable reset times keep the window.
        let unknown = state(None, Some("not a date"));
        assert_eq!(
            claude_usage_summary(Language::English, true, Some(&unknown), now),
            Some(format!("{session} 42% used, {weekly} 10% used"))
        );
        assert_eq!(
            claude_usage_summary(Language::English, false, Some(&unknown), now),
            Some(format!("{session} 58% left, {weekly} 90% left"))
        );
    }

    #[test]
    fn grok_menu_checks_current_account_and_routes_saved_accounts() {
        use codexbar::providers::grok::accounts::GrokAccount;
        let current = GrokAccount {
            id: "user-a".into(),
            email: "a@example.com".into(),
            organization: None,
            plan: Some("SuperGrok".into()),
            is_active: true,
            is_saved: false,
        };
        let saved = GrokAccount {
            id: "user-b".into(),
            is_active: false,
            is_saved: true,
            ..current.clone()
        };
        let menu = grok_accounts_menu(&[current, saved], Language::English, false);
        assert_eq!(menu.id.as_deref(), Some("grok_accounts"));
        assert_eq!(menu.children[0].checked, Some(true));
        assert!(menu.children[0].disabled);
        assert_eq!(
            menu.children[1].id.as_deref(),
            Some("switch_grok_account:user-b")
        );
        assert!(!menu.children[1].disabled);
        assert!(menu_contains(&menu.children, "add_grok_account"));
        assert!(menu_contains(
            &grok_accounts_menu(&[], Language::English, false).children,
            "add_grok_account"
        ));
    }

    fn claude_menu_account(id: &str) -> codexbar::providers::claude::accounts::ClaudeAccount {
        codexbar::providers::claude::accounts::ClaudeAccount {
            id: id.into(),
            email: format!("{id}@example.com"),
            organization: None,
            plan: None,
            is_active: false,
            is_saved: true,
        }
    }

    #[test]
    fn hidden_claude_and_grok_tray_labels_show_no_email() {
        use codexbar::providers::claude::accounts::ClaudeAccount;
        use codexbar::providers::grok::accounts::GrokAccount;
        let claude = ClaudeAccount {
            id: "a:org".into(),
            email: "proof@example.com".into(),
            organization: Some("proof@example.com's Organization".into()),
            plan: None,
            is_active: true,
            is_saved: true,
        };
        let claude_other = ClaudeAccount {
            id: "b:org".into(),
            email: "second@example.com".into(),
            is_active: false,
            ..claude.clone()
        };
        let grok = GrokAccount {
            id: "user-a".into(),
            email: "proof@example.com".into(),
            organization: None,
            plan: None,
            is_active: true,
            is_saved: true,
        };
        let claude_menu = claude_accounts_menu(
            &[claude, claude_other],
            Language::English,
            true,
            true,
            &HashMap::new(),
        );
        let grok_menu = grok_accounts_menu(&[grok], Language::English, true);
        let labels: Vec<&str> = claude_menu.children[..2]
            .iter()
            .chain(&grok_menu.children[..1])
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["Account 1", "Account 2", "Account 1"]);

        // An unsaved live login is listed first; numbering follows the list,
        // so the first row reads "Account 1" and the saved rows follow it.
        let live = ClaudeAccount {
            is_active: true,
            is_saved: false,
            ..claude_menu_account("0:org")
        };
        let reordered = claude_accounts_menu(
            &[
                live,
                claude_menu_account("b:org"),
                claude_menu_account("a:org"),
            ],
            Language::English,
            true,
            true,
            &HashMap::new(),
        );
        let by_id: Vec<(&str, &str)> = reordered.children[..3]
            .iter()
            .map(|entry| (entry.id.as_deref().unwrap_or(""), entry.label.as_str()))
            .collect();
        assert_eq!(
            by_id,
            [
                ("switch_claude_account:0:org", "Account 1"),
                ("switch_claude_account:b:org", "Account 2"),
                ("switch_claude_account:a:org", "Account 3"),
            ]
        );
        let grok_reordered = grok_accounts_menu(
            &[
                GrokAccount {
                    id: "user-z".into(),
                    email: "z@example.com".into(),
                    organization: None,
                    plan: None,
                    is_active: true,
                    is_saved: false,
                },
                GrokAccount {
                    id: "user-a".into(),
                    email: "a@example.com".into(),
                    organization: None,
                    plan: None,
                    is_active: false,
                    is_saved: true,
                },
            ],
            Language::English,
            true,
        );
        let grok_labels: Vec<&str> = grok_reordered.children[..2]
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(grok_labels, ["Account 1", "Account 2"]);
        for entry in claude_menu.children.iter().chain(&grok_menu.children) {
            assert!(!entry.label.contains('@'), "{}", entry.label);
            assert!(!entry.label.contains("example.com"), "{}", entry.label);
        }
    }
}
