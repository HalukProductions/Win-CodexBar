//! Tauri event emit helpers for the desktop shell.
//!
//! Every `emit_*` function here is fire-and-forget: emissions are non-fatal
//! and each call site discards the emit result. A failed emit leaves the UI
//! stale, but the next refresh corrects it.
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::ProviderUsageSnapshot;
use crate::state::UpdateStatePayload;
use crate::surface::SurfaceMode;
use crate::surface_target::SurfaceTarget;

// ── Event name constants ─────────────────────────────────────────────

pub const SURFACE_MODE_CHANGED: &str = "surface-mode-changed";
pub const PROVIDER_UPDATED: &str = "provider-updated";
pub const REFRESH_STARTED: &str = "refresh-started";
pub const REFRESH_COMPLETE: &str = "refresh-complete";
pub const UPDATE_STATE_CHANGED: &str = "update-state-changed";
pub const LOCALE_CHANGED: &str = "locale-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const STAY_AWAKE_CHANGED: &str = "stay-awake-changed";
pub const CODEX_ACCOUNTS_UPDATED: &str = "codex-accounts-updated";
pub const API_KEYS_UPDATED: &str = "api-keys-updated";
pub const LOGIN_PHASE: &str = "login-phase";
pub const PROVIDER_STATUSES_UPDATED: &str = "provider-statuses-updated";

// ── Payloads ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceModePayload {
    pub mode: &'static str,
    pub previous: &'static str,
    pub target: SurfaceTarget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshCompletePayload {
    pub provider_count: usize,
    pub error_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshStartedPayload {
    pub provider_ids: Vec<String>,
}

/// Names only the provider whose stored API key changed. Never carries the key.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeysUpdatedPayload {
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPhasePayload {
    pub provider_id: String,
    pub phase: String,
    pub auth_link: Option<String>,
}

// ── Emit helpers ─────────────────────────────────────────────────────

pub fn emit_surface_mode_changed(
    app: &AppHandle,
    from: SurfaceMode,
    to: SurfaceMode,
    target: SurfaceTarget,
) {
    let _emit_surface_mode = app.emit(
        SURFACE_MODE_CHANGED,
        SurfaceModePayload {
            mode: to.as_str(),
            previous: from.as_str(),
            target,
        },
    );
}

pub fn emit_provider_updated(app: &AppHandle, snapshot: &ProviderUsageSnapshot) {
    let settings = codexbar::settings::Settings::load();
    let presentation =
        crate::commands::ProviderUsagePresentationSnapshot::new(snapshot.clone(), &settings);
    let _emit_provider = app.emit(PROVIDER_UPDATED, presentation);
}

pub fn emit_refresh_started(app: &AppHandle, provider_ids: Vec<String>) {
    let _emit_refresh_started = app.emit(REFRESH_STARTED, RefreshStartedPayload { provider_ids });
}

pub fn emit_refresh_complete(app: &AppHandle, provider_count: usize, error_count: usize) {
    let _emit_refresh_complete = app.emit(
        REFRESH_COMPLETE,
        RefreshCompletePayload {
            provider_count,
            error_count,
        },
    );
}

/// Broadcast that the Codex account snapshot store was refreshed (ADR 0003
/// multi-account lanes). Payload-less; listeners re-fetch via
/// `get_codex_accounts_state`.
pub fn emit_codex_accounts_updated(app: &AppHandle) {
    let _emit_codex_accounts = app.emit(CODEX_ACCOUNTS_UPDATED, ());
}

/// Broadcast that a provider's stored API key was written or removed, so the
/// Settings API key section re-reads the key list in every window.
pub fn emit_api_keys_updated(app: &AppHandle, provider_id: &str) {
    let _emit_api_keys = app.emit(
        API_KEYS_UPDATED,
        ApiKeysUpdatedPayload {
            provider_id: provider_id.to_string(),
        },
    );
}

pub fn emit_update_state_changed(app: &AppHandle, payload: &UpdateStatePayload) {
    let _emit_update_state = app.emit(UPDATE_STATE_CHANGED, payload);
}

/// Broadcast to every window that persisted settings changed, so surfaces in
/// other windows (e.g. the PopOut dashboard) re-read settings and re-render —
/// the detached Settings window and the main window are separate webviews and
/// do not share React state. Payload-less; listeners re-fetch the snapshot.
pub fn emit_settings_changed(app: &AppHandle) {
    let _emit_settings = app.emit(SETTINGS_CHANGED, ());
}

pub fn emit_stay_awake_changed(app: &AppHandle, held: bool) {
    let _emit_stay_awake = app.emit(STAY_AWAKE_CHANGED, held);
}

pub fn emit_login_phase(app: &AppHandle, provider_id: &str, phase: &str, auth_link: Option<&str>) {
    let _emit_login_phase = app.emit(
        LOGIN_PHASE,
        LoginPhasePayload {
            provider_id: provider_id.to_string(),
            phase: phase.to_string(),
            auth_link: auth_link.map(|s| s.to_string()),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_keys_updated_payload_names_the_provider_and_never_a_key() {
        let json = serde_json::to_value(ApiKeysUpdatedPayload {
            provider_id: "openrouter".into(),
        })
        .expect("serialize");
        assert_eq!(json, serde_json::json!({ "providerId": "openrouter" }));
        assert_eq!(API_KEYS_UPDATED, "api-keys-updated");
    }
}
