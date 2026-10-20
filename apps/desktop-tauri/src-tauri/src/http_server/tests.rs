use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

const TOKEN: &str = "synthetic-token";

fn temp_config_path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("codexbar-http-server-{}", uuid::Uuid::new_v4()))
        .join("http_server.json")
}

fn free_port() -> u32 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    u32::from(listener.local_addr().expect("addr").port())
}

fn enabled(port: u32) -> HttpServerConfig {
    HttpServerConfig {
        enabled: true,
        port,
        token: TOKEN.to_string(),
        ..HttpServerConfig::default()
    }
}

/// Status line of one request, or `None` when nothing accepts on the port.
async fn status_line(port: u32, path: &str, bearer: Option<&str>) -> Option<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", u16::try_from(port).ok()?))
        .await
        .ok()?;
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut buf = vec![0_u8; 256];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&buf[..read]).to_string();
    text.lines().next().map(str::to_string)
}

async fn wait_closed(port: u32) -> bool {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", u16::try_from(port).unwrap()))
            .await
            .is_err()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

fn run<F: std::future::Future<Output = ()>>(future: F) {
    tauri::async_runtime::block_on(future);
}

#[test]
fn disabled_startup_binds_nothing_and_writes_no_file() {
    run(async {
        let path = temp_config_path();
        let controller = HttpServerController::new(Some(path.clone()));
        let snapshot = controller.start_saved().await;
        assert_eq!(snapshot.status, HttpServerStatus::Stopped);
        assert_eq!(snapshot.config, HttpServerConfig::default());
        assert!(!path.exists());
    });
}

#[test]
fn invalid_enabled_settings_are_neither_saved_nor_started() {
    run(async {
        let path = temp_config_path();
        let controller = HttpServerController::new(Some(path.clone()));
        let mut config = enabled(free_port());
        config.host = "0.0.0.0".to_string();
        let snapshot = controller.apply(config).await;
        assert_eq!(
            snapshot.status,
            HttpServerStatus::Invalid {
                code: HttpServerValidationError::PlainHttpNotAccepted
            }
        );
        assert_eq!(snapshot.config, HttpServerConfig::default());
        assert!(!path.exists());
    });
}

#[test]
fn invalid_apply_stops_the_running_server_and_keeps_the_saved_config() {
    run(async {
        let path = temp_config_path();
        let controller = HttpServerController::new(Some(path.clone()));
        let port = free_port();
        let mut lan = enabled(port);
        lan.host = "0.0.0.0".to_string();
        lan.allow_plain_http = true;
        let snapshot = controller.apply(lan.clone()).await;
        assert!(
            matches!(snapshot.status, HttpServerStatus::Running { .. }),
            "{:?}",
            snapshot.status
        );
        assert_eq!(
            status_line(port, "/", None).await.as_deref(),
            Some("HTTP/1.1 200 OK")
        );

        // The same config without plain-HTTP consent is invalid.
        let mut invalid = lan.clone();
        invalid.allow_plain_http = false;
        let snapshot = controller.apply(invalid).await;
        assert_eq!(
            snapshot.status,
            HttpServerStatus::Invalid {
                code: HttpServerValidationError::PlainHttpNotAccepted
            }
        );
        assert!(
            wait_closed(port).await,
            "an invalid apply closes the running server's port"
        );
        assert_eq!(
            snapshot.config, lan,
            "the rejected config is not adopted; the snapshot keeps the saved one"
        );
        assert_eq!(
            HttpServerConfig::load_from(&path).expect("saved settings"),
            lan,
            "the rejected config is not saved over the working one"
        );

        // A valid apply recovers from the invalid state.
        let snapshot = controller.apply(lan.clone()).await;
        assert!(matches!(snapshot.status, HttpServerStatus::Running { .. }));
        assert_eq!(
            status_line(port, "/", None).await.as_deref(),
            Some("HTTP/1.1 200 OK")
        );
        controller.shutdown();
        assert!(wait_closed(port).await);
    });
}

#[test]
fn pathless_controller_never_saves_nor_binds() {
    run(async {
        // Proof mode builds the controller without a config path.
        let controller = HttpServerController::new(None);
        let snapshot = controller.start_saved().await;
        assert_eq!(snapshot.status, HttpServerStatus::Stopped);
        assert_eq!(snapshot.config, HttpServerConfig::default());

        let port = free_port();
        let snapshot = controller.apply(enabled(port)).await;
        assert_eq!(snapshot.status, HttpServerStatus::SaveFailed);
        assert_eq!(
            snapshot.config,
            HttpServerConfig::default(),
            "a failed save does not adopt the config"
        );
        assert!(
            wait_closed(port).await,
            "nothing binds without a config path"
        );
    });
}

#[test]
fn server_starts_restarts_on_change_stops_on_disable_and_on_shutdown() {
    run(async {
        let path = temp_config_path();
        let controller = HttpServerController::new(Some(path.clone()));
        let first_port = free_port();
        let snapshot = controller.apply(enabled(first_port)).await;
        assert_eq!(
            snapshot.status,
            HttpServerStatus::Running {
                url: format!("http://127.0.0.1:{first_port}/")
            }
        );
        assert!(path.exists(), "applied settings are saved");
        assert_eq!(
            status_line(first_port, "/", None).await.as_deref(),
            Some("HTTP/1.1 200 OK")
        );
        // The in-memory token gates the snapshot route; the request is
        // rejected before any provider data is gathered.
        assert_eq!(
            status_line(first_port, "/dashboard/v1/snapshot", None)
                .await
                .as_deref(),
            Some("HTTP/1.1 401 Unauthorized")
        );
        assert_eq!(
            status_line(first_port, "/dashboard/v1/snapshot", Some("wrong-token"))
                .await
                .as_deref(),
            Some("HTTP/1.1 401 Unauthorized")
        );

        let mut new_token = enabled(first_port);
        new_token.token = "rotated-token".to_string();
        let snapshot = controller.apply(new_token).await;
        assert_eq!(
            snapshot.status,
            HttpServerStatus::Running {
                url: format!("http://127.0.0.1:{first_port}/")
            },
            "a same-port restart rebinds after the old listener is gone"
        );

        let second_port = free_port();
        let snapshot = controller.apply(enabled(second_port)).await;
        assert!(matches!(snapshot.status, HttpServerStatus::Running { .. }));
        assert!(wait_closed(first_port).await, "old listener is released");
        assert_eq!(
            status_line(second_port, "/", None).await.as_deref(),
            Some("HTTP/1.1 200 OK")
        );

        let mut disabled = enabled(second_port);
        disabled.enabled = false;
        let snapshot = controller.apply(disabled).await;
        assert_eq!(snapshot.status, HttpServerStatus::Stopped);
        assert!(wait_closed(second_port).await);

        // A fresh controller (next app launch) starts from the saved file.
        controller.apply(enabled(second_port)).await;
        controller.shutdown();
        assert!(wait_closed(second_port).await, "shutdown releases the port");
        let relaunched = HttpServerController::new(Some(path.clone()));
        let snapshot = relaunched.start_saved().await;
        assert_eq!(snapshot.config, enabled(second_port));
        assert!(matches!(snapshot.status, HttpServerStatus::Running { .. }));
        assert_eq!(
            status_line(second_port, "/", None).await.as_deref(),
            Some("HTTP/1.1 200 OK")
        );

        relaunched.shutdown();
        assert!(wait_closed(second_port).await);
        let late_port = free_port();
        let snapshot = relaunched.apply(enabled(late_port)).await;
        assert_eq!(snapshot.status, HttpServerStatus::Stopped);
        assert!(
            wait_closed(late_port).await,
            "apply after shutdown never restarts"
        );
        assert_eq!(
            HttpServerConfig::load_from(&path).expect("saved settings"),
            enabled(second_port),
            "apply after shutdown never saves"
        );
    });
}

#[test]
fn port_conflicts_and_corrupt_settings_surface_without_touching_the_file() {
    run(async {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").expect("busy port");
        let busy_port = u32::from(busy.local_addr().expect("addr").port());
        let controller = HttpServerController::new(Some(temp_config_path()));
        let snapshot = controller.apply(enabled(busy_port)).await;
        assert!(
            matches!(snapshot.status, HttpServerStatus::StartFailed { .. }),
            "{:?}",
            snapshot.status
        );
        drop(busy);

        let path = temp_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "invalid").unwrap();
        let controller = HttpServerController::new(Some(path.clone()));
        let snapshot = controller.start_saved().await;
        assert_eq!(snapshot.status, HttpServerStatus::LoadFailed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid");
    });
}

#[test]
fn status_serializes_to_the_bridge_shape() {
    let running = serde_json::to_value(HttpServerStatus::Running {
        url: "http://127.0.0.1:8080/".to_string(),
    })
    .unwrap();
    assert_eq!(
        running,
        serde_json::json!({ "state": "running", "url": "http://127.0.0.1:8080/" })
    );
    let invalid = serde_json::to_value(HttpServerStatus::Invalid {
        code: HttpServerValidationError::PlainHttpNotAccepted,
    })
    .unwrap();
    assert_eq!(
        invalid,
        serde_json::json!({ "state": "invalid", "code": "plainHttpNotAccepted" })
    );
    assert_eq!(
        serde_json::to_value(HttpServerStatus::StartFailed {
            detail: "x".to_string()
        })
        .unwrap(),
        serde_json::json!({ "state": "startFailed", "detail": "x" })
    );
    let config = serde_json::to_value(HttpServerConfig::default()).unwrap();
    assert_eq!(
        config,
        serde_json::json!({
            "enabled": false,
            "host": "127.0.0.1",
            "port": 8080,
            "token": "",
            "refreshIntervalSecs": 60.0,
            "requestTimeoutSecs": 30.0,
            "identity": "redacted",
            "allowPlainHttp": false,
        })
    );
}
