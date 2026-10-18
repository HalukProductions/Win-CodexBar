//! Loading, migrating and saving `settings.json`.

use super::*;

impl Settings {
    /// Get the settings file path (`CODEXBAR_CONFIG` when the CLI applied it).
    pub fn settings_path() -> Option<PathBuf> {
        config_path::settings_file_for(settings_file_override(), crate::logging::config_root())
    }

    /// Load settings from disk
    pub fn load() -> Self {
        let path = Self::settings_path();
        #[allow(
            unused_mut,
            reason = "mutability is needed for conditional initialization paths that the compiler cannot prove"
        )]
        let mut settings = Self::load_from_path(path.as_deref());

        // Sync autostart toggle with actual registry state and repair stale commands from older builds.
        // A CLI run on a `CODEXBAR_CONFIG` file skips these desktop integrations, so it touches
        // neither the Run key nor the default config root.
        #[cfg(target_os = "windows")]
        if settings_file_override().is_none() {
            settings.start_at_login = Self::sync_start_at_login_registry();
            // The one-shot migration writes its marker even when the save fails,
            // so running it on stand-in defaults would skip the real file later.
            if !settings.load_state.is_unreadable_file() {
                settings.apply_promote_tray_default_migration();
            }
        }

        // One-shot migration: materialize the persisted hidden usage-item list
        // from the pre-0.62 per-provider visibility flags. After this the list
        // is the sole source of truth and the flags stay untouched.
        settings.migrate_legacy_usage_item_flags();

        settings.resolve_system_language();

        settings
    }

    /// A missing file is a first run and loads saveable defaults. A file that
    /// exists but cannot be read or parsed loads defaults that refuse to save,
    /// so a later settings change cannot replace it (upstream 0.76.0 #4509).
    pub(super) fn load_from_path(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let content = match crate::secure_file::read_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                tracing::warn!(
                    "settings.json could not be read ({error}); using defaults and leaving the file untouched"
                );
                return Self::unreadable_file_defaults();
            }
        };
        match serde_json::from_str(content.trim_start_matches('\u{feff}')) {
            Ok(settings) => settings,
            Err(error) => {
                // Only the position is logged: serde messages can quote file values.
                tracing::warn!(
                    "settings.json could not be parsed ({:?} error at line {}, column {}); using defaults and leaving the file untouched",
                    error.classify(),
                    error.line(),
                    error.column()
                );
                Self::unreadable_file_defaults()
            }
        }
    }

    fn unreadable_file_defaults() -> Self {
        Self {
            load_state: SettingsLoadState::UNREADABLE_FILE,
            ..Self::default()
        }
    }

    /// Materialize `hidden_usage_item_ids` from the pre-0.62 per-provider
    /// visibility flags where the list was never persisted. Idempotent: a
    /// provider with an explicit list is left alone.
    pub(super) fn migrate_legacy_usage_item_flags(&mut self) {
        if self
            .provider_config(ProviderId::Codex)
            .and_then(|config| config.hidden_usage_item_ids.as_ref())
            .is_none()
            && !self.spark_usage_visible(ProviderId::Codex)
        {
            self.toggle_hidden_items(ProviderId::Codex, &CODEX_SPARK_USAGE_ITEM_IDS, false);
        }
        if self
            .provider_config(ProviderId::Claude)
            .and_then(|config| config.hidden_usage_item_ids.as_ref())
            .is_none()
            && !self.claude_daily_routines_usage_visible
        {
            self.toggle_hidden_items(
                ProviderId::Claude,
                &[CLAUDE_DAILY_ROUTINES_USAGE_ITEM_ID],
                false,
            );
        }
    }

    /// Marker written after the one-shot "pin tray by default" migration (issue #237).
    pub(super) fn promote_tray_default_marker_path() -> Option<PathBuf> {
        crate::logging::config_root().map(|p| p.join(".tray-pin-default-v1"))
    }

    /// Old builds defaulted `promote_tray_icon` to false and persisted that on any
    /// settings save. Flip those installs to the new default once; later opt-outs
    /// are preserved because the marker file remains.
    pub(super) fn should_migrate_promote_tray_default(
        promote_tray_icon: bool,
        already_migrated: bool,
    ) -> bool {
        !already_migrated && !promote_tray_icon
    }

    pub(super) fn apply_promote_tray_default_migration(&mut self) {
        let Some(marker) = Self::promote_tray_default_marker_path() else {
            return;
        };
        let already_migrated = marker.exists();
        if Self::should_migrate_promote_tray_default(self.promote_tray_icon, already_migrated) {
            self.promote_tray_icon = true;
            if let Err(error) = self.save() {
                tracing::warn!("Failed to persist promote_tray_icon default migration: {error}");
            }
        }
        if !already_migrated && let Some(parent) = marker.parent() {
            // Best-effort marker dir creation; the write below reports failure.
            let _created_dir = std::fs::create_dir_all(parent);
            if let Err(error) = std::fs::write(&marker, b"1") {
                tracing::warn!("Failed to write promote_tray_icon migration marker: {error}");
            }
        }
    }

    /// Marker written after the one-shot Display mode "minimal" fold (Mac parity PR 3).
    fn display_mode_fold_marker_path() -> Option<PathBuf> {
        crate::logging::config_root().map(|p| p.join(".display-mode-fold-v1"))
    }

    /// The Display mode row is gone. "Minimal" used to force the most-used
    /// provider into the tray, so carry that into `menu_bar_shows_highest_usage`
    /// once. Returns true when the settings changed. Later opt-outs stick
    /// because the marker remains.
    fn fold_minimal_display_mode(&mut self, already_migrated: bool) -> bool {
        if already_migrated
            || self.menu_bar_shows_highest_usage
            || !self
                .menu_bar_display_mode
                .trim()
                .eq_ignore_ascii_case("minimal")
        {
            return false;
        }
        self.menu_bar_shows_highest_usage = true;
        true
    }

    /// Run the one-shot Display mode fold against the default config root.
    /// The desktop shell calls this once at startup rather than from
    /// [`Settings::load`], so unit tests and helpers that load settings never
    /// write the marker into the real config dir. A `CODEXBAR_CONFIG` run skips it.
    pub fn apply_display_mode_fold_migration(&mut self) {
        if settings_file_override().is_some() {
            return;
        }
        let Some(marker) = Self::display_mode_fold_marker_path() else {
            return;
        };
        self.apply_display_mode_fold_with_marker(&marker, Self::save);
    }

    pub(super) fn apply_display_mode_fold_with_marker(
        &mut self,
        marker: &Path,
        persist: impl FnOnce(&Self) -> anyhow::Result<()>,
    ) {
        // Retry on a launch that reads the real file instead of marking it done.
        if self.load_state.is_unreadable_file() {
            return;
        }
        let already_migrated = marker.exists();
        if self.fold_minimal_display_mode(already_migrated)
            && let Err(error) = persist(self)
        {
            // No marker: the fold retries on the next launch instead of being lost.
            tracing::warn!("Failed to persist display mode fold migration: {error}");
            return;
        }
        if !already_migrated && let Some(parent) = marker.parent() {
            // Best-effort marker dir creation; the write below reports failure.
            let _created_dir = std::fs::create_dir_all(parent);
            if let Err(error) = std::fs::write(marker, b"1") {
                tracing::warn!("Failed to write display mode fold marker: {error}");
            }
        }
    }

    /// Migration #5 write-back: when the loaded file still had legacy
    /// `provider_usage_thresholds` (already folded into `quota_warnings` on
    /// read), save once so the file holds only the new key. The desktop shell
    /// calls this at startup; tests and helpers that only load never write.
    pub fn apply_quota_warning_migration(&mut self) {
        self.apply_quota_warning_migration_with(Self::save);
    }

    pub(super) fn apply_quota_warning_migration_with(
        &mut self,
        persist: impl FnOnce(&Self) -> anyhow::Result<()>,
    ) {
        if !self.legacy_usage_thresholds_pending {
            return;
        }
        match persist(self) {
            Ok(()) => self.legacy_usage_thresholds_pending = false,
            // The legacy key stays readable, so the write retries next launch.
            Err(error) => tracing::warn!("Failed to persist quota warning migration: {error}"),
        }
    }

    /// Store the merged Overview selection: canonical ids, deduplicated,
    /// at most [`MERGED_OVERVIEW_PROVIDER_LIMIT`].
    pub fn set_merged_overview_selected_providers(&mut self, ids: Vec<String>) {
        self.merged_overview_selected_providers = raw::normalize_merged_overview_selection(ids);
    }

    /// Save settings to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::settings_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine settings path"))?;

        self.save_to_path(&path)
    }

    pub(super) fn save_to_path(&self, path: &Path) -> anyhow::Result<()> {
        if self.load_state.is_unreadable_file() {
            tracing::warn!("Not saving settings: settings.json exists but could not be loaded");
            anyhow::bail!(
                "settings.json could not be read, so CodexBar will not overwrite it. \
                 Repair or move the file, then change the setting again."
            );
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        crate::secure_file::write_string(path, &json)?;

        Ok(())
    }
}
