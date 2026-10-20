//! The external tool CodexBar runs to list Claude accounts.
//!
//! Every source prints claude-swap's list schema v1, so one parser and one
//! projection serve them all. A source only fixes the argument array, the
//! account identity prefix and the label shown on cards.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClaudeSwapAccountSource {
    /// claude-swap (`cswap --list --json`), github.com/realiti4/claude-swap.
    #[default]
    #[serde(rename = "claude-swap")]
    ClaudeSwap,
    /// tcr (`tcr list --json`), the teamclaude-rs pooling proxy. tcr routes
    /// each request to a pooled account, so its list reports no active account
    /// and `supportsAccountSwitching: false`.
    #[serde(rename = "tcr")]
    Tcr,
}

impl ClaudeSwapAccountSource {
    pub const ALL: [Self; 2] = [Self::ClaudeSwap, Self::Tcr];

    /// Identity prefix (`<source>:<slot>`), config value and card label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeSwap => "claude-swap",
            Self::Tcr => "tcr",
        }
    }

    /// The fixed list argument array. Never a shell, never config-defined arguments.
    pub fn list_arguments(self) -> Vec<String> {
        let arguments: &[&str] = match self {
            Self::ClaudeSwap => &["--list", "--json"],
            Self::Tcr => &["list", "--json"],
        };
        arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect()
    }

    /// Only claude-swap activates slots; tcr picks the account for each request.
    pub fn supports_switching(self) -> bool {
        matches!(self, Self::ClaudeSwap)
    }

    /// A known config value, trimmed. Unknown values return `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        Self::ALL
            .into_iter()
            .find(|source| source.label() == trimmed)
    }

    /// An unset or unrecognised config value reads as claude-swap, the source
    /// that predates the key.
    pub fn from_config(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_source_runs_its_own_fixed_list_arguments() {
        assert_eq!(
            ClaudeSwapAccountSource::ClaudeSwap.list_arguments(),
            vec!["--list", "--json"]
        );
        assert_eq!(
            ClaudeSwapAccountSource::Tcr.list_arguments(),
            vec!["list", "--json"]
        );
    }

    #[test]
    fn config_resolves_the_source_and_treats_unknown_values_as_claude_swap() {
        use ClaudeSwapAccountSource::*;
        assert_eq!(ClaudeSwapAccountSource::from_config(None), ClaudeSwap);
        assert_eq!(ClaudeSwapAccountSource::from_config(Some("tcr")), Tcr);
        assert_eq!(ClaudeSwapAccountSource::from_config(Some(" tcr ")), Tcr);
        assert_eq!(
            ClaudeSwapAccountSource::from_config(Some("something-else")),
            ClaudeSwap
        );
        assert_eq!(ClaudeSwapAccountSource::parse("something-else"), None);
        assert_eq!(serde_json::to_value(Tcr).unwrap(), "tcr");
        assert_eq!(serde_json::to_value(ClaudeSwap).unwrap(), "claude-swap");
    }

    #[test]
    fn only_claude_swap_switches_accounts() {
        assert!(ClaudeSwapAccountSource::ClaudeSwap.supports_switching());
        assert!(!ClaudeSwapAccountSource::Tcr.supports_switching());
    }
}
