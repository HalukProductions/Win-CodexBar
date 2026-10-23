//! Scripted stand-in for `grok login --oauth --leader-socket <path>`, used by
//! the CLI proof runs. It never writes a login file, so it cannot touch the
//! real Grok account store.
//!
//! The mode comes from the `FAKE_GROK_MODE` environment variable:
//! - `wait` (default): sleeps as if the browser sign-in were still open.
//! - `fail`: exits 3 without signing in.
//!
//! This is a cargo example, not a shipped binary. `cargo test` builds it into
//! `target/<profile>/examples`.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    match std::env::var("FAKE_GROK_MODE").unwrap_or_default().as_str() {
        "fail" => ExitCode::from(3),
        _ => {
            thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
    }
}
