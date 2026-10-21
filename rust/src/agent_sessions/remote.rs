//! Remote (SSH / Tailscale) agent-session retrieval and wire decoding.

use super::*;
use crate::settings::Settings;
use futures::future::join_all;
use std::ffi::OsString;

/// Decides whether remote (SSH / Tailscale) discovery may run.
///
/// Checked immediately before every remote spawn (see [`spawn_remote`]) and
/// again before remote results are merged, so a toggle made while a discovery
/// is in flight stops the next spawn and drops results already gathered.
#[derive(Clone)]
pub(crate) struct RemoteDiscoveryGate {
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl RemoteDiscoveryGate {
    /// Re-reads `settings.json` on every check. See `Settings::agent_sessions_remote_allowed_now`.
    pub(crate) fn from_settings() -> Self {
        Self::new(Settings::agent_sessions_remote_allowed_now)
    }

    pub(crate) fn new(allowed: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            allowed: Arc::new(allowed),
        }
    }

    pub(crate) fn allows(&self) -> bool {
        (self.allowed)()
    }
}

/// Runs one remote discovery process. Production uses the hidden [`CommandRunner`];
/// tests inject a recorder so they can assert which processes would have started.
pub(crate) trait RemoteProcessSpawner: Send + Sync {
    fn spawn(&self, binary: &str, options: &CommandOptions) -> Result<CommandResult, CommandError>;
}

/// Spawns through [`CommandRunner::run`], whose `spawn_child` sets `CREATE_NO_WINDOW` on Windows.
pub(crate) struct HiddenProcessSpawner;

impl RemoteProcessSpawner for HiddenProcessSpawner {
    fn spawn(&self, binary: &str, options: &CommandOptions) -> Result<CommandResult, CommandError> {
        CommandRunner::new().run(binary, None, options)
    }
}

/// Everything one discovery pass needs to reach the network: the gate, the spawner,
/// and the `PATH` used to find the standalone Tailscale CLI.
#[derive(Clone)]
pub(crate) struct RemoteContext {
    pub(crate) gate: RemoteDiscoveryGate,
    pub(crate) spawner: Arc<dyn RemoteProcessSpawner>,
    pub(crate) path_var: Option<OsString>,
}

