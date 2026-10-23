//! Proof-only readback of the native tray context menu.
//!
//! Automation can't open the tray menu without a right-click, and a right-click
//! needs `SetForegroundWindow`, which steals the user's focus. In proof mode,
//! `CODEXBAR_PROOF_TRAY_MENU_DUMP=<absolute path>` makes the shell write the
//! live menu as JSON after every menu rebuild. The matching activation command
//! is `tray_bridge::proof_activate_tray_item`; it accepts only enabled rows of
//! the menu currently installed on the tray.
//!
//! Nothing here runs unless proof mode is active and the env var is set.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::menu::{Menu, MenuItemKind};
use tauri::{AppHandle, Wry};

#[cfg(test)]
use crate::tray_menu::TrayMenuEntry;

pub(crate) const DUMP_ENV_VAR: &str = "CODEXBAR_PROOF_TRAY_MENU_DUMP";

/// Generation of the last dump written by this process. It is held across the
/// file write so generation order matches file order. Never held while the
/// menu is read, because those reads may wait on the main thread.
static GENERATION: Mutex<u64> = Mutex::new(0);

/// Enabled leaf ids of the menu last installed on the tray. Proof activation
/// dispatches only these, so it can't act on a row the menu doesn't show.
static INSTALLED_IDS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Top-level dump document.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TrayMenuDump {
    pub(crate) generation: u64,
    /// RFC 3339 UTC time of the write.
    pub(crate) timestamp: String,
    pub(crate) items: Vec<DumpNode>,
}

/// One menu row. `kind` is `item`, `check`, `submenu`, `separator` or `icon`.
/// `checked` is `null` unless `kind` is `check`. `id` is `null` for separators.
/// `children` is empty unless `kind` is `submenu`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DumpNode {
    pub(crate) id: Option<String>,
    pub(crate) text: String,
    pub(crate) kind: &'static str,
    pub(crate) enabled: bool,
    pub(crate) checked: Option<bool>,
    pub(crate) children: Vec<DumpNode>,
}

/// Dump path for this process, or `None` when the dump is off.
///
/// Outside proof mode the dump is off. A relative path is rejected, because it
/// would resolve against an unpredictable working directory.
pub(crate) fn dump_target(proof_mode: bool, raw: &str) -> Option<PathBuf> {
    if !proof_mode {
        return None;
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        tracing::warn!("{} must be an absolute path; ignoring it", DUMP_ENV_VAR);
        return None;
    }
    Some(path)
}

/// Dump nodes built from the menu spec. This mirrors how `tray_bridge` turns
/// each `TrayMenuEntry` into a native item, so the spec tests can pin the format.
#[cfg(test)]
pub(crate) fn nodes_from_spec(entries: &[TrayMenuEntry]) -> Vec<DumpNode> {
    entries.iter().map(node_from_spec).collect()
}

#[cfg(test)]
fn node_from_spec(entry: &TrayMenuEntry) -> DumpNode {
    if entry.is_separator {
        return separator_node();
    }
    let kind = if !entry.children.is_empty() {
        "submenu"
    } else if entry.checked.is_some() {
        "check"
    } else {
        "item"
    };
    DumpNode {
        id: entry.id.clone(),
        text: entry.label.clone(),
        kind,
        enabled: !entry.disabled,
        checked: entry.checked,
        children: nodes_from_spec(&entry.children),
    }
}

/// Dump nodes read back from the live native menu. Text and state come from the
/// tauri objects, so the dump shows what is actually set on the tray.
pub(crate) fn nodes_from_native(items: &[MenuItemKind<Wry>]) -> tauri::Result<Vec<DumpNode>> {
    items.iter().map(node_from_native).collect()
}

