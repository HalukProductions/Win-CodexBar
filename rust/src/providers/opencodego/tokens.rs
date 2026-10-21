//! Recorded local token counts for OpenCode Go (upstream 0.67.0).
//!
//! OpenCode stores `tokens: {total, input, output, reasoning, cache: {read, write}}`
//! on assistant messages and `step-finish` parts. Costs stay the recorded `cost`
//! field; tokens are never used to derive a cost. A row whose tokens are absent,
//! malformed, negative, or overflowing has *unknown* tokens (never zero), and any
//! aggregate containing such a row has no complete token total.

use serde::Deserialize;

use crate::cost_scanner::ModelTokenCounts;

#[derive(Debug, Deserialize)]
struct RawTokens {
    total: Option<i64>,
    input: Option<i64>,
    output: Option<i64>,
    reasoning: Option<i64>,
    cache: Option<RawCache>,
}

#[derive(Debug, Deserialize)]
struct RawCache {
    read: Option<i64>,
    write: Option<i64>,
}

/// Usable token counts for one row. Classes the row did not record stay `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowTokens {
    pub(super) total: u64,
    pub(super) input: Option<u64>,
    pub(super) output: Option<u64>,
    pub(super) reasoning: Option<u64>,
    pub(super) cache_read: Option<u64>,
    pub(super) cache_write: Option<u64>,
}

/// Outer `None` = invalid (negative); inner `None` = the class was not recorded.
fn non_negative(value: Option<i64>) -> Option<Option<u64>> {
    match value {
        None => Some(None),
        Some(count) => u64::try_from(count).ok().map(Some),
    }
}

impl RowTokens {
    /// Parse a `$.tokens` JSON object. `None` when malformed, when any present
    /// count (including `total`) is negative, or when no total can be resolved.
    pub(crate) fn parse(json: &str) -> Option<Self> {
        let raw: RawTokens = serde_json::from_str(json).ok()?;
        let cache = raw.cache.as_ref();
        let total = non_negative(raw.total)?;
        let input = non_negative(raw.input)?;
        let output = non_negative(raw.output)?;
        let reasoning = non_negative(raw.reasoning)?;
        let cache_read = non_negative(cache.and_then(|c| c.read))?;
        let cache_write = non_negative(cache.and_then(|c| c.write))?;
        // OpenCode separates output from reasoning and input from cache counts, so
        // the five components sum to the total when it was not recorded.
        let total = match total {
            Some(total) => total,
            None => [input, output, reasoning, cache_read, cache_write]
                .into_iter()
                .try_fold(0u64, |sum, value| sum.checked_add(value?))?,
        };
        Some(Self {
            total,
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
        })
    }
}

/// Token sums over a bucket of rows (a day, a model, or a window).
///
/// Class sums add only what rows recorded. Completeness is tracked separately so
/// callers never present a partial sum as an exact total.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenSums {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    rows: u32,
    unusable_rows: u32,
    rows_with_input: u32,
    rows_with_output: u32,
    rows_with_reasoning: u32,
}

impl TokenSums {
    /// Add one row. `None` (or a row whose sums would overflow) makes the bucket incomplete.
    pub(crate) fn add(&mut self, tokens: Option<&RowTokens>) {
        self.rows = self.rows.saturating_add(1);
        let Some(tokens) = tokens else {
            self.unusable_rows = self.unusable_rows.saturating_add(1);
            return;
        };
        let sums = (
            self.input.checked_add(tokens.input.unwrap_or(0)),
            self.output.checked_add(tokens.output.unwrap_or(0)),
            self.reasoning.checked_add(tokens.reasoning.unwrap_or(0)),
            self.cache_read.checked_add(tokens.cache_read.unwrap_or(0)),
            self.cache_write
                .checked_add(tokens.cache_write.unwrap_or(0)),
            self.total.checked_add(tokens.total),
        );
        let (
            Some(input),
            Some(output),
            Some(reasoning),
            Some(cache_read),
            Some(cache_write),
            Some(total),
        ) = sums
        else {
            self.unusable_rows = self.unusable_rows.saturating_add(1);
            return;
        };
        self.input = input;
        self.output = output;
        self.reasoning = reasoning;
        self.cache_read = cache_read;
        self.cache_write = cache_write;
        self.total = total;
        if tokens.input.is_some() {
            self.rows_with_input = self.rows_with_input.saturating_add(1);
        }
        if tokens.output.is_some() {
            self.rows_with_output = self.rows_with_output.saturating_add(1);
        }
        if tokens.reasoning.is_some() {
            self.rows_with_reasoning = self.rows_with_reasoning.saturating_add(1);
        }
    }

    /// True when at least one row carried usable tokens.
    pub fn has_usable_rows(&self) -> bool {
        self.rows > self.unusable_rows
    }

    /// Rows whose tokens were missing, malformed, negative or overflowing. Any
    /// such row makes every token total in the bucket unknown.
    pub fn unusable_rows(&self) -> u32 {
        self.unusable_rows
    }

    /// Total tokens, only when every row in the bucket had usable tokens.
    pub fn complete_total(&self) -> Option<u64> {
        (self.rows > 0 && self.unusable_rows == 0).then_some(self.total)
    }

    /// Input + output tokens, only when every row recorded both classes.
    pub fn complete_input_output(&self) -> Option<u64> {
        if self.rows > 0
            && self.unusable_rows == 0
            && self.rows_with_input == self.rows
            && self.rows_with_output == self.rows
        {
            self.input.checked_add(self.output)
        } else {
            None
        }
    }

    /// Reasoning tokens, only when every row in the bucket recorded them.
    pub fn complete_reasoning(&self) -> Option<u64> {
        (self.rows > 0 && self.rows_with_reasoning == self.rows).then_some(self.reasoning)
    }

    /// Shared cost-summary token counts. `cached_tokens` follows the local
    /// Claude/Pi convention (cache read + cache write) because the shared
    /// summary has no separate cache-write class. `None` when no row had usable
    /// tokens or the combined cache count cannot be represented.
    pub fn to_model_token_counts(&self) -> Option<ModelTokenCounts> {
        if !self.has_usable_rows() {
            return None;
        }
        let cached_tokens = self.cache_read.checked_add(self.cache_write)?;
        Some(ModelTokenCounts {
            input_tokens: self.input,
            output_tokens: self.output,
            cached_tokens,
            reasoning_tokens: self.complete_reasoning(),
        })
    }
}
