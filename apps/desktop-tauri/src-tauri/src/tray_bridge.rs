//! System tray icon setup: left-click opens the tray panel, right-click native menu.

use std::sync::Mutex;

use crate::commands::ProviderCatalogEntry;
use codexbar::settings::Settings;
use tauri::image::Image;
use tauri::menu::{CheckMenuItemBuilder, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::shell;
use crate::state::{AppState, TrayAnchor};
use crate::tray_menu::{MenuState, TrayMenuEntry, build_tray_menu_with};
use crate::tray_presentation::TrayPresentationPlan;

#[derive(Debug, Clone, Copy)]
struct MonitorScaleInfo {
    physical_x: i32,
    physical_y: i32,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f64,
}

impl MonitorScaleInfo {
    fn from_monitor(monitor: &tauri::Monitor) -> Self {
        let scale_factor = monitor.scale_factor();
        let safe_scale = crate::window_positioner::safe_scale(scale_factor);
        let position = monitor.position();
        let size = monitor.size();

        Self {
            physical_x: position.x,
            physical_y: position.y,
            physical_width: size.width,
            physical_height: size.height,
            scale_factor: safe_scale,
        }
    }
}

fn scale_factor_for_physical_point(x: f64, y: f64, monitors: &[MonitorScaleInfo]) -> Option<f64> {
    monitors
        .iter()
        .find(|monitor| {
            x >= monitor.physical_x as f64
                && x < (monitor.physical_x + monitor.physical_width as i32) as f64
                && y >= monitor.physical_y as f64
                && y < (monitor.physical_y + monitor.physical_height as i32) as f64
        })
        .map(|monitor| monitor.scale_factor)
}

fn logical_to_physical_anchor(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale_factor: f64,
) -> TrayAnchor {
    let safe_scale = crate::window_positioner::safe_scale(scale_factor);

    TrayAnchor {
        x: (x * safe_scale).round() as i32,
        y: (y * safe_scale).round() as i32,
        width: ((width * safe_scale).round().max(1.0)) as u32,
        height: ((height * safe_scale).round().max(1.0)) as u32,
    }
}

fn resolve_tray_anchor(
    rect: &tauri::Rect,
    click_position: tauri::PhysicalPosition<f64>,
    monitors: &[MonitorScaleInfo],
) -> Option<TrayAnchor> {
    let click_scale = scale_factor_for_physical_point(click_position.x, click_position.y, monitors);

    match (rect.position, rect.size) {
        (tauri::Position::Physical(position), tauri::Size::Physical(size)) => Some(TrayAnchor {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }),
        (tauri::Position::Logical(position), tauri::Size::Logical(size)) => {
            click_scale.map(|scale| {
                logical_to_physical_anchor(position.x, position.y, size.width, size.height, scale)
            })
        }
        (tauri::Position::Physical(position), tauri::Size::Logical(size)) => {
            click_scale.map(|scale| TrayAnchor {
                x: position.x,
                y: position.y,
                width: ((size.width * scale).round().max(1.0)) as u32,
                height: ((size.height * scale).round().max(1.0)) as u32,
            })
        }
        (tauri::Position::Logical(position), tauri::Size::Physical(size)) => {
            click_scale.map(|scale| TrayAnchor {
                x: (position.x * scale).round() as i32,
                y: (position.y * scale).round() as i32,
                width: size.width,
                height: size.height,
            })
        }
    }
}

fn build_native_tray_menu(
    app: &AppHandle,
    providers: &[ProviderCatalogEntry],
    status_labels: &[(String, String)],
) -> tauri::Result<Menu<tauri::Wry>> {
    let settings = Settings::load();
    let enabled = settings.enabled_providers.clone();
    let mut spec = build_tray_menu_with(
        providers,
        status_labels,
        &enabled,
        MenuState {
            float_bar_enabled: settings.float_bar_enabled,
            stay_awake_held: crate::stay_awake::is_held(),
        },
        settings.ui_language,
    );
    // Callers release the AppState lock before building the tray menu.
    let claude_usage = app
        .try_state::<Mutex<AppState>>()
        .and_then(|state| {
            state
                .lock()
                .ok()
                .map(|guard| guard.claude_account_usage.clone())
        })
        .unwrap_or_default();
    crate::tray_accounts::prepend_account_menus(&mut spec, &settings, &claude_usage);
    let entries = spec
        .iter()
        .map(|entry| build_native_menu_entry(app, entry))
        .collect::<tauri::Result<Vec<_>>>()?;
    let item_refs = entries
        .iter()
        .map(NativeMenuEntry::as_item)
        .collect::<Vec<_>>();

    Menu::with_items(app, &item_refs)
}

pub(crate) enum MenuAction {
    /// Open Settings/About in a detached window.
    OpenSettings(String),
    /// Open (or focus) the dedicated flyout ("Pop Out Dashboard") window.
    OpenFlyout,
    Refresh,
    CheckForUpdates,
    /// Toggle the enabled/disabled state of the provider with the given CLI name.
    ToggleProvider(String),
    /// Toggle the floating bar window on/off.
    ToggleFloatBar,
    Account(crate::tray_accounts::AccountMenuAction),
    Quit,
}

pub(crate) fn resolve_menu_action(id: &str) -> Option<MenuAction> {
    if let Some(action) = crate::tray_accounts::resolve_action(id) {
        return Some(MenuAction::Account(action));
    }
    match id {
        "refresh" => Some(MenuAction::Refresh),
        "check_for_updates" => Some(MenuAction::CheckForUpdates),
        "quit" => Some(MenuAction::Quit),
        "settings" => Some(MenuAction::OpenSettings("general".into())),
        "about" => Some(MenuAction::OpenSettings("about".into())),
        "toggle_float_bar" => Some(MenuAction::ToggleFloatBar),
        "pop_out" => Some(MenuAction::OpenFlyout),
        _ if id.starts_with("toggle_provider:") => {
            let provider_id = id["toggle_provider:".len()..].to_string();
            Some(MenuAction::ToggleProvider(provider_id))
        }
        _ => None,
    }
}

/// Store the tray icon bounds from a click event into shared state.
fn store_anchor(app: &AppHandle, rect: &tauri::Rect, click_position: tauri::PhysicalPosition<f64>) {
    let monitors = app
        .get_webview_window("main")
        .and_then(|window| window.available_monitors().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|monitor| MonitorScaleInfo::from_monitor(&monitor))
        .collect::<Vec<_>>();

    let Some(anchor) = resolve_tray_anchor(rect, click_position, &monitors) else {
        return;
    };

    if let Some(st) = app.try_state::<Mutex<AppState>>() {
        let mut guard = st.lock().unwrap();
        guard.tray_anchor = Some(anchor);
    }
}

/// Initialise the system tray icon, context menu, and event handlers.
///
/// - **Left-click** toggles the custom tray panel via the surface state machine.
/// - **Right-click** opens the native context menu with shell actions.
pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let menu = build_native_tray_menu(app.handle(), &crate::commands::get_provider_catalog(), &[])?;

    // Embed the icon at compile time so it works regardless of working directory.
    let icon_bytes = include_bytes!("../../../../rust/icons/icon.png");
    let icon = Image::from_bytes(icon_bytes)?;

    let _tray = TrayIconBuilder::with_id("codexbar-main")
        .icon(icon)
        .tooltip("CodexBar Desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                position,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    store_anchor(app, &rect, position);
                    // Left-click toggles the tray-panel flyout window (Pop Out
                    // Dashboard): open it, or cleanly close it when this same
                    // click already blur-dismissed it (no open→close flicker).
                    // The flyout is the only dashboard layout; the legacy
                    // PopOut layout on `main` is retired. Called directly (not spawned): native
                    // tray-icon event callbacks run on the same main-thread
                    // event-loop context as `on_menu_event` below, where
                    // `settings_window::open_or_focus` is also called
                    // synchronously — the WebviewWindowBuilder deadlock only
                    // affects builds invoked from *synchronous Tauri IPC
                    // commands*, not native event-loop callbacks.
                    shell::flyout_window::toggle_with_blur_consume(app, None);
                }
            }
        })
        .on_menu_event(|app, event| {
            handle_menu_event(app, event.id().as_ref());
        })
        .build(app)?;
    crate::proof_tray_menu::record_installed(app.handle(), &menu);

    // Apply tray promotion on startup. The NotifyIconSettings entry is created
    // by Windows only after the icon is first registered, so on first run /
    // post-upgrade the subkey may not exist yet — apply_promotion tolerates
    // EntryNotFound. Retry a few times while explorer finishes registration.
    schedule_tray_promotion_retries(app.handle().clone());

    Ok(())
}

