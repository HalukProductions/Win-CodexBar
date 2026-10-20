//! App-managed HTTP usage server (upstream 0.77.0 #4553 `HTTPServerController`).
//!
//! Upstream launches its bundled CLI helper as a child process. The Windows
//! shell has no existing way to locate `codexbar-cli.exe` (dev builds name it
//! `codexbar.exe`, which is the desktop binary in release), while the
//! `codexbar` crate exposes the serve entry as a library. So the server runs
//! in-process on the app's Tokio runtime: the token stays in memory (never on
//! a command line or in an environment block), no console window can appear,
//! and the server cannot outlive the app.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use codexbar::settings::{HttpServerConfig, HttpServerValidationError};
use serde::Serialize;
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum HttpServerStatus {
    Stopped,
    Running { url: String },
    Invalid { code: HttpServerValidationError },
    StartFailed { detail: String },
    Exited { detail: String },
    LoadFailed,
    SaveFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpServerSnapshot {
    pub config: HttpServerConfig,
    pub status: HttpServerStatus,
}

struct Shared {
    status: HttpServerStatus,
    /// Bumped on every start so a late exit report from a replaced server
    /// cannot overwrite the current status.
    generation: u64,
    abort: Option<AbortHandle>,
}

struct Lifecycle {
    config: HttpServerConfig,
    task: Option<JoinHandle<()>>,
}

pub struct HttpServerController {
    path: Option<PathBuf>,
    lifecycle: tokio::sync::Mutex<Lifecycle>,
    shared: Arc<Mutex<Shared>>,
    shutting_down: AtomicBool,
}

impl HttpServerController {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            lifecycle: tokio::sync::Mutex::new(Lifecycle {
                config: HttpServerConfig::default(),
                task: None,
            }),
            shared: Arc::new(Mutex::new(Shared {
                status: HttpServerStatus::Stopped,
                generation: 0,
                abort: None,
            })),
            shutting_down: AtomicBool::new(false),
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_status(&self, status: HttpServerStatus) {
        self.shared().status = status;
    }

    pub async fn snapshot(&self) -> HttpServerSnapshot {
        let lifecycle = self.lifecycle.lock().await;
        HttpServerSnapshot {
            config: lifecycle.config.clone(),
            status: self.shared().status.clone(),
        }
    }

    /// Load the saved configuration and start the server when it is enabled.
    pub async fn start_saved(&self) -> HttpServerSnapshot {
        {
            let mut lifecycle = self.lifecycle.lock().await;
            if self.shutting_down.load(Ordering::SeqCst) {
                drop(lifecycle);
                return self.snapshot().await;
            }
            let loaded = match &self.path {
                Some(path) => HttpServerConfig::load_from(path),
                None => Ok(HttpServerConfig::default()),
            };
            match loaded {
                Ok(config) => {
                    lifecycle.config = config;
                    if lifecycle.config.enabled {
                        self.restart(&mut lifecycle).await;
                    }
                }
                Err(error) => {
                    tracing::warn!("could not read HTTP server settings: {error}");
                    self.set_status(HttpServerStatus::LoadFailed);
                }
            }
        }
        self.snapshot().await
    }

    /// Save `config` and restart the server to match it. An enabled but
    /// invalid configuration is neither saved nor started; a running server
    /// is stopped first so its port closes instead of serving stale settings.
    pub async fn apply(&self, config: HttpServerConfig) -> HttpServerSnapshot {
        {
            let mut lifecycle = self.lifecycle.lock().await;
            if self.shutting_down.load(Ordering::SeqCst) {
                drop(lifecycle);
                return self.snapshot().await;
            }
            if config.enabled
                && let Some(code) = config.validation_error()
            {
                self.stop(&mut lifecycle).await;
                self.set_status(HttpServerStatus::Invalid { code });
                drop(lifecycle);
                return self.snapshot().await;
            }
            let saved = match &self.path {
                Some(path) => config.save_to(path),
                None => Err(std::io::Error::other("no settings directory")),
            };
            if let Err(error) = saved {
                tracing::warn!("could not save HTTP server settings: {error}");
                self.set_status(HttpServerStatus::SaveFailed);
                drop(lifecycle);
                return self.snapshot().await;
            }
            lifecycle.config = config;
            self.restart(&mut lifecycle).await;
        }
        self.snapshot().await
    }

    /// Synchronous stop for app exit. Later `apply` / `start_saved` calls
    /// are ignored.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let mut shared = self.shared();
        if let Some(abort) = shared.abort.take() {
            abort.abort();
        }
        shared.status = HttpServerStatus::Stopped;
    }

    async fn stop(&self, lifecycle: &mut Lifecycle) {
        let task = lifecycle.task.take();
        {
            let mut shared = self.shared();
            shared.abort = None;
            shared.generation += 1;
            shared.status = HttpServerStatus::Stopped;
        }
        if let Some(task) = task {
            task.abort();
            // Awaiting the aborted task drops its listener, freeing the port
            // before a restart binds it again.
            let _cancelled: Result<(), _> = task.await;
        }
    }

    async fn restart(&self, lifecycle: &mut Lifecycle) {
        self.stop(lifecycle).await;
        let config = &lifecycle.config;
        if !config.enabled {
            return;
        }
        if let Some(code) = config.validation_error() {
            self.set_status(HttpServerStatus::Invalid { code });
            return;
        }
        let server = match codexbar::cli::serve::bind(config.serve_args()).await {
            Ok(server) => server,
            Err(error) => {
                tracing::warn!("HTTP server failed to start: {error}");
                self.set_status(HttpServerStatus::StartFailed {
                    detail: error.to_string(),
                });
                return;
            }
        };
        let url = config.url();
        let mut shared = self.shared();
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let generation = shared.generation;
        let reporter = Arc::clone(&self.shared);
        let task = tokio::spawn(async move {
            let detail = match server.serve().await {
                Ok(()) => String::new(),
                Err(error) => error.to_string(),
            };
            tracing::warn!("HTTP server stopped unexpectedly: {detail}");
            let mut shared = reporter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if shared.generation == generation {
                shared.abort = None;
                shared.status = HttpServerStatus::Exited { detail };
            }
        });
        shared.abort = Some(task.abort_handle());
        shared.status = HttpServerStatus::Running { url };
        drop(shared);
        lifecycle.task = Some(task);
    }
}

pub fn controller() -> Arc<HttpServerController> {
    Arc::new(HttpServerController::new(HttpServerConfig::default_path()))
}

/// Controller for proof mode: no config path, so `apply` fails with
/// `SaveFailed` before anything is written to disk or bound to a port.
pub fn proof_controller() -> Arc<HttpServerController> {
    Arc::new(HttpServerController::new(None))
}

pub fn install(controller: Arc<HttpServerController>) {
    tauri::async_runtime::spawn(async move {
        controller.start_saved().await;
    });
}

#[tauri::command]
pub async fn get_http_server_state(
    controller: tauri::State<'_, Arc<HttpServerController>>,
) -> Result<HttpServerSnapshot, String> {
    Ok(controller.snapshot().await)
}

#[tauri::command]
pub async fn apply_http_server_config(
    config: HttpServerConfig,
    controller: tauri::State<'_, Arc<HttpServerController>>,
) -> Result<HttpServerSnapshot, String> {
    Ok(controller.apply(config).await)
}

#[cfg(test)]
mod tests;
