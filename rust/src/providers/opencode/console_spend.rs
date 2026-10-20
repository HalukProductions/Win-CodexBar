//! OpenCode Console usage for migrated workspaces (upstream 0.71
//! #4131/#4139).
//!
//! A `__Host-console_session` cookie reads usage through the Console APIs
//! that OpenCode Go already uses: workspace discovery, then Go quota. A
//! workspace without quota is published as spend only after the Console
//! confirms it has no Go subscription and bills pay-as-you-go; its spend is
//! the rolling 30-day usage summary.

use serde_json::Value;
use std::time::Duration;

use crate::core::{ProviderError, ProviderFetchResult};
use crate::providers::opencodego::console::{self as go_console, ConsoleUsage};

use super::OpenCodeProvider;
use super::billing::{OpenCodeBillingPeriod, OpenCodeZenBilling};

const ORG_CURRENT_PATH: &str = "/console/api/orgs/current";
const USAGE_SUMMARY_PATH: &str = "/console/api/usage/summary?range=30d";
const MICRO_CENTS_PER_USD: f64 = 100_000_000.0;

impl OpenCodeProvider {
    pub(super) async fn fetch_console_workspace_id(
        &self,
        cookie_header: &str,
        timeout: Duration,
    ) -> Result<String, ProviderError> {
        go_console::fetch_workspace_id(&self.client, &self.origin, cookie_header, timeout).await
    }

    pub(super) async fn fetch_console_usage(
        &self,
        workspace_id: &str,
        cookie_header: &str,
        timeout: Duration,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let usage = go_console::fetch_usage(
            &self.client,
            &self.origin,
            workspace_id,
            cookie_header,
            timeout,
        )
        .await?;
        match usage {
            ConsoleUsage::Snapshot(usage) => {
                let mut usage = (*usage).with_login_method("OpenCode");
                // The OpenCode card shows the rolling and weekly windows only;
                // the monthly Go meter belongs to the OpenCode Go provider.
                usage.tertiary = None;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            ConsoleUsage::NoSubscription => {
                self.fetch_console_pay_as_you_go(workspace_id, cookie_header, timeout)
                    .await
            }
        }
    }

    /// A null Go status means no quota. The workspace's subscription state and
    /// billing mode are confirmed before spend is published alone, so an
    /// inaccessible quota is never presented as a pay-as-you-go account.
    async fn fetch_console_pay_as_you_go(
        &self,
        workspace_id: &str,
        cookie_header: &str,
        timeout: Duration,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let organization = self
            .console_text(
                ORG_CURRENT_PATH,
                workspace_id,
                cookie_header,
                timeout,
                "organization",
            )
            .await?;
        if !organization_lacks_go_subscription(&organization) {
            return Err(ProviderError::Parse(
                "OpenCode Console subscription usage is unavailable".to_string(),
            ));
        }

        let billing = self
            .console_text(
                go_console::CONSOLE_BILLING_STATUS_PATH,
                workspace_id,
                cookie_header,
                timeout,
                "billing status",
            )
            .await?;
        if !is_pay_as_you_go_billing(&billing) {
            return Err(ProviderError::Parse(
                "No supported OpenCode Console pay-as-you-go billing is available".to_string(),
            ));
        }

        let summary = self
            .console_text(
                USAGE_SUMMARY_PATH,
                workspace_id,
                cookie_header,
                timeout,
                "usage summary",
            )
            .await?;
        let usage_usd = parse_usage_summary(&summary)?;
        // Missing or unsupported balance data does not erase spend for a
        // confirmed pay-as-you-go account.
        let balance_usd = go_console::parse_balance(&billing).ok().flatten();
        Ok(self.pay_as_you_go_result(&OpenCodeZenBilling {
            monthly_usage_usd: usage_usd,
            monthly_limit_usd: None,
            balance_usd,
            has_subscription: false,
            period: OpenCodeBillingPeriod::Last30Days,
        }))
    }

    async fn console_text(
        &self,
        path: &str,
        workspace_id: &str,
        cookie_header: &str,
        timeout: Duration,
        what: &str,
    ) -> Result<String, ProviderError> {
        go_console::fetch_text(
            &self.client,
            &format!("{}{path}", self.origin),
            Some(workspace_id),
            cookie_header,
            timeout,
            what,
        )
        .await
    }
}

fn organization_lacks_go_subscription(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|root| root.get("hasGoSubscription")?.as_bool())
        == Some(false)
}

fn is_pay_as_you_go_billing(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .is_some_and(|root| root.get("mode").and_then(Value::as_str) == Some("pay-as-you-go"))
}

/// `totalCostMicroCents` arrives as a digit string or a non-negative integer.
/// Anything else (negative, fractional, boolean, null) is not a spend total.
fn parse_usage_summary(text: &str) -> Result<f64, ProviderError> {
    let invalid = || ProviderError::Parse("Invalid OpenCode Console usage summary".to_string());
    let root: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "microcent totals stay far below 2^53 for any real 30-day spend"
    )]
    let micro_cents = match root.get("totalCostMicroCents") {
        Some(Value::String(raw))
            if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            raw.parse::<f64>().ok().filter(|value| value.is_finite())
        }
        Some(Value::Number(number)) => number.as_u64().map(|value| value as f64),
        _ => None,
    }
    .ok_or_else(invalid)?;
    Ok(micro_cents / MICRO_CENTS_PER_USD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microcent_totals_preserve_scale() {
        for (raw, usd) in [
            ("0", 0.0),
            ("5520922478", 55.209_224_78),
            (r#""6653244286""#, 66.532_442_86),
            (r#""325000000""#, 3.25),
        ] {
            let amount = parse_usage_summary(&format!(r#"{{"totalCostMicroCents":{raw}}}"#))
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert!((amount - usd).abs() < 1e-9, "{raw}: {amount}");
        }
    }

    #[test]
    fn malformed_microcent_totals_fail_instead_of_becoming_spend() {
        for raw in [
            "true", "-1", "1.5", r#""NaN""#, "null", r#""-1""#, r#""1.5""#, r#""""#,
        ] {
            let error =
                parse_usage_summary(&format!(r#"{{"totalCostMicroCents":{raw}}}"#)).expect_err(raw);
            assert!(matches!(error, ProviderError::Parse(_)), "{raw}: {error:?}");
        }
        assert!(parse_usage_summary("{}").is_err());
        assert!(parse_usage_summary("not json").is_err());
    }

    #[test]
    fn organization_and_billing_guards_require_explicit_values() {
        assert!(organization_lacks_go_subscription(
            r#"{"id":"wrk_CONSOLE123","hasGoSubscription":false}"#
        ));
        assert!(!organization_lacks_go_subscription(
            r#"{"hasGoSubscription":true}"#
        ));
        assert!(!organization_lacks_go_subscription(
            r#"{"id":"wrk_CONSOLE123"}"#
        ));
        assert!(!organization_lacks_go_subscription("null"));

        assert!(is_pay_as_you_go_billing(
            r#"{"billingMode":"prepaid","mode":"pay-as-you-go"}"#
        ));
        assert!(!is_pay_as_you_go_billing(
            r#"{"billingMode":"credit","mode":"invoiceable"}"#
        ));
        assert!(!is_pay_as_you_go_billing("not json"));
    }
}