/// Re-apply Win11 tray promotion a few times after startup.
///
/// Windows often creates the NotifyIconSettings subkey only after the first
/// successful NIM_ADD (and sometimes only after the icon is refreshed). A
/// single immediate write is not enough after upgrades.
fn schedule_tray_promotion_retries(app_handle: AppHandle) {
    if !codexbar::settings::Settings::load().promote_tray_icon {
        return;
    }
    crate::tray_visibility::apply_promotion(true);
    tauri::async_runtime::spawn(async move {
        for secs in [1_u64, 3, 8] {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            if !codexbar::settings::Settings::load().promote_tray_icon {
                break;
            }
            crate::tray_visibility::apply_promotion(true);
        }
        drop(app_handle);
    });
}

/// Route a native menu-item click to the corresponding shell action.
fn handle_menu_event(app: &AppHandle, id: &str) {
    match resolve_menu_action(id) {
        Some(MenuAction::Account(action)) => crate::tray_accounts::handle_action(app, action),
        Some(MenuAction::OpenSettings(tab)) => {
            let _ = shell::settings_window::open_or_focus(app, &tab);
        }
        Some(MenuAction::OpenFlyout) => {
            // Pass None: open_or_focus falls back to the tray-anchored
            // default position (same placement chain the old TrayPanel
            // transition used) when no explicit position is given.
            crate::auto_refresh::note_menu_open();
            let _ = shell::flyout_window::open_or_focus(
                app,
                None,
                shell::activation::Activation::UserAction,
            );
        }
        Some(MenuAction::Refresh) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::commands::do_refresh_providers(&handle).await;
            });
        }
        Some(MenuAction::CheckForUpdates) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<Mutex<AppState>>();
                let _ = crate::commands::check_for_updates(handle.clone(), state).await;
            });
        }
        Some(MenuAction::ToggleProvider(provider_id)) => {
            let write_guard = crate::commands::lock_settings_writes();
            let mut settings = Settings::load();
            if settings.enabled_providers.contains(&provider_id) {
                settings.enabled_providers.remove(&provider_id);
            } else {
                settings.enabled_providers.insert(provider_id);
            }
            let saved = settings.save();
            drop(write_guard);
            if saved.is_err() {
                rebuild_tray_menu(app);
                return;
            }
            // Same follow-up as `update_settings`: without the prune and the
            // broadcast, open surfaces keep the disabled provider's card.
            let enabled_ids = settings.get_enabled_provider_ids();
            if let Some(state) = app.try_state::<Mutex<AppState>>() {
                let _ = crate::commands::invalidate_provider_refresh_and_prune_disabled(
                    &state,
                    &enabled_ids,
                );
            }
            crate::floatbar::notify_settings_changed(app);
            crate::status_poller::settings_changed(app, &settings, false);
            refresh_tray_presentation(app);
            crate::events::emit_settings_changed(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::commands::do_refresh_providers(&handle).await;
            });
        }
        Some(MenuAction::ToggleFloatBar) => {
            crate::floatbar::toggle(app);
            rebuild_tray_menu(app);
        }
        Some(MenuAction::Quit) => {
            app.exit(0);
        }
        None => {}
    }
}

