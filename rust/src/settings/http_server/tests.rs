use super::*;

const TOKEN: &str = "synthetic-token";

fn with_token() -> HttpServerConfig {
    HttpServerConfig {
        token: TOKEN.to_string(),
        ..HttpServerConfig::default()
    }
}

#[test]
fn defaults_are_disabled_loopback_and_redacted() {
    let config = HttpServerConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 8080);
    assert_eq!(config.refresh_interval_secs, 60.0);
    assert_eq!(config.request_timeout_secs, 30.0);
    assert_eq!(config.identity, "redacted");
    assert!(!config.allow_plain_http);
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidToken),
        "an empty token never starts"
    );
    assert_eq!(with_token().validation_error(), None);
    assert_eq!(with_token().url(), "http://127.0.0.1:8080/");
}

#[test]
fn validation_mirrors_cli_host_port_timing_and_remote_consent_rules() {
    let mut config = with_token();
    for host in ["localhost", " LOCALHOST ", "127.0.0.1", "127.10.1.2"] {
        config.host = host.to_string();
        assert_eq!(config.validation_error(), None, "{host}");
        assert!(config.is_loopback(), "{host}");
    }

    config.host = "100.64.0.1".to_string();
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::PlainHttpNotAccepted)
    );
    config.allow_plain_http = true;
    assert_eq!(config.validation_error(), None);

    for host in [
        "",
        "127.0.0.1.evil",
        "127.01.0.1",
        "::1",
        "999.1.1.1",
        "+1.2.3.4",
    ] {
        config.host = host.to_string();
        assert_eq!(
            config.validation_error(),
            Some(HttpServerValidationError::InvalidHost),
            "{host:?}"
        );
    }
    config.host = "0.0.0.0".to_string();
    assert_eq!(config.validation_error(), None);

    config.port = 0;
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidPort)
    );
    config.port = 65_536;
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidPort)
    );
    config.port = 8080;

    config.request_timeout_secs = f64::INFINITY;
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidTiming)
    );
    config.request_timeout_secs = 0.0;
    config.refresh_interval_secs = -1.0;
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidTiming)
    );
    config.refresh_interval_secs = 0.0;
    assert_eq!(config.validation_error(), None);

    config.identity = "partial".to_string();
    assert_eq!(
        config.validation_error(),
        Some(HttpServerValidationError::InvalidIdentity)
    );
    config.identity = "full".to_string();

    for token in [" ", "two words", "tab\there"] {
        config.token = token.to_string();
        assert_eq!(
            config.validation_error(),
            Some(HttpServerValidationError::InvalidToken),
            "{token:?}"
        );
    }
}

#[test]
fn serve_args_carry_the_token_in_memory_and_bind_the_normalized_host() {
    let mut config = with_token();
    config.host = "localhost".to_string();
    config.refresh_interval_secs = 90.4;
    config.request_timeout_secs = 12.5;
    config.identity = "full".to_string();
    let args = config.serve_args();
    assert_eq!(args.host, "127.0.0.1");
    assert_eq!(args.port, 8080);
    assert_eq!(args.refresh_interval, 90);
    assert_eq!(args.request_timeout, 12.5);
    assert_eq!(args.dashboard_token.as_deref(), Some(TOKEN));
    assert_eq!(args.identity.as_deref(), Some("full"));
    assert!(!args.allow_plain_http);
    assert!(!args.metrics);
}

#[test]
fn round_trips_privately_and_missing_file_means_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join(HTTP_SERVER_CONFIG_FILE);
    assert_eq!(
        HttpServerConfig::load_from(&path).expect("missing file"),
        HttpServerConfig::default()
    );
    assert!(!path.exists(), "loading defaults never writes a file");

    let mut config = with_token();
    config.enabled = true;
    config.port = 9123;
    config.save_to(&path).expect("save");
    assert_eq!(HttpServerConfig::load_from(&path).expect("load"), config);

    #[cfg(windows)]
    {
        let raw = std::fs::read_to_string(&path).expect("raw");
        assert!(!raw.contains(TOKEN), "the token is DPAPI-protected at rest");
        assert!(matches!(
            crate::secure_file::status(&path),
            crate::secure_file::SecureFileStatus::Protected(_)
        ));
    }
}

#[test]
fn corrupt_file_is_an_error_and_is_left_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(HTTP_SERVER_CONFIG_FILE);
    std::fs::write(&path, "invalid").expect("write");
    assert!(HttpServerConfig::load_from(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "invalid");
}

#[test]
fn settings_documents_never_carry_server_fields() {
    let json = serde_json::to_string(&crate::settings::Settings::default()).expect("serialize");
    assert!(!json.contains("httpServer"));
    assert!(!json.contains("http_server"));
    assert!(!json.contains("dashboardToken"));
}
