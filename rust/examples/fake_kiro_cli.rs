//! Scripted stand-in for `kiro-cli login`, used by the login runner tests and
//! the CLI proof runs.
//!
//! Usage: `fake_kiro_cli login <mode>`, or `fake_kiro_cli login` with the mode
//! in the `FAKE_KIRO_MODE` environment variable (what `config login` runs).
//!
//! Modes:
//! - `success`: prints a sign-in URL and the success marker, then exits 0.
//! - `wait`: prints a sign-in URL, then waits for the browser (sleeps).
//! - `silent`: prints nothing and sleeps.
//! - anything else: exits 3.
//!
//! This is a cargo example, not a shipped binary. `cargo test` builds it next to
//! the test binary, so no dependency or PATH setup is needed.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const SIGN_IN_URL: &str = "https://example.com/device?code=ABCD-1234";

fn main() -> ExitCode {
    let mode = std::env::args()
        .nth(2)
        .or_else(|| std::env::var("FAKE_KIRO_MODE").ok())
        .unwrap_or_default();
    match mode.as_str() {
        "success" => {
            println!("Open {SIGN_IN_URL} to continue");
            println!("Successfully logged in");
            ExitCode::SUCCESS
        }
        "wait" => {
            println!("Open {SIGN_IN_URL} to continue");
            thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        "silent" => {
            thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        _ => ExitCode::from(3),
    }
}