/// Pure gate for `proof_activate_tray_item`: proof mode only, only ids that
/// resolve to a menu action, and only enabled rows of the installed menu.
/// Status rows, unknown ids and rows the menu doesn't show never dispatch.
pub(crate) fn activation_allowed(proof_mode: bool, id: &str, in_installed_menu: bool) -> bool {
    proof_mode && in_installed_menu && resolve_menu_action(id).is_some()
}

const NOT_ACTIVATABLE: &str = "tray item is not a proof-activatable menu action";

/// Proof-only: run the same handler a native click on `id` runs.
///
/// The check and the handler run on the main thread through
/// `run_on_main_thread`, not in this command: menu installs also run there, so
/// the check sees the menu that is on the tray, and window-building actions
/// stay out of an IPC command.
#[tauri::command]
pub(crate) async fn proof_activate_tray_item(app: AppHandle, id: String) -> Result<(), String> {
    let proof_mode = crate::proof_harness::is_proof_mode(&app);
    if !proof_mode {
        return Err(NOT_ACTIVATABLE.into());
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let in_menu = crate::proof_tray_menu::is_installed_leaf(&id);
        let allowed = activation_allowed(proof_mode, &id, in_menu);
        if allowed {
            handle_menu_event(&handle, &id);
        }
        let _ = sender.send(allowed);
    })
    .map_err(|error| error.to_string())?;
    match receiver.await {
        Ok(true) => Ok(()),
        Ok(false) => Err(NOT_ACTIVATABLE.into()),
        Err(_) => Err("tray item activation did not run".into()),
    }
}

/// Install `menu` on the tray from the main thread. The event loop runs these
/// tasks in order and each one records its own menu, so a later install is
/// never followed by an earlier proof snapshot. Runs inline when already on
/// the main thread.
fn install_tray_menu(app: &AppHandle, menu: Menu<tauri::Wry>) {
    let handle = app.clone();
    let posted = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id("codexbar-main") else {
            return;
        };
        match tray.set_menu(Some(menu.clone())) {
            Ok(()) => crate::proof_tray_menu::record_installed(&handle, &menu),
            Err(error) => tracing::warn!("Failed to install tray menu: {error}"),
        }
    });
    if let Err(error) = posted {
        tracing::warn!("Failed to schedule the tray menu install: {error}");
    }
}

/// Rebuild the native tray menu from current provider + settings state.
pub(crate) fn rebuild_tray_menu(app: &AppHandle) {
    let catalog = crate::commands::get_provider_catalog();
    let settings = Settings::load();
    let rates_cache = app
        .try_state::<crate::commands::CurrencyRateCache>()
        .map(|state| state.inner());
    let status_labels = if let Some(st) = app.try_state::<Mutex<AppState>>() {
        let guard = st.lock().unwrap();
        TrayPresentationPlan::resolve(&settings, &guard.provider_cache, rates_cache)
            .status_labels(settings.ui_language)
    } else {
        vec![]
    };
    if let Ok(menu) = build_native_tray_menu(app, &catalog, &status_labels) {
        install_tray_menu(app, menu);
    }
}

/// Rebuild the tray menu with current provider status labels after a refresh cycle.
pub fn update_tray_status_items(
    app: &AppHandle,
    snapshots: &[crate::commands::ProviderUsageSnapshot],
) {
    let catalog = crate::commands::get_provider_catalog();
    let settings = Settings::load();
    let rates_cache = app
        .try_state::<crate::commands::CurrencyRateCache>()
        .map(|state| state.inner());
    let status_labels = TrayPresentationPlan::resolve(&settings, snapshots, rates_cache)
        .status_labels(settings.ui_language);

    if let Ok(menu) = build_native_tray_menu(app, &catalog, &status_labels) {
        install_tray_menu(app, menu);
    }
}

/// Refresh every native tray surface that depends on settings and cached provider data.
pub(crate) fn refresh_tray_presentation(app: &AppHandle) {
    let snapshots = app
        .try_state::<Mutex<AppState>>()
        .map(|st| st.lock().unwrap().provider_cache.clone())
        .unwrap_or_default();
    update_tray_status_items(app, &snapshots);
    update_tray_icon_and_tooltip(app, &snapshots);
}

/// Update the tray icon pixels and tooltip text to reflect current provider usage.
///
/// Behaviour mirrors egui's `choose_tray_update_plan` (rust/src/native_ui/app.rs):
/// - If `menu_bar_shows_highest_usage` is on (legacy Display mode "minimal" is
///   folded into it on load), render the bar from the healthy provider with the highest session usage.
/// - Otherwise render from the first enabled healthy provider (catalog order).
/// - When any provider exposes a weekly/secondary window, the icon shows both
///   bars from the same picked provider.
/// - With zero healthy providers but at least one error, fall back to an
///   error-styled icon using the last known max percentage so the tray
///   still communicates "something is wrong".
pub fn update_tray_icon_and_tooltip(
    app: &AppHandle,
    snapshots: &[crate::commands::ProviderUsageSnapshot],
) {
    let Some(tray) = app.tray_by_id("codexbar-main") else {
        return;
    };

    let settings = Settings::load();
    let rates_cache = app
        .try_state::<crate::commands::CurrencyRateCache>()
        .map(|state| state.inner());
    let plan = TrayPresentationPlan::resolve(&settings, snapshots, rates_cache);
    let (rgba, w, h) = plan.render_icon();
    let icon = Image::new_owned(rgba, w, h);
    let _ = tray.set_icon(Some(icon));

    let tooltip = build_tooltip(
        snapshots,
        &settings,
        settings.ui_language,
        settings.hide_personal_info,
    );
    let tooltip = crate::status_poller::with_incident_lines(app, &settings, tooltip);
    let tooltip = fit_tooltip(&tooltip, TRAY_TOOLTIP_MAX_UTF16);
    let _ = tray.set_tooltip(Some(tooltip));
}