/// Spawns one remote discovery process only while the gate allows it.
///
/// The gate is read on the blocking thread right before the spawner runs, so a
/// closed gate never starts a process. Returns `None` when nothing was spawned.
pub(crate) async fn spawn_remote(
    ctx: &RemoteContext,
    binary: String,
    options: CommandOptions,
) -> Option<Result<CommandResult, CommandError>> {
    let gate = ctx.gate.clone();
    let spawner = Arc::clone(&ctx.spawner);
    match tokio::task::spawn_blocking(move || {
        gate.allows().then(|| spawner.spawn(&binary, &options))
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => Some(Err(CommandError::LaunchFailed(error.to_string()))),
    }
}

impl RemoteSessionFetcher {
    const BUNDLED_CLI_FALLBACK: &'static str =
        "/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI";

    pub fn new(per_host_timeout: Duration) -> Self {
        Self { per_host_timeout }
    }

    /// Fetch every host over SSH. Returns nothing once the gate is closed, and drops
    /// results gathered while it was closing.
    pub(crate) async fn fetch(
        &self,
        hosts: &[String],
        ctx: &RemoteContext,
    ) -> Vec<AgentSessionHostResult> {
        if !ctx.gate.allows() {
            return Vec::new();
        }
        let valid = Self::sanitized_hosts(hosts);
        let valid_keys = valid
            .iter()
            .map(|host| Self::host_dedup_key(host))
            .collect::<HashSet<_>>();
        let mut invalid = hosts
            .iter()
            .filter(|host| {
                Self::validate_host(host).is_err()
                    && !valid_keys.contains(&Self::host_dedup_key(host.trim()))
            })
            .map(|_| {
                AgentSessionHostResult::failed(
                    "<invalid SSH host>",
                    "Invalid SSH host entry; use a host name or user@host without spaces or options.",
                )
            })
            .collect::<Vec<_>>();
        let timeout = self.per_host_timeout;
        let mut results = Self::fetch_hosts_with(&valid, |host| {
            let ctx = ctx.clone();
            async move { Self::fetch_host(host, timeout, &ctx).await }
        })
        .await;
        if !ctx.gate.allows() {
            return Vec::new();
        }
        results.append(&mut invalid);
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    /// Tailscale peers from the standalone CLI. `Ok(empty)` when the gate is closed
    /// (nothing spawned) or no standalone CLI exists.
    pub(crate) async fn tailscale_hosts(ctx: &RemoteContext) -> Result<Vec<String>, String> {
        let Some(cli) = tailscale_cli_path(ctx.path_var.as_deref()) else {
            tracing::debug!(
                "No standalone Tailscale CLI found on PATH; skipping Tailscale discovery."
            );
            return Ok(Vec::new());
        };
        let options = CommandOptions {
            timeout: Duration::from_secs(5),
            initial_delay: Duration::ZERO,
            extra_args: vec!["status".to_string(), "--json".to_string()],
            ..CommandOptions::default()
        };
        let Some(outcome) = spawn_remote(ctx, cli, options).await else {
            return Ok(Vec::new());
        };
        match outcome {
            Err(CommandError::BinaryNotFound(_)) => Ok(Vec::new()),
            Err(_) => Err(
                "Unable to query Tailscale peers; manual SSH hosts are still available.".to_string(),
            ),
            Ok(result) if result.exit_code == Some(0) && !result.timed_out => {
                TailscaleStatusParser::hosts(&result.text).map_err(|_| {
                    "Tailscale returned an invalid status response; manual SSH hosts are still available."
                        .to_string()
                })
            }
            Ok(_) => Err(
                "Tailscale status failed; manual SSH hosts are still available.".to_string(),
            ),
        }
    }

    /// One host's SSH result. `None` when the gate closed before the spawn.
    async fn fetch_host(
        host: String,
        timeout: Duration,
        ctx: &RemoteContext,
    ) -> Option<AgentSessionHostResult> {
        let options = match Self::ssh_options(&host, timeout) {
            Ok(options) => options,
            Err(error) => return Some(AgentSessionHostResult::failed("<invalid SSH host>", error)),
        };
        let result = spawn_remote(ctx, "ssh".to_string(), options).await?;
        Some(match result {
            Ok(result) if result.timed_out => AgentSessionHostResult::failed(
                host,
                "SSH session discovery timed out; verify the host is reachable and key authentication is configured.",
            ),
            Ok(result) if result.exit_code == Some(0) => {
                Self::decode_remote_sessions(&host, &result.text).unwrap_or_else(|error| {
                    AgentSessionHostResult::failed(
                        host,
                        actionable_message(
                            "Remote session response was not valid JSON; update CodexBar on the remote host",
                            error,
                        ),
                    )
                })
            }
            Ok(result) => AgentSessionHostResult::failed(
                host,
                format!(
                    "SSH session discovery failed{}; verify BatchMode key access and the remote codexbar installation.",
                    result
                        .exit_code
                        .map(|code| format!(" with exit code {code}"))
                        .unwrap_or_default()
                ),
            ),
            Err(error) => AgentSessionHostResult::failed(
                host,
                actionable_message(
                    "Unable to start SSH; install the Windows OpenSSH client and verify PATH",
                    error,
                ),
            ),
        })
    }

    pub(crate) fn ssh_options(host: &str, timeout: Duration) -> Result<CommandOptions, String> {
        let host = Self::validate_host(host)?;
        let connect_timeout = timeout.as_secs().clamp(1, 3);
        // Upstream 0.48.0 #2626: negotiate the v2 session JSON (Pi-family
        // included) first, fall back to the legacy v1 array for older hosts.
        let remote_command = remote_sessions_command(Self::BUNDLED_CLI_FALLBACK);
        Ok(CommandOptions {
            timeout,
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                format!("ConnectTimeout={connect_timeout}"),
                "--".to_string(),
                host,
                "sh".to_string(),
                "-lc".to_string(),
                remote_command,
            ],
            ..CommandOptions::default()
        })
    }

    /// Fetch the versioned, summary-only Codex cost payload from one trusted
    /// SSH host. The command intentionally never requests sessions, account
    /// metadata, project paths, or a remote shell chosen by the caller.
    pub(crate) async fn fetch_codex_cost_summary(
        &self,
        host: &str,
        history_days: u32,
        force_refresh: bool,
    ) -> Result<String, String> {
        let options = Self::codex_cost_options(host, history_days, force_refresh)?;
        let runner = Self::codex_cost_runner();
        let result = runner
            .run_async("ssh", None, &options)
            .await
            .map_err(|_| crate::codex_costs::REMOTE_CODEX_COST_UNAVAILABLE)?;
        if result.timed_out || result.exit_code != Some(0) {
            return Err(crate::codex_costs::REMOTE_CODEX_COST_UNAVAILABLE.to_string());
        }
        if result.text.len() > crate::codex_costs::MAX_REMOTE_CODEX_COST_BYTES {
            return Err(crate::codex_costs::REMOTE_CODEX_COST_INVALID.to_string());
        }
        Ok(result.text)
    }

    pub(crate) fn validate_codex_cost_host(host: &str) -> Result<String, String> {
        let host = Self::validate_host(host)?;
        if host.len() > 255 {
            return Err("host must not exceed 255 bytes".to_string());
        }
        Ok(host)
    }

    pub(crate) fn codex_cost_options(
        host: &str,
        history_days: u32,
        force_refresh: bool,
    ) -> Result<CommandOptions, String> {
        if !(1..=365).contains(&history_days) {
            return Err("history days must be between 1 and 365".to_string());
        }
        let refresh = if force_refresh { " --refresh" } else { "" };
        Self::codex_cli_options(
            host,
            &format!(
                "cost --provider codex --format json --summary-only --provider-native-only --days {history_days}{refresh}"
            ),
        )
    }

    /// Fetch the identified-request ledger (upstream #4447) over the same
    /// one-shot SSH boundary, with an explicit UTC millisecond window.
    pub(crate) async fn fetch_codex_identified_usage(
        &self,
        host: &str,
        history_days: u32,
        end_unix_ms: i64,
    ) -> Result<String, String> {
        let options = Self::codex_identified_options(host, history_days, end_unix_ms)?;
        let result = Self::codex_cost_runner()
            .run_async("ssh", None, &options)
            .await
            .map_err(|error| error.to_string())?;
        if result.timed_out || result.exit_code != Some(0) {
            return Err("remote identified usage command failed".to_string());
        }
        if result.text.len() > crate::codex_costs::MAX_IDENTIFIED_OUTPUT_BYTES {
            return Err("remote identified usage output is too large".to_string());
        }
        Ok(result.text)
    }

    pub(crate) fn codex_identified_options(
        host: &str,
        history_days: u32,
        end_unix_ms: i64,
    ) -> Result<CommandOptions, String> {
        if !(1..=365).contains(&history_days) || end_unix_ms < 0 {
            return Err("invalid identified usage window".to_string());
        }
        Self::codex_cli_options(
            host,
            &format!(
                "cost --provider codex --format json --identified-only --days {history_days} --identified-end {end_unix_ms}"
            ),
        )
    }

    /// Select the executable before scanning: a failed scan must never invoke
    /// a fallback scan.
    fn codex_cli_options(host: &str, cli_arguments: &str) -> Result<CommandOptions, String> {
        let host = Self::validate_codex_cost_host(host)?;
        let command = format!(
            "if command -v codexbar >/dev/null 2>&1; then exec codexbar {cli_arguments}; else exec /Applications/CodexBar.app/Contents/Helpers/CodexBarCLI {cli_arguments}; fi"
        );
        let remote_command = format!("'{command}'");

        Ok(CommandOptions {
            timeout: Duration::from_secs(60),
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                "ConnectTimeout=5".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=yes".to_string(),
                "-o".to_string(),
                "RemoteCommand=none".to_string(),
                "-o".to_string(),
                "RequestTTY=no".to_string(),
                "-o".to_string(),
                "ForwardAgent=no".to_string(),
                "-o".to_string(),
                "ClearAllForwardings=yes".to_string(),
                "-T".to_string(),
                "-n".to_string(),
                "--".to_string(),
                host,
                "sh".to_string(),
                "-lc".to_string(),
                remote_command,
            ],
            ..CommandOptions::default()
        })
    }

    fn codex_cost_runner() -> CommandRunner {
        const ALLOWED_ENVIRONMENT: &[&str] = &[
            "PATH",
            "PATHEXT",
            "HOME",
            "USER",
            "LOGNAME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "TEMP",
            "TMP",
            "SystemRoot",
            "SYSTEMROOT",
            "ComSpec",
            "COMSPEC",
            "LANG",
            "LC_ALL",
            "SSH_AUTH_SOCK",
        ];

        ALLOWED_ENVIRONMENT.iter().fold(
            CommandRunner::new().without_inherited_env(),
            |runner, key| match std::env::var(key) {
                Ok(value) => runner.with_env(*key, value),
                Err(_) => runner,
            },
        )
    }

    /// Runs `fetch` for every host concurrently. `None` marks a host skipped because
    /// the gate closed; it contributes no row.
    pub(crate) async fn fetch_hosts_with<F, Fut>(
        hosts: &[String],
        fetch: F,
    ) -> Vec<AgentSessionHostResult>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: Future<Output = Option<AgentSessionHostResult>>,
    {
        let outcomes = join_all(hosts.iter().cloned().map(|host| fetch.clone()(host))).await;
        let mut results = outcomes.into_iter().flatten().collect::<Vec<_>>();
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    fn decode_remote_sessions(host: &str, body: &str) -> Result<AgentSessionHostResult, String> {
        if let Ok(mut sessions) = serde_json::from_str::<Vec<AgentSession>>(body) {
            for session in &mut sessions {
                session.host = host.to_string();
            }
            return Ok(AgentSessionHostResult::success(host, sessions));
        }
        let mut result = Self::decode_host_result(body)?;
        result.host = host.to_string();
        for session in &mut result.sessions {
            session.host = host.to_string();
        }
        Ok(result)
    }

    pub fn sanitized_hosts(hosts: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut sanitized = Vec::new();

        for host in hosts {
            let Ok(host) = Self::validate_host(host) else {
                continue;
            };

            let key = Self::host_dedup_key(&host);
            if seen.insert(key) {
                sanitized.push(host);
            }
        }

        sanitized
    }

    pub fn merge_hosts(manual: &[String], automatic: &[String]) -> Vec<String> {
        Self::sanitized_hosts(&manual.iter().chain(automatic).cloned().collect::<Vec<_>>())
    }

    /// Deduplicate host names case-insensitively while preserving SSH username
    /// case. SSH may treat `Alice@host` and `alice@host` as different users,
    /// whereas the host component remains case-insensitive for this input.
    fn host_dedup_key(host: &str) -> String {
        let host = host.trim();
        if let Some(separator) = host.rfind('@') {
            let (prefix, hostname) = host.split_at(separator + 1);
            format!("{prefix}{}", hostname.to_ascii_lowercase())
        } else {
            host.to_ascii_lowercase()
        }
    }

    pub fn validate_host(host: &str) -> Result<String, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("host must not be empty".to_string());
        }
        if host.starts_with('-') {
            return Err("host must not start with '-'".to_string());
        }
        if host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || !is_safe_host_char(c))
        {
            return Err(
                "host must not contain whitespace, control characters, or unsafe shell characters"
                    .to_string(),
            );
        }

        Ok(host.to_string())
    }

    pub fn decode_host_result(body: &str) -> Result<AgentSessionHostResult, String> {
        let result: AgentSessionHostResult = serde_json::from_str(body)
            .map_err(|err| actionable_message("Unable to decode remote session response", err))?;
        Self::validate_host(&result.host).map_err(|err| {
            actionable_message("Remote session response has an invalid host", err)
        })?;
        Ok(result)
    }
}

