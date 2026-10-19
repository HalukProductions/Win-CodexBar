//! A settings file that exists but cannot be loaded is never overwritten
//! (upstream 0.76.0 #4509).

use super::*;

const UNPARSEABLE: &str = r#"{ "refresh_interval_secs": 60, "enabled_providers": ["#;

/// Load, change one setting and save, as every settings command does.
fn change_and_save(path: &Path) -> anyhow::Result<()> {
    let mut settings = Settings::load_from_path(Some(path));
    settings.refresh_interval_secs = 900;
    settings.save_to_path(path)
}

#[test]
fn unparseable_file_is_not_overwritten_by_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, UNPARSEABLE).expect("write");

    let loaded = Settings::load_from_path(Some(&path));
    assert!(loaded.load_state.is_unreadable_file());
    assert_eq!(loaded.refresh_interval_secs, 300, "runs on defaults");

    let error = change_and_save(&path).expect_err("save must refuse");
    assert!(error.to_string().contains("will not overwrite"));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), UNPARSEABLE);
}

#[test]
fn unreadable_secure_file_is_not_overwritten_by_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    // Wrappers `secure_file::read_string` rejects: a payload that cannot be
    // decrypted and a format version this build does not know.
    let wrappers = [
        r#"{"format":"codexbar.secure-file","version":1,"protection":"windows-dpapi-user","payload":"not base64!"}"#,
        r#"{"format":"codexbar.secure-file","version":99,"protection":"windows-dpapi-user","payload":""}"#,
    ];
    for wrapper in wrappers {
        std::fs::write(&path, wrapper).expect("write");
        assert!(crate::secure_file::read_string(&path).is_err());

        let loaded = Settings::load_from_path(Some(&path));
        assert!(loaded.load_state.is_unreadable_file());
        assert!(change_and_save(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), wrapper);
    }
}

#[test]
fn missing_file_loads_defaults_that_save() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");

    assert!(
        !Settings::load_from_path(Some(&path))
            .load_state
            .is_unreadable_file()
    );
    assert!(
        !Settings::load_from_path(None)
            .load_state
            .is_unreadable_file()
    );
    change_and_save(&path).expect("first-run save");
    assert_eq!(
        Settings::load_from_path(Some(&path)).refresh_interval_secs,
        900
    );
}

#[test]
fn valid_file_round_trips_and_saves() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    let original = Settings {
        refresh_interval_secs: 120,
        ..Settings::default()
    };
    original.save_to_path(&path).expect("save");

    let loaded = Settings::load_from_path(Some(&path));
    assert!(!loaded.load_state.is_unreadable_file());
    assert_eq!(loaded.refresh_interval_secs, 120);
    change_and_save(&path).expect("save after a good load");
    assert_eq!(
        Settings::load_from_path(Some(&path)).refresh_interval_secs,
        900
    );
}

#[test]
fn repaired_file_saves_again_on_the_next_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, UNPARSEABLE).expect("write");
    let stale = Settings::load_from_path(Some(&path));

    std::fs::write(&path, r#"{ "refresh_interval_secs": 60 }"#).expect("repair");
    // A copy loaded before the repair still must not replace the repaired file.
    assert!(stale.save_to_path(&path).is_err());
    assert_eq!(
        Settings::load_from_path(Some(&path)).refresh_interval_secs,
        60
    );

    change_and_save(&path).expect("save after the repair");
    assert_eq!(
        Settings::load_from_path(Some(&path)).refresh_interval_secs,
        900
    );
}

#[test]
fn preferences_import_keeps_the_save_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, UNPARSEABLE).expect("write");

    let mut settings = Settings::load_from_path(Some(&path));
    let document = PreferencesDocument::from_settings(&Settings::default()).expect("export");
    document.apply_to(&mut settings).expect("apply");
    assert!(settings.load_state.is_unreadable_file());
    assert!(settings.save_to_path(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).expect("read"), UNPARSEABLE);
}

#[test]
fn display_mode_fold_waits_for_a_readable_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    let marker = dir.path().join(".display-mode-fold-v1");
    std::fs::write(&path, UNPARSEABLE).expect("write");

    let mut settings = Settings::load_from_path(Some(&path));
    settings.menu_bar_display_mode = "minimal".to_string();
    settings.apply_display_mode_fold_with_marker(&marker, |_| {
        panic!("must not persist stand-in defaults")
    });
    assert!(!marker.exists(), "the fold retries once the file loads");
    assert!(!settings.menu_bar_shows_highest_usage);
}