/// Build a compact multi-line tooltip string from provider snapshots.
fn build_tooltip(
    snapshots: &[crate::commands::ProviderUsageSnapshot],
    settings: &Settings,
    lang: codexbar::settings::Language,
    hide_personal_info: bool,
) -> String {
    use codexbar::locale::{LocaleKey, get_text};

    if snapshots.is_empty() {
        return "CodexBar Desktop".to_string();
    }

    let error_label = get_text(lang, LocaleKey::TrayStatusRowError);
    let mut lines = Vec::with_capacity(snapshots.len() + 1);
    for s in snapshots {
        let status = if let Some(ref err) = s.error {
            // Provider errors can quote the signed-in email.
            let err = codexbar::core::PersonalInfoRedactor::hide_emails_in_text(
                Some(err),
                hide_personal_info,
            )
            .unwrap_or_default()
            // A newline would split one status row into two tooltip lines.
            .replace(['\r', '\n'], " ");
            let short = truncate_tooltip_text(&err, 36);
            format!("{}: {} ({})", s.display_name, error_label, short)
        } else {
            let label = crate::tray_presentation::metric_status_label(s, settings, lang);
            format!("{}: {}", s.display_name, truncate_tooltip_text(&label, 42))
        };
        lines.push(status);
    }

    format!("CodexBar\n{}", lines.join("\n"))
}