impl Default for RemoteSessionFetcher {
    fn default() -> Self {
        Self {
            per_host_timeout: Duration::from_secs(5),
        }
    }
}

#[cfg(windows)]
pub(crate) const TAILSCALE_CLI_FILE_NAME: &str = "tailscale.exe";
#[cfg(not(windows))]
pub(crate) const TAILSCALE_CLI_FILE_NAME: &str = "tailscale";

/// First standalone Tailscale CLI on `path_var`, as a UTF-8 path.
///
/// Only the exact CLI file name qualifies. The Windows GUI client (`tailscale-ipn.exe`), `.cmd`/`.bat`
/// launchers and symlinks are never spawned, because running the VPN app's binary in status mode can
/// start its tunnel. Bare-name lookup through `which` is avoided for the same reason: it also honors
/// PATHEXT, so a launcher script would be selected ahead of the CLI.
pub(crate) fn tailscale_cli_path(path_var: Option<&std::ffi::OsStr>) -> Option<String> {
    let path_var = path_var?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(TAILSCALE_CLI_FILE_NAME))
        .find(|candidate| is_standalone_tailscale_cli(candidate))
        .and_then(|path| path.to_str().map(str::to_owned))
}

/// A regular file (not a symlink or directory) with a native executable header: PE (`MZ`) on
/// Windows, ELF elsewhere. Scripts and other launchers are rejected before any process starts.
pub(crate) fn is_standalone_tailscale_cli(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    magic[..2] == *b"MZ" || magic == *b"\x7fELF"
}