fn node_from_native(item: &MenuItemKind<Wry>) -> tauri::Result<DumpNode> {
    Ok(match item {
        MenuItemKind::MenuItem(item) => DumpNode {
            id: Some(item.id().as_ref().to_string()),
            text: item.text()?,
            kind: "item",
            enabled: item.is_enabled()?,
            checked: None,
            children: Vec::new(),
        },
        MenuItemKind::Check(item) => DumpNode {
            id: Some(item.id().as_ref().to_string()),
            text: item.text()?,
            kind: "check",
            enabled: item.is_enabled()?,
            checked: Some(item.is_checked()?),
            children: Vec::new(),
        },
        MenuItemKind::Submenu(submenu) => DumpNode {
            id: Some(submenu.id().as_ref().to_string()),
            text: submenu.text()?,
            kind: "submenu",
            enabled: submenu.is_enabled()?,
            checked: None,
            children: nodes_from_native(&submenu.items()?)?,
        },
        MenuItemKind::Icon(item) => DumpNode {
            id: Some(item.id().as_ref().to_string()),
            text: item.text()?,
            kind: "icon",
            enabled: item.is_enabled()?,
            checked: None,
            children: Vec::new(),
        },
        MenuItemKind::Predefined(_) => separator_node(),
    })
}

fn separator_node() -> DumpNode {
    DumpNode {
        id: None,
        text: String::new(),
        kind: "separator",
        enabled: true,
        checked: None,
        children: Vec::new(),
    }
}

/// Ids of the enabled leaf rows (items, checks, icons) in `nodes`, at any depth.
/// Submenus, separators and disabled rows such as status lines are left out.
pub(crate) fn activatable_ids(nodes: &[DumpNode]) -> BTreeSet<String> {
    fn collect(nodes: &[DumpNode], ids: &mut BTreeSet<String>) {
        for node in nodes {
            if node.kind == "submenu" {
                collect(&node.children, ids);
            } else if node.enabled
                && let Some(id) = &node.id
            {
                ids.insert(id.clone());
            }
        }
    }
    let mut ids = BTreeSet::new();
    collect(nodes, &mut ids);
    ids
}

/// Whether `id` is an enabled leaf of the menu last installed on the tray.
pub(crate) fn is_installed_leaf(id: &str) -> bool {
    INSTALLED_IDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(id)
}

/// Record the menu just installed on the tray and, when the env var is set,
/// write it to the dump path. Call it on the main thread right after `set_menu`
/// succeeds: the main thread runs installs one at a time, so each record
/// matches the menu on the tray. Does nothing outside proof mode.
pub(crate) fn record_installed(app: &AppHandle, menu: &Menu<Wry>) {
    if !crate::proof_harness::is_proof_mode(app) {
        return;
    }
    let items = match menu.items().and_then(|items| nodes_from_native(&items)) {
        Ok(items) => items,
        Err(error) => {
            *INSTALLED_IDS.lock().unwrap_or_else(PoisonError::into_inner) = BTreeSet::new();
            tracing::warn!(%error, "proof: failed to read the tray menu");
            return;
        }
    };
    *INSTALLED_IDS.lock().unwrap_or_else(PoisonError::into_inner) = activatable_ids(&items);
    let Some(raw) = std::env::var(DUMP_ENV_VAR).ok() else {
        return;
    };
    let Some(path) = dump_target(true, &raw) else {
        return;
    };
    let mut generation = GENERATION.lock().unwrap_or_else(PoisonError::into_inner);
    *generation += 1;
    let dump = TrayMenuDump {
        generation: *generation,
        timestamp: chrono::Utc::now().to_rfc3339(),
        items,
    };
    if let Err(error) = write_atomic(&path, &dump) {
        tracing::warn!(%error, "proof: failed to write the tray menu dump");
    }
}

