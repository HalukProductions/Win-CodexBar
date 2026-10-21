//! OpenCode Go's `codexbar cost` extension (upstream #4629): per-model token
//! counts in the text report and `byModelTokens` in the JSON payload. Tokens
//! that rows did not record are reported as unknown, never as zero. Costs stay
//! the recorded `cost` field.

use serde_json::{Map, Value, json};

use crate::core::{CostCliExtension, format_count};
use crate::cost_scanner::CostSummary;

/// Provider key for the shared total rule (cache counts are separate from input).
const PROVIDER: &str = "opencodego";

pub(crate) static OPENCODEGO_COST_CLI: OpenCodeGoCostCli = OpenCodeGoCostCli;

pub(crate) struct OpenCodeGoCostCli;

/// Unknown-token rows for one model, `0` when every row recorded usable tokens.
fn unknown_rows(summary: &CostSummary, model: &str) -> u32 {
    summary
        .unknown_token_rows_by_model
        .get(model)
        .copied()
        .unwrap_or(0)
}

impl CostCliExtension for OpenCodeGoCostCli {
    fn model_token_detail(&self, summary: &CostSummary, model: &str) -> Option<String> {
        let unknown = unknown_rows(summary, model);
        if unknown > 0 {
            return Some(format!(
                "tokens unknown ({unknown} rows lacked complete token counts)"
            ));
        }
        let counts = summary.by_model_tokens.get(model)?;
        Some(format!(
            "{} input, {} output, {} cached",
            format_count(counts.input_tokens),
            format_count(counts.output_tokens),
            format_count(counts.cached_tokens)
        ))
    }

    fn json_fields(&self, summary: &CostSummary) -> Vec<(&'static str, Value)> {
        let mut models: Vec<&String> = summary.by_model.keys().collect();
        models.sort();
        let by_model: Map<String, Value> = models
            .into_iter()
            .map(|model| {
                let counts = summary
                    .by_model_tokens
                    .get(model)
                    .filter(|_| unknown_rows(summary, model) == 0);
                let value = match counts {
                    Some(counts) => json!({
                        "inputTokens": counts.input_tokens,
                        "outputTokens": counts.output_tokens,
                        "cachedTokens": counts.cached_tokens,
                        "reasoningTokens": counts.reasoning_tokens,
                        "totalTokens": counts.total_for_provider(PROVIDER),
                        "unknownTokenRowCount": 0,
                    }),
                    None => json!({
                        "inputTokens": Value::Null,
                        "outputTokens": Value::Null,
                        "cachedTokens": Value::Null,
                        "reasoningTokens": Value::Null,
                        "totalTokens": Value::Null,
                        "unknownTokenRowCount": unknown_rows(summary, model),
                    }),
                };
                (model.clone(), value)
            })
            .collect();
        vec![("byModelTokens", Value::Object(by_model))]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::cli::cost_period::cost_totals_json;
    use crate::cost_scanner::ModelTokenCounts;

    fn summary_with_unknown_glm() -> CostSummary {
        CostSummary {
            total_cost_usd: 1.5,
            input_tokens: 1000,
            output_tokens: 400,
            cached_tokens: 50,
            sessions_count: 3,
            by_model: HashMap::from([("kimi-k2".to_string(), 1.0), ("glm-5".to_string(), 0.5)]),
            by_model_tokens: HashMap::from([(
                "kimi-k2".to_string(),
                ModelTokenCounts {
                    input_tokens: 1000,
                    output_tokens: 400,
                    cached_tokens: 50,
                    reasoning_tokens: None,
                },
            )]),
            unknown_token_rows: 2,
            unknown_token_rows_by_model: HashMap::from([("glm-5".to_string(), 2)]),
            ..CostSummary::default()
        }
    }

    #[test]
    fn unknown_model_tokens_render_as_unknown_not_zero() {
        let summary = summary_with_unknown_glm();
        assert_eq!(
            OPENCODEGO_COST_CLI.model_token_detail(&summary, "glm-5"),
            Some("tokens unknown (2 rows lacked complete token counts)".to_string())
        );
        assert_eq!(
            OPENCODEGO_COST_CLI.model_token_detail(&summary, "kimi-k2"),
            Some("1,000 input, 400 output, 50 cached".to_string())
        );
    }

    #[test]
    fn json_reports_unknown_tokens_as_null() {
        let summary = summary_with_unknown_glm();
        let fields = OPENCODEGO_COST_CLI.json_fields(&summary);
        let (key, by_model) = &fields[0];
        assert_eq!(*key, "byModelTokens");
        assert_eq!(by_model["kimi-k2"]["inputTokens"], 1000);
        assert_eq!(by_model["kimi-k2"]["totalTokens"], 1450);
        assert!(by_model["glm-5"]["inputTokens"].is_null());
        assert!(by_model["glm-5"]["totalTokens"].is_null());
        assert_eq!(by_model["glm-5"]["unknownTokenRowCount"], 2);
    }

    #[test]
    fn totals_json_nulls_tokens_but_keeps_cost() {
        let summary = summary_with_unknown_glm();
        let totals = cost_totals_json("opencodego", &summary);
        assert!(totals["inputTokens"].is_null());
        assert!(totals["totalTokens"].is_null());
        assert_eq!(totals["totalCost"], 1.5);
    }

    #[test]
    fn known_tokens_keep_numeric_totals() {
        let summary = CostSummary {
            unknown_token_rows: 0,
            ..summary_with_unknown_glm()
        };
        let totals = cost_totals_json("opencodego", &summary);
        assert_eq!(totals["inputTokens"], 1000);
        assert_eq!(totals["totalTokens"], 1450);
        assert!(summary.tokens_known());
        assert_eq!(
            OPENCODEGO_COST_CLI.json_fields(&summary)[0].1["kimi-k2"]["totalTokens"],
            1450
        );
    }
}