/// `--json-v2` (Pi-family aware) first, legacy `--json` for older installs —
/// upstream 0.48.0 #2626 negotiation, including the bundled macOS CLI
/// fallback path.
fn remote_sessions_command(bundled_cli_fallback: &str) -> String {
    [
        "codexbar sessions --json-v2",
        "codexbar sessions --json",
        &format!("'{bundled_cli_fallback}' sessions --json-v2"),
        &format!("'{bundled_cli_fallback}' sessions --json"),
    ]
    .join(" || ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_cost_options_are_noninteractive_and_summary_only() {
        let options = RemoteSessionFetcher::codex_cost_options("build-host", 30, false).unwrap();
        assert_eq!(options.timeout, Duration::from_secs(60));
        assert!(options.extra_args.contains(&"BatchMode=yes".to_string()));
        assert!(
            options
                .extra_args
                .contains(&"StrictHostKeyChecking=yes".to_string())
        );
        assert!(options.extra_args.contains(&"ForwardAgent=no".to_string()));
        assert!(
            options
                .extra_args
                .contains(&"RemoteCommand=none".to_string())
        );
        assert!(options.extra_args.contains(&"RequestTTY=no".to_string()));
        assert!(
            options
                .extra_args
                .contains(&"ClearAllForwardings=yes".to_string())
        );
        assert!(options.extra_args.contains(&"-T".to_string()));
        assert!(options.extra_args.contains(&"-n".to_string()));

        let separator = options
            .extra_args
            .iter()
            .position(|arg| arg == "--")
            .expect("SSH host separator is present");
        assert_eq!(
            &options.extra_args[separator..separator + 4],
            ["--", "build-host", "sh", "-lc"]
        );
        let remote_command = options
            .extra_args
            .get(separator + 4)
            .expect("SSH command is present");
        assert!(remote_command.starts_with('\''));
        assert!(remote_command.ends_with('\''));
        assert!(remote_command.contains("cost --provider codex --format json --summary-only"));
        assert!(remote_command.contains("--days 30"));
    }

    #[test]
    fn codex_cost_options_reject_unsafe_or_unbounded_targets() {
        assert!(RemoteSessionFetcher::codex_cost_options("-oProxyCommand=x", 30, false).is_err());
        assert!(RemoteSessionFetcher::codex_cost_options("build host", 30, false).is_err());
        assert!(RemoteSessionFetcher::codex_cost_options("build-host", 0, false).is_err());
        assert!(RemoteSessionFetcher::codex_cost_options("build-host", 366, false).is_err());
    }

    #[test]
    fn codex_identified_options_request_the_exact_window_noninteractively() {
        let options =
            RemoteSessionFetcher::codex_identified_options("build-host", 7, 1_700_000_000_000)
                .unwrap();
        assert!(options.extra_args.contains(&"BatchMode=yes".to_string()));
        assert!(
            options
                .extra_args
                .contains(&"StrictHostKeyChecking=yes".to_string())
        );
        let command = options.extra_args.last().unwrap();
        assert!(command.contains(
            "cost --provider codex --format json --identified-only --days 7 --identified-end 1700000000000"
        ));
        assert!(!command.contains("--summary-only"));
        assert!(RemoteSessionFetcher::codex_identified_options("-oProxyCommand=x", 7, 0).is_err());
        assert!(RemoteSessionFetcher::codex_identified_options("build-host", 0, 0).is_err());
        assert!(RemoteSessionFetcher::codex_identified_options("build-host", 7, -1).is_err());
    }
}