/// Write the dump to a temp file beside `path`, then rename it over `path`, so
/// a reader sees either the previous dump or the new one.
fn write_atomic(path: &Path, dump: &TrayMenuDump) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(dump).map_err(io::Error::other)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::other("dump path has no file name"))?;
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray_menu::tests::sample_provider_catalog;
    use crate::tray_menu::{MenuState, build_tray_menu_with};
    use codexbar::settings::Language;
    use serde_json::json;

    #[test]
    fn dump_target_requires_proof_mode_and_an_absolute_path() {
        let absolute = if cfg!(windows) {
            r"C:\proof\tray-menu.json"
        } else {
            "/tmp/tray-menu.json"
        };
        assert_eq!(dump_target(false, absolute), None);
        assert_eq!(dump_target(true, ""), None);
        assert_eq!(dump_target(true, "   "), None);
        assert_eq!(dump_target(true, "relative/tray-menu.json"), None);
        assert_eq!(dump_target(true, absolute), Some(PathBuf::from(absolute)));
    }

    #[test]
    fn dump_serializes_each_row_kind_with_checked_state() {
        let items = vec![
            TrayMenuEntry::status_row("status_claude", "Claude 60%"),
            TrayMenuEntry::separator(),
            TrayMenuEntry::item("refresh", "Refresh all"),
            TrayMenuEntry::check_item("toggle_float_bar", "Show Float Bar", true),
            TrayMenuEntry::submenu(
                "providers",
                "Providers",
                vec![TrayMenuEntry::check_item(
                    "toggle_provider:codex",
                    "Codex",
                    false,
                )],
            ),
        ];
        let dump = TrayMenuDump {
            generation: 7,
            timestamp: "2026-01-02T03:04:05+00:00".into(),
            items: nodes_from_spec(&items),
        };
        assert_eq!(
            serde_json::to_value(&dump).unwrap(),
            json!({
                "generation": 7,
                "timestamp": "2026-01-02T03:04:05+00:00",
                "items": [
                    {"id": "status_claude", "text": "Claude 60%", "kind": "item",
                     "enabled": false, "checked": null, "children": []},
                    {"id": null, "text": "", "kind": "separator",
                     "enabled": true, "checked": null, "children": []},
                    {"id": "refresh", "text": "Refresh all", "kind": "item",
                     "enabled": true, "checked": null, "children": []},
                    {"id": "toggle_float_bar", "text": "Show Float Bar", "kind": "check",
                     "enabled": true, "checked": true, "children": []},
                    {"id": "providers", "text": "Providers", "kind": "submenu",
                     "enabled": true, "checked": null, "children": [
                        {"id": "toggle_provider:codex", "text": "Codex", "kind": "check",
                         "enabled": true, "checked": false, "children": []}
                     ]}
                ]
            })
        );
    }

    #[test]
    fn activatable_ids_are_enabled_leaves_at_any_depth() {
        let items = vec![
            TrayMenuEntry::status_row("status_claude", "Claude 60%"),
            TrayMenuEntry::separator(),
            TrayMenuEntry::item("refresh", "Refresh all"),
            TrayMenuEntry::submenu(
                "providers",
                "Providers",
                vec![TrayMenuEntry::check_item(
                    "toggle_provider:codex",
                    "Codex",
                    false,
                )],
            ),
        ];
        let ids: Vec<String> = activatable_ids(&nodes_from_spec(&items))
            .into_iter()
            .collect();
        assert_eq!(ids, vec!["refresh", "toggle_provider:codex"]);
    }

    #[test]
    fn dump_of_built_menu_lists_status_rows_then_actions() {
        let labels = vec![("claude".to_string(), "Claude 60%".to_string())];
        let enabled = ["claude".to_string()].into_iter().collect();
        let spec = build_tray_menu_with(
            &sample_provider_catalog(),
            &labels,
            &enabled,
            MenuState {
                float_bar_enabled: true,
                ..MenuState::default()
            },
            Language::English,
        );
        let items = nodes_from_spec(&spec);
        let ids: Vec<Option<&str>> = items.iter().map(|node| node.id.as_deref()).collect();
        assert_eq!(
            ids,
            vec![
                Some("status_claude"),
                None,
                Some("refresh"),
                Some("pop_out"),
                Some("toggle_float_bar"),
                None,
                Some("providers"),
                None,
                Some("settings"),
                Some("check_for_updates"),
                Some("about"),
                None,
                Some("quit"),
            ]
        );
        let providers = items
            .iter()
            .find(|node| node.id.as_deref() == Some("providers"))
            .expect("providers submenu");
        assert_eq!(providers.kind, "submenu");
        let claude = providers
            .children
            .iter()
            .find(|node| node.id.as_deref() == Some("toggle_provider:claude"))
            .expect("claude toggle");
        assert_eq!(claude.kind, "check");
        assert_eq!(claude.checked, Some(true));
        let codex = providers
            .children
            .iter()
            .find(|node| node.id.as_deref() == Some("toggle_provider:codex"))
            .expect("codex toggle");
        assert_eq!(codex.checked, Some(false));
    }
}