/// `NOTIFYICONDATAW.szTip` holds 128 UTF-16 units, but tray-icon 0.21 leaves
/// `cbSize` zeroed, so Explorer reads the pre-Windows 2000 layout and shows
/// only the first 64 units, cutting mid-line without a marker. 63 leaves room
/// for the terminating NUL of that legacy `szTip[64]`.
const TRAY_TOOLTIP_MAX_UTF16: usize = 63;

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Longest char-boundary prefix of `text` that fits in `budget` UTF-16 units.
/// A surrogate pair is never split.
fn take_utf16(text: &str, budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if used + width > budget {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

fn truncate_tooltip_text(text: &str, max_units: usize) -> String {
    if utf16_len(text) <= max_units {
        return text.to_string();
    }
    format!("{}...", take_utf16(text, max_units))
}

/// Fits a newline-separated tooltip into `max_units` UTF-16 units. Whole lines
/// are kept in order (the title, then incident lines, then provider lines).
/// When lines are dropped, the last kept line ends with "…" (a symbol, so no
/// new localized string is needed). A single line that is too long is cut
/// with "…".
fn fit_tooltip(tooltip: &str, max_units: usize) -> String {
    if utf16_len(tooltip) <= max_units {
        return tooltip.to_string();
    }
    let lines: Vec<&str> = tooltip.split('\n').collect();

    // Every candidate line reserves one unit for the "…" that would follow it
    // when more lines come after it. The reserved unit is always available.
    let mut kept = 0;
    let mut used = 0;
    for (index, line) in lines.iter().enumerate() {
        let separator = usize::from(kept > 0);
        let reserve = usize::from(index + 1 < lines.len());
        let cost = separator + utf16_len(line);
        if used + cost + reserve > max_units {
            break;
        }
        used += cost;
        kept += 1;
    }

    if kept == 0 {
        let budget = max_units.saturating_sub(1);
        return format!("{}…", take_utf16(lines[0], budget));
    }

    let mut out: Vec<String> = lines[..kept].iter().map(|line| line.to_string()).collect();
    let last = out.len() - 1;
    out[last].push('…');
    out.join("\n")
}

enum NativeMenuEntry {
    Item(MenuItem<tauri::Wry>),
    CheckItem(tauri::menu::CheckMenuItem<tauri::Wry>),
    Submenu(Submenu<tauri::Wry>),
    Separator(PredefinedMenuItem<tauri::Wry>),
}

impl NativeMenuEntry {
    fn as_item(&self) -> &dyn IsMenuItem<tauri::Wry> {
        match self {
            Self::Item(item) => item,
            Self::CheckItem(item) => item,
            Self::Submenu(item) => item,
            Self::Separator(item) => item,
        }
    }
}

fn build_native_menu_entry(
    app: &AppHandle,
    entry: &TrayMenuEntry,
) -> tauri::Result<NativeMenuEntry> {
    if entry.is_separator {
        return Ok(NativeMenuEntry::Separator(PredefinedMenuItem::separator(
            app,
        )?));
    }

    if !entry.children.is_empty() {
        let children = entry
            .children
            .iter()
            .map(|child| build_native_menu_entry(app, child))
            .collect::<tauri::Result<Vec<_>>>()?;
        let child_refs = children
            .iter()
            .map(NativeMenuEntry::as_item)
            .collect::<Vec<_>>();

        return Ok(NativeMenuEntry::Submenu(Submenu::with_items(
            app,
            &entry.label,
            true,
            &child_refs,
        )?));
    }

    // Render as a checkbox item when `checked` is set.
    if let Some(checked) = entry.checked {
        return Ok(NativeMenuEntry::CheckItem(
            CheckMenuItemBuilder::with_id(entry.id.clone().unwrap_or_default(), &entry.label)
                .enabled(!entry.disabled)
                .checked(checked)
                .build(app)?,
        ));
    }

    Ok(NativeMenuEntry::Item(MenuItem::with_id(
        app,
        entry.id.clone().unwrap_or_default(),
        &entry.label,
        !entry.disabled,
        None::<&str>,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::surface::SurfaceMode;
    use crate::tray_menu::tests::{actionable_ids, flatten, sample_provider_catalog};
    use crate::tray_presentation::tests::{fake_snapshot, fake_snapshot_with};
    use codexbar::settings::Language;

    #[test]
    fn proof_activation_requires_proof_mode_a_menu_action_and_an_installed_row() {
        assert!(!activation_allowed(false, "refresh", true));
        assert!(activation_allowed(true, "refresh", true));
        assert!(activation_allowed(true, "settings", true));
        assert!(activation_allowed(true, "toggle_provider:codex", true));
        assert!(activation_allowed(
            true,
            "switch_claude_account:a:org",
            true
        ));
        assert!(!activation_allowed(true, "status_codex", true));
        assert!(!activation_allowed(true, "stay_awake_active", true));
        assert!(!activation_allowed(true, "not_a_menu_item", true));
        assert!(!activation_allowed(true, "", true));
        assert!(!activation_allowed(true, "refresh", false));
        assert!(!activation_allowed(true, "toggle_provider:unlisted", false));
    }

    #[test]
    fn every_spec_action_resolves_and_status_rows_do_not() {
        let labels = vec![
            ("claude".to_string(), "Claude 60%".to_string()),
            ("codex".to_string(), "Codex 30%".to_string()),
        ];
        let spec = build_tray_menu_with(
            &sample_provider_catalog(),
            &labels,
            &crate::tray_menu::tests::both_enabled(),
            MenuState {
                float_bar_enabled: true,
                stay_awake_held: true,
            },
            Language::English,
        );

        let actionable = actionable_ids(&spec);
        for expected in [
            "refresh",
            "pop_out",
            "toggle_float_bar",
            "toggle_provider:codex",
            "toggle_provider:claude",
            "settings",
            "check_for_updates",
            "about",
            "quit",
        ] {
            assert!(
                actionable.iter().any(|id| id == expected),
                "{expected} should be actionable in the spec"
            );
        }
        for id in &actionable {
            assert!(
                resolve_menu_action(id).is_some(),
                "spec id {id} does not resolve to a menu action"
            );
        }

        let mut inert = Vec::new();
        for entry in flatten(&spec) {
            if let Some(id) = entry.id.as_deref()
                && (id.starts_with("status_") || id == "stay_awake_active")
            {
                assert!(resolve_menu_action(id).is_none(), "{id} must not dispatch");
                inert.push(id.to_string());
            }
        }
        assert_eq!(
            inert,
            vec!["status_claude", "status_codex", "stay_awake_active"],
            "the drift guard must see every status row"
        );
    }

    #[test]
    fn toggle_float_bar_routes_to_toggle_action() {
        let action = resolve_menu_action("toggle_float_bar").expect("float bar action");
        assert!(matches!(action, MenuAction::ToggleFloatBar));
    }

    #[test]
    fn settings_menu_routes_to_open_settings_action() {
        let action = resolve_menu_action("about").expect("about action");
        match action {
            MenuAction::OpenSettings(tab) => assert_eq!(tab, "about"),
            _ => panic!("expected OpenSettings for 'about'"),
        }

        let action = resolve_menu_action("settings").expect("settings action");
        match action {
            MenuAction::OpenSettings(tab) => assert_eq!(tab, "general"),
            _ => panic!("expected OpenSettings for 'settings'"),
        }
    }

    #[test]
    fn pop_out_menu_routes_to_open_flyout_action() {
        // "Pop Out Dashboard" opens the tray-panel flyout window, the only
        // dashboard layout.
        let action = resolve_menu_action("pop_out").expect("pop_out action");
        assert!(matches!(action, MenuAction::OpenFlyout));

        let props = SurfaceMode::TrayPanel.window_properties();
        assert!(!props.resizable && props.blur_dismiss && props.skip_taskbar);
    }

    #[test]
    fn legacy_popout_menu_ids_no_longer_route_anywhere() {
        // "show_panel" ("Show Window") and "provider:<id>" used to open the
        // retired PopOut layout on `main`.
        assert!(resolve_menu_action("show_panel").is_none());
        assert!(resolve_menu_action("provider:codex").is_none());
    }

    #[test]
    fn logical_tray_anchor_uses_click_monitor_scale() {
        let monitors = vec![
            MonitorScaleInfo {
                physical_x: 0,
                physical_y: 0,
                physical_width: 1920,
                physical_height: 1080,
                scale_factor: 1.0,
            },
            MonitorScaleInfo {
                physical_x: 1920,
                physical_y: 0,
                physical_width: 2560,
                physical_height: 1440,
                scale_factor: 2.0,
            },
        ];

        let rect = tauri::Rect {
            position: tauri::Position::Logical(tauri::LogicalPosition::new(1500.0, 500.0)),
            size: tauri::Size::Logical(tauri::LogicalSize::new(12.0, 12.0)),
        };
        let anchor = resolve_tray_anchor(
            &rect,
            tauri::PhysicalPosition::new(1510.0, 500.0),
            &monitors,
        )
        .expect("matching click monitor scale");

        assert_eq!(anchor.x, 1500);
        assert_eq!(anchor.y, 500);
        assert_eq!(anchor.width, 12);
        assert_eq!(anchor.height, 12);
    }

    #[test]
    fn logical_tray_anchor_skips_conversion_without_click_monitor() {
        let monitors = vec![MonitorScaleInfo {
            physical_x: 0,
            physical_y: 0,
            physical_width: 1920,
            physical_height: 1080,
            scale_factor: 1.0,
        }];
        let rect = tauri::Rect {
            position: tauri::Position::Logical(tauri::LogicalPosition::new(1500.0, 500.0)),
            size: tauri::Size::Logical(tauri::LogicalSize::new(12.0, 12.0)),
        };

        let anchor = resolve_tray_anchor(
            &rect,
            tauri::PhysicalPosition::new(2500.0, 500.0),
            &monitors,
        );

        assert!(anchor.is_none());
    }

    #[test]
    fn tooltip_uses_compact_status_labels() {
        let mut claude = fake_snapshot("claude", "Claude", 13.0);
        claude.primary.reset_description = Some("2h 05m".to_string());
        let mut codex = fake_snapshot("codex", "Codex", 8.0);
        codex.primary.reset_description = Some("4h 10m".to_string());

        let tooltip = build_tooltip(
            &[claude, codex],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(
            tooltip,
            "CodexBar\nClaude: 87% left • Resets in 2h 05m\nCodex: 92% left • Resets in 4h 10m"
        );
    }

    #[test]
    fn tooltip_names_used_or_remaining_percent_from_settings() {
        let mut claude = fake_snapshot("claude", "Claude", 13.0);
        claude.primary.reset_description = Some("2h 05m".to_string());
        let lang = codexbar::settings::Language::English;

        assert_eq!(
            build_tooltip(&[claude.clone()], &Settings::default(), lang, false),
            "CodexBar\nClaude: 87% left • Resets in 2h 05m"
        );
        let used = Settings {
            show_as_used: true,
            ..Settings::default()
        };
        assert_eq!(
            build_tooltip(&[claude], &used, lang, false),
            "CodexBar\nClaude: 13% used • Resets in 2h 05m"
        );
    }

    #[test]
    fn tooltip_marks_an_estimated_quota_with_a_tilde() {
        let mut estimated = fake_snapshot("opencodego", "OpenCode Go", 95.0);
        estimated.quota_estimated = true;
        let exact = fake_snapshot("opencodego", "OpenCode Go", 95.0);

        let lang = codexbar::settings::Language::English;
        assert_eq!(
            build_tooltip(&[estimated], &Settings::default(), lang, false),
            "CodexBar\nOpenCode Go: ~5% left"
        );
        assert_eq!(
            build_tooltip(&[exact], &Settings::default(), lang, false),
            "CodexBar\nOpenCode Go: 5% left"
        );
    }

    #[test]
    fn tooltip_shows_the_selected_named_extra_metric() {
        let mut settings = Settings::default();
        settings.set_provider_metric(
            codexbar::core::ProviderId::Cursor,
            codexbar::settings::MetricPreference::NamedExtra("cursor-grok-bot".to_string()),
        );
        let mut cursor = fake_snapshot("cursor", "Cursor", 30.0);
        cursor.extra_rate_windows = vec![crate::commands::NamedRateWindowSnapshot {
            id: "cursor-grok-bot".to_string(),
            title: "Grok Bot".to_string(),
            window: crate::commands::RateWindowSnapshot {
                used_percent: 42.0,
                remaining_percent: 58.0,
                ..Default::default()
            },
            fallback_lane: false,
            icon_fallback: None,
        }];

        let lang = codexbar::settings::Language::English;
        assert_eq!(
            build_tooltip(&[cursor.clone()], &settings, lang, false),
            "CodexBar\nCursor: Grok Bot 58% left"
        );
        assert_eq!(
            build_tooltip(&[cursor], &Settings::default(), lang, false),
            "CodexBar\nCursor: 70% left"
        );
    }

    #[test]
    fn tooltip_shows_tokens_per_minute_when_selected() {
        let mut settings = Settings::default();
        settings.set_provider_metric(
            codexbar::core::ProviderId::OpenAIApi,
            codexbar::settings::MetricPreference::TokensPerMinute,
        );
        let mut openai = fake_snapshot("openaiapi", "OpenAI API", 40.0);
        openai.throughput = Some(crate::commands::ProviderThroughputSnapshot {
            minutes: Vec::new(),
            frontier: Some(1_700_000_000),
            latest_tokens_per_minute: Some(1_500_000),
            peak_tokens_per_minute: 1_500_000,
            peak_at: None,
            five_minute_average_tokens_per_minute: 0.0,
            models: Vec::new(),
            window_minutes: 60,
            fetched_at: 1_700_000_060,
            project_id: None,
        });

        let tooltip = build_tooltip(
            &[openai, fake_snapshot("codex", "Codex", 8.0)],
            &settings,
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip, "CodexBar\nOpenAI API: 1.5M/min\nCodex: 92% left");
    }

    #[test]
    fn tooltip_skips_informational_codex_primary() {
        // Weekly-only Codex plan: the 5h lane is an informational placeholder,
        // so the tooltip must label the real weekly lane instead of echoing
        // "No active 5h session" back at the user.
        let mut codex = fake_snapshot_with("codex", "Codex", 0.0, Some(16.0), None, None);
        codex.primary.is_informational = true;
        codex.primary.reset_description = Some("No active 5h session".to_string());
        codex.secondary.as_mut().unwrap().reset_description = Some("3d 17h".to_string());

        let tooltip = build_tooltip(
            &[codex],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip, "CodexBar\nCodex: 84% left • Resets in 3d 17h");
    }

    fn balance_detail(id: &str, value: &str) -> crate::commands::ProviderDisplayDetailSnapshot {
        crate::commands::ProviderDisplayDetailSnapshot {
            id: id.to_string(),
            section_title: None,
            title: id.to_string(),
            value: value.to_string(),
            secondary_value: None,
            progress: None,
        }
    }

    fn informational_nous(reset_description: &str) -> crate::commands::ProviderUsageSnapshot {
        let mut nous = fake_snapshot("nous", "Nous Portal", 0.0);
        nous.primary.is_informational = true;
        nous.primary.reset_description = Some(reset_description.to_string());
        nous
    }

    #[test]
    fn tooltip_shows_nous_balance_instead_of_generic_credit_label() {
        // Top-up credits only: the headline is the generic informational label,
        // so the tooltip shows the reported balance (upstream #4319).
        let mut nous = informational_nous("Nous Portal credits");
        nous.display_details = vec![balance_detail("top-up-credits", "$6.76")];

        let tooltip = build_tooltip(
            &[nous],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip, "CodexBar\nNous Portal: $6.76");
    }

    #[test]
    fn tooltip_nous_balance_prefers_total_usable_and_keeps_zero() {
        let mut nous = informational_nous("$0.00 monthly grant");
        nous.display_details = vec![
            balance_detail("top-up-credits", "$3.00"),
            balance_detail("total-usable", "$0.00"),
        ];

        let tooltip = build_tooltip(
            &[nous],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip, "CodexBar\nNous Portal: $0.00");
    }

    #[test]
    fn tooltip_keeps_nous_monthly_percentage_over_balance() {
        let mut nous = fake_snapshot("nous", "Nous Portal", 25.0);
        nous.display_details = vec![balance_detail("total-usable", "$74.25")];

        let tooltip = build_tooltip(
            &[nous],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip, "CodexBar\nNous Portal: 75% left");
    }

    #[test]
    fn tooltip_balance_stays_with_the_declaring_provider() {
        // Another provider's balance-named row must not replace its own label,
        // and Nous without a reported balance keeps its headline text.
        let mut claude = informational_nous("Claude informational");
        claude.provider_id = "claude".to_string();
        claude.display_name = "Claude".to_string();
        claude.display_details = vec![balance_detail("top-up-credits", "$6.76")];
        let nous = informational_nous("Nous Portal credits");

        let tooltip = build_tooltip(
            &[claude, nous],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(
            tooltip,
            "CodexBar\nClaude: Claude informational\nNous Portal: Nous Portal credits"
        );
    }

    #[test]
    fn tooltip_truncates_long_provider_lines() {
        let mut claude = fake_snapshot("claude", "Claude", 13.0);
        claude.primary.reset_description =
            Some("resets in Jun 10 at 3:00PM with extra noisy suffix".to_string());

        let tooltip = build_tooltip(
            &[claude],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        let line = tooltip.lines().nth(1).expect("provider tooltip line");
        assert!(line.starts_with("Claude: 87% left • Resets in Jun 10"));
        assert!(line.ends_with("..."));
        assert!(line.chars().count() <= 53);
    }

    #[test]
    fn hidden_personal_info_removes_emails_from_tooltip_errors() {
        let mut codex = fake_snapshot("codex", "Codex", 13.0);
        codex.error = Some("proof@example.com: session expired".to_string());

        let shown = build_tooltip(
            &[codex.clone()],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );
        assert!(shown.contains("proof@example.com"), "{shown}");

        let hidden = build_tooltip(
            &[codex],
            &Settings::default(),
            codexbar::settings::Language::English,
            true,
        );
        assert!(hidden.contains("session expired"), "{hidden}");
        assert!(!hidden.contains('@'), "{hidden}");
        assert!(!hidden.contains("example.com"), "{hidden}");
    }

    #[test]
    fn japanese_tooltip_localizes_error_status() {
        let mut claude = fake_snapshot("claude", "Claude", 13.0);
        claude.error = Some("network timeout".to_string());

        let tooltip = build_tooltip(
            &[claude],
            &Settings::default(),
            codexbar::settings::Language::Japanese,
            false,
        );

        assert!(tooltip.contains("エラー"), "{tooltip}");
        assert!(!tooltip.contains(": error ("), "{tooltip}");
    }

    #[test]
    fn tray_labels_relocalize_on_language_change_without_refetch() {
        let mut claude = fake_snapshot("claude", "Claude", 13.0);
        claude.primary.resets_at =
            Some((chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339());

        let english_tooltip = build_tooltip(
            &[claude.clone()],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );
        let japanese_tooltip = build_tooltip(
            &[claude.clone()],
            &Settings::default(),
            codexbar::settings::Language::Japanese,
            false,
        );

        assert!(english_tooltip.contains("Resets in"), "{english_tooltip}");
        assert!(
            japanese_tooltip.contains("リセットまで"),
            "{japanese_tooltip}"
        );
        assert!(
            !japanese_tooltip.to_ascii_lowercase().contains("resets in"),
            "{japanese_tooltip}"
        );

        let settings = Settings::default();
        let snapshots = vec![claude];
        let rates_cache = None;
        let plan = TrayPresentationPlan::resolve(&settings, &snapshots, rates_cache);
        let english_label = plan.status_labels(codexbar::settings::Language::English)[0]
            .1
            .clone();
        let japanese_label = plan.status_labels(codexbar::settings::Language::Japanese)[0]
            .1
            .clone();
        assert!(english_label.contains("Resets in"), "{english_label}");
        assert!(japanese_label.contains("リセットまで"), "{japanese_label}");
    }

    /// The full `szTip[128]` budget, so layout tests can use realistic line lengths.
    const WIDE_BUDGET: usize = 127;

    #[test]
    fn fit_tooltip_keeps_short_tooltips_unchanged() {
        let tooltip = "CodexBar\nClaude: 87% left\nCodex: 92% left";
        assert_eq!(fit_tooltip(tooltip, TRAY_TOOLTIP_MAX_UTF16), tooltip);
    }

    #[test]
    fn fit_tooltip_stays_within_the_windows_limit_and_keeps_whole_lines() {
        let mut lines = vec!["CodexBar".to_string()];
        for index in 0..20 {
            lines.push(format!(
                "Provider {index}: 50% left • Resets in 3d 17h with a long suffix"
            ));
        }
        let tooltip = lines.join("\n");

        let fitted = fit_tooltip(&tooltip, WIDE_BUDGET);

        assert!(utf16_len(&fitted) <= WIDE_BUDGET, "{fitted}");
        let shown: Vec<&str> = fitted.split('\n').collect();
        assert_eq!(shown[0], "CodexBar", "{fitted}");
        assert!(shown.len() > 2, "{fitted}");
        let last = shown.last().expect("last line");
        assert!(last.ends_with('…'), "{fitted}");
        for line in &shown[1..shown.len() - 1] {
            assert!(lines.contains(&line.to_string()), "split line: {line}");
        }
    }

    #[test]
    fn fit_tooltip_ends_the_last_kept_line_with_an_ellipsis_when_lines_are_dropped() {
        // Title plus one line that uses almost the whole budget; the dropped
        // third line leaves the kept line with the ellipsis, which still fits.
        let long = "A".repeat(115);
        let tooltip = format!("CodexBar\n{long}\nB line that is dropped entirely here");

        let fitted = fit_tooltip(&tooltip, WIDE_BUDGET);

        assert!(utf16_len(&fitted) <= WIDE_BUDGET, "{fitted}");
        assert_eq!(fitted, format!("CodexBar\n{long}…"));
    }

    #[test]
    fn fit_tooltip_keeps_incident_lines_under_the_title_before_provider_lines() {
        let tooltip = [
            "CodexBar".to_string(),
            "OpenAI: Major outage".to_string(),
            "Claude: 87% left".to_string(),
            "A".repeat(120),
        ]
        .join("\n");

        let fitted = fit_tooltip(&tooltip, WIDE_BUDGET);

        assert_eq!(fitted, "CodexBar\nOpenAI: Major outage\nClaude: 87% left…");
        assert!(utf16_len(&fitted) <= WIDE_BUDGET);
    }

    #[test]
    fn fit_tooltip_drops_an_oversized_line_and_marks_the_title() {
        // The 200-unit emoji line cannot fit beside the title, so it is dropped.
        let tooltip = format!("CodexBar\n{}", "🙂".repeat(100));

        let fitted = fit_tooltip(&tooltip, TRAY_TOOLTIP_MAX_UTF16);

        assert_eq!(fitted, "CodexBar…");
    }

    #[test]
    fn fit_tooltip_cuts_an_oversized_first_line_without_splitting_a_surrogate_pair() {
        // Each emoji is two UTF-16 units; a char-count cut would overflow or split a pair.
        let tooltip = format!("{}\nCodexBar", "🙂".repeat(100));

        let fitted = fit_tooltip(&tooltip, WIDE_BUDGET);

        assert!(utf16_len(&fitted) <= WIDE_BUDGET, "{fitted}");
        assert_eq!(fitted.chars().filter(|ch| *ch == '🙂').count(), 63);
        assert!(fitted.ends_with('…'), "{fitted}");
        assert!(!fitted.contains('\n'), "{fitted}");
    }

    #[test]
    fn fit_tooltip_keeps_whole_provider_lines_within_the_legacy_tip_limit() {
        let tooltip =
            "CodexBar\nClaude: 58% left • Resets in 2h 59m\nCodex: 82% left • Resets in 4d 3h";

        let fitted = fit_tooltip(tooltip, TRAY_TOOLTIP_MAX_UTF16);

        assert!(utf16_len(&fitted) <= 63, "{fitted}");
        assert_eq!(fitted, "CodexBar\nClaude: 58% left • Resets in 2h 59m…");
    }

    #[test]
    fn fit_tooltip_handles_a_first_line_longer_than_the_limit() {
        let fitted = fit_tooltip(&"중".repeat(200), TRAY_TOOLTIP_MAX_UTF16);

        assert!(utf16_len(&fitted) <= TRAY_TOOLTIP_MAX_UTF16, "{fitted}");
        assert!(fitted.ends_with('…'));
        assert!(!fitted.contains('\n'));
    }

    #[test]
    fn truncate_tooltip_text_counts_utf16_units_not_chars() {
        // Four emoji are 8 UTF-16 units; a 6-unit budget keeps 3 emoji.
        let truncated = truncate_tooltip_text(&"🙂".repeat(4), 6);
        assert_eq!(truncated, "🙂🙂🙂...");
        assert_eq!(truncate_tooltip_text("short", 36), "short");
    }

    #[test]
    fn provider_error_newlines_do_not_split_the_tooltip_line() {
        let mut codex = fake_snapshot("codex", "Codex", 13.0);
        codex.error = Some("first line\nsecond line".to_string());

        let tooltip = build_tooltip(
            &[codex],
            &Settings::default(),
            codexbar::settings::Language::English,
            false,
        );

        assert_eq!(tooltip.lines().count(), 2, "{tooltip}");
    }
}
