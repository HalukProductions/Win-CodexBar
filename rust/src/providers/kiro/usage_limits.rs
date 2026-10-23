use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::OptionalExtension;
use serde::Deserialize;

use crate::core::{ProviderError, RateWindow, UsageSnapshot};

const US_EAST_ENDPOINT: &str = "https://codewhisperer.us-east-1.amazonaws.com/";
const FRANKFURT_ENDPOINT: &str = "https://q.eu-central-1.amazonaws.com/";
const TARGET: &str = "AmazonCodeWhispererService.GetUsageLimits";
const TOKEN_KEY: &str = "kirocli:odic:token";
const PROFILE_KEY: &str = "api.codewhisperer.profile";
/// Upper bound for the unauthenticated reachability check before a background CLI run.
const REACHABILITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

#[derive(Debug, Clone)]
pub(super) struct KiroUsageLimits {
    pub plan_limit: f64,
    pub plan_used: f64,
    pub overage_used: f64,
    pub overage_cap: Option<f64>,
    pub overage_enabled: Option<bool>,
    pub overage_charges: Option<f64>,
    pub overage_rate: Option<f64>,
    pub currency_code: String,
    pub resets_at: DateTime<Utc>,
    pub has_unseparated_bonus: bool,
}

#[derive(Debug)]
struct KiroIdentity {
    access_token: String,
    profile_arn: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageLimitsResponse {
    usage_breakdown_list: Vec<UsageBreakdown>,
    overage_configuration: Option<OverageConfiguration>,
    next_date_reset: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageBreakdown {
    resource_type: String,
    current_usage_with_precision: f64,
    usage_limit_with_precision: f64,
    current_overages_with_precision: Option<f64>,
    overage_cap_with_precision: Option<f64>,
    overage_charges: Option<f64>,
    overage_rate: Option<f64>,
    currency: Option<String>,
    next_date_reset: Option<f64>,
    bonuses: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverageConfiguration {
    overage_status: String,
}

pub(super) async fn fetch_usage_limits() -> Result<KiroUsageLimits, ProviderError> {
    let identity = read_identity(&state_database_path())?;
    let endpoint = endpoint_for_profile_arn(&identity.profile_arn).ok_or_else(|| {
        ProviderError::Other("Kiro profile ARN is unsupported for usage enrichment".into())
    })?;
    let client = crate::core::credentialed_http_client_builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| ProviderError::Other(error.to_string()))?;
    let response = client
        .post(endpoint)
        .header("Content-Type", "application/x-amz-json-1.0")
        .header("X-Amz-Target", TARGET)
        .header("Authorization", format!("Bearer {}", identity.access_token))
        .json(&serde_json::json!({"profileArn": identity.profile_arn}))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(ProviderError::AuthRequired);
    }
    if !response.status().is_success() {
        return Err(ProviderError::Other(format!(
            "Kiro GetUsageLimits returned HTTP {}",
            response.status()
        )));
    }
    let bytes = response.bytes().await?;
    parse_usage_limits(&bytes)
}

pub(super) fn apply_usage_limits(
    mut usage: UsageSnapshot,
    limits: &KiroUsageLimits,
) -> UsageSnapshot {
    if !limits.has_unseparated_bonus && limits.plan_limit > 0.0 {
        usage.primary.used_percent =
            (limits.plan_used / limits.plan_limit * 100.0).clamp(0.0, 100.0);
        usage.primary.resets_at = Some(limits.resets_at);
        usage.primary.window_minutes = RateWindow::monthly_window_minutes(Some(limits.resets_at));
        usage.primary.reset_description = None;
        usage.primary.is_informational = false;
    }

    if limits.overage_enabled == Some(false) {
        usage
            .extra_rate_windows
            .retain(|row| !row.id.starts_with("kiro-overage-"));
        return usage;
    }

    if let Some(cap) = limits.overage_cap.filter(|cap| *cap > 0.0) {
        usage
            .extra_rate_windows
            .retain(|row| row.id != "kiro-overage-credits");
        let percent = (limits.overage_used / cap * 100.0).clamp(0.0, 100.0);
        let window = RateWindow::with_details(
            percent,
            None,
            Some(limits.resets_at),
            Some(format!("{:.2} of {:.2} credits", limits.overage_used, cap)),
        );
        usage = usage.with_extra_rate_window("kiro-overage-credits", "Overage usage", window);
    }

    if let Some(charges) = limits.overage_charges {
        usage
            .extra_rate_windows
            .retain(|row| row.id != "kiro-overage-cost");
        let budget = limits
            .overage_cap
            .zip(limits.overage_rate)
            .map(|(cap, rate)| cap * rate)
            .filter(|value| value.is_finite() && *value > 0.0);
        let currency = limits.currency_code.trim();
        let description = match budget {
            Some(limit) => format!("{charges:.2} of {limit:.2} {currency}"),
            None => format!("{charges:.2} {currency}"),
        };
        let window = RateWindow::informational(description);
        usage = usage.with_extra_rate_window("kiro-overage-cost", "Overage cost", window);
    }

    usage
}

pub(super) fn parse_usage_limits(data: &[u8]) -> Result<KiroUsageLimits, ProviderError> {
    let response: UsageLimitsResponse = serde_json::from_slice(data)
        .map_err(|error| ProviderError::Parse(format!("Kiro usage-limits JSON: {error}")))?;
    let mut credits = response
        .usage_breakdown_list
        .iter()
        .filter(|row| row.resource_type == "CREDIT");
    let credit = credits
        .next()
        .ok_or_else(|| ProviderError::Parse("Kiro usage limits reported no CREDIT row".into()))?;
    if credits.next().is_some() {
        return Err(ProviderError::Parse(
            "Kiro usage limits reported several CREDIT rows".into(),
        ));
    }

    let plan_limit = usable(credit.usage_limit_with_precision, "plan limit")?;
    let total_used = usable(credit.current_usage_with_precision, "usage")?;
    let overage_used = usable(
        credit.current_overages_with_precision.unwrap_or(0.0),
        "overage usage",
    )?;
    if total_used < overage_used {
        return Err(ProviderError::Parse(
            "Kiro overage exceeds total usage".into(),
        ));
    }
    let plan_used = total_used - overage_used;
    let has_unseparated_bonus = credit.bonuses.as_ref().is_some_and(|rows| !rows.is_empty());
    if !has_unseparated_bonus && plan_used > plan_limit {
        return Err(ProviderError::Parse(
            "Kiro plan usage exceeds plan limit".into(),
        ));
    }

    let overage_availability = match response
        .overage_configuration
        .as_ref()
        .map(|config| config.overage_status.trim().to_ascii_uppercase())
        .as_deref()
    {
        Some("ENABLED") => Some(true),
        Some("DISABLED") => Some(false),
        _ => None,
    };
    let overage_cap = if overage_availability == Some(true) {
        credit
            .overage_cap_with_precision
            .map(|value| usable(value, "overage cap"))
            .transpose()?
    } else {
        None
    };
    let overage_enabled = if overage_availability == Some(true) && overage_cap.is_none() {
        None
    } else {
        overage_availability
    };
    let reset_seconds = credit
        .next_date_reset
        .or(response.next_date_reset)
        .ok_or_else(|| ProviderError::Parse("Kiro usage limits reported no reset date".into()))?;
    let resets_at = reset_date(reset_seconds).ok_or_else(|| {
        ProviderError::Parse("Kiro usage limits reset date is implausible".into())
    })?;

    Ok(KiroUsageLimits {
        plan_limit,
        plan_used,
        overage_used,
        overage_cap,
        overage_enabled,
        overage_charges: finite_nonnegative(credit.overage_charges),
        overage_rate: finite_positive(credit.overage_rate),
        currency_code: credit
            .currency
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("USD")
            .to_string(),
        resets_at,
        has_unseparated_bonus,
    })
}

/// Unauthenticated reachability check for the Kiro usage host, run before a
/// background `kiro-cli` probe. Background refreshes that cannot reach the host
/// skip the CLI, because its sign-in attempt opens browser tabs when offline.
/// The check sends no token and no cookies. Any HTTP response proves
/// reachability; only a transport failure returns an error.
pub(super) async fn ensure_reachable() -> Result<(), ProviderError> {
    ensure_reachable_at(reachability_endpoint(&state_database_path())).await
}

/// Usage host for the CLI's profile. Missing or unsupported region state
/// falls back to US East, matching upstream.
fn reachability_endpoint(database: &Path) -> &'static str {
    read_profile_arn(database)
        .as_deref()
        .and_then(endpoint_for_profile_arn)
        .unwrap_or(US_EAST_ENDPOINT)
}

async fn ensure_reachable_at(endpoint: &str) -> Result<(), ProviderError> {
    let client = crate::core::apply_app_proxy(reqwest::Client::builder())
        .timeout(REACHABILITY_TIMEOUT)
        .build()
        .map_err(|error| ProviderError::Other(error.to_string()))?;
    // The shared classifier retains only refused connections, but offline
    // Windows hosts usually fail DNS first. Every connect failure here means the
    // host is unreachable, so it must keep the cached snapshot like upstream.
    client.head(endpoint).send().await.map_err(|error| {
        if error.is_connect() {
            ProviderError::Timeout
        } else {
            ProviderError::Network(error)
        }
    })?;
    Ok(())
}

/// The profile ARN alone, without the access token. Used only for the
/// reachability endpoint, so a signed-out CLI still gets a host choice.
fn read_profile_arn(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    let connection = crate::core::open_readonly_sqlite_connection(
        path,
        crate::core::DEFAULT_SQLITE_BUSY_TIMEOUT,
    )
    .ok()?;
    let profile_json: Option<String> = connection
        .query_row(
            "SELECT value FROM state WHERE key = ?1",
            [PROFILE_KEY],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    json_string(profile_json.as_deref(), "arn", false)
}

fn state_database_path() -> PathBuf {
    if let Some(path) = std::env::var_os("KIRO_DATA_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join("data.sqlite3");
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
        .join("Kiro-Cli")
        .join("data.sqlite3")
}

fn read_identity(path: &Path) -> Result<KiroIdentity, ProviderError> {
    if !path.is_file() {
        return Err(ProviderError::NotInstalled(format!(
            "Kiro CLI state database not found at {}",
            path.display()
        )));
    }
    let connection = crate::core::open_readonly_sqlite_connection(
        path,
        crate::core::DEFAULT_SQLITE_BUSY_TIMEOUT,
    )
    .map_err(|error| ProviderError::Other(format!("Kiro state database: {error}")))?;
    let token_json: Option<String> = connection
        .query_row(
            "SELECT value FROM auth_kv WHERE key = ?1",
            [TOKEN_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| ProviderError::Other(format!("Kiro token lookup: {error}")))?;
    let profile_json: Option<String> = connection
        .query_row(
            "SELECT value FROM state WHERE key = ?1",
            [PROFILE_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| ProviderError::Other(format!("Kiro profile lookup: {error}")))?;
    let access_token = json_string(token_json.as_deref(), "access_token", true)
        .ok_or(ProviderError::AuthRequired)?;
    let profile_arn =
        json_string(profile_json.as_deref(), "arn", false).ok_or(ProviderError::AuthRequired)?;
    Ok(KiroIdentity {
        access_token,
        profile_arn,
    })
}

/// A non-empty string field of a JSON document; the ARN is matched untrimmed.
fn json_string(json: Option<&str>, key: &str, trim: bool) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(json?).ok()?;
    let text = value.get(key)?.as_str()?;
    let text = if trim { text.trim() } else { text };
    (!text.is_empty()).then(|| text.to_string())
}

fn endpoint_for_profile_arn(profile_arn: &str) -> Option<&'static str> {
    if profile_arn
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let fields: Vec<_> = profile_arn.splitn(6, ':').collect();
    if fields.len() != 6
        || fields[0] != "arn"
        || fields[1] != "aws"
        || fields[2] != "codewhisperer"
        || fields[5]
            .strip_prefix("profile/")
            .is_none_or(|name| name.is_empty())
    {
        return None;
    }
    match fields[3] {
        "us-east-1" => Some(US_EAST_ENDPOINT),
        "eu-central-1" => Some(FRANKFURT_ENDPOINT),
        _ => None,
    }
}

fn usable(value: f64, field: &str) -> Result<f64, ProviderError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(ProviderError::Parse(format!("Kiro has no usable {field}")))
    }
}

fn finite_nonnegative(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn finite_positive(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

fn reset_date(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() || !(1_000_000_000.0..=4_102_444_800.0).contains(&value) {
        return None;
    }
    // Range-checked above to valid Unix seconds.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "value is range-checked above to valid Unix seconds"
    )]
    let seconds = value as i64;
    Utc.timestamp_opt(seconds, 0).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsageWithPrecision": 70.0,
                "usageLimitWithPrecision": 60.0,
                "currentOveragesWithPrecision": 10.0,
                "overageCapWithPrecision": 20.0,
                "overageCharges": 5.0,
                "overageRate": 0.5,
                "currency": "USD",
                "nextDateReset": 1798761600.0,
                "bonuses": []
            }],
            "overageConfiguration": {"overageStatus": "ENABLED"}
        }))
        .unwrap()
    }

    #[test]
    fn parses_plan_overage_cap_and_charges() {
        let limits = parse_usage_limits(&fixture()).unwrap();
        assert_eq!(limits.plan_limit, 60.0);
        assert_eq!(limits.plan_used, 60.0);
        assert_eq!(limits.overage_used, 10.0);
        assert_eq!(limits.overage_cap, Some(20.0));
        assert_eq!(limits.overage_charges, Some(5.0));
        assert_eq!(limits.overage_rate, Some(0.5));
        assert_eq!(limits.currency_code, "USD");
    }

    #[test]
    fn enrichment_adds_overage_credit_and_cost_rows() {
        let limits = parse_usage_limits(&fixture()).unwrap();
        let usage = apply_usage_limits(UsageSnapshot::new(RateWindow::new(1.0)), &limits);
        assert!((usage.primary.used_percent - 100.0).abs() < 0.001);
        let credits = usage
            .extra_rate_windows
            .iter()
            .find(|row| row.id == "kiro-overage-credits")
            .unwrap();
        assert!((credits.window.used_percent - 50.0).abs() < 0.001);
        assert!(
            usage
                .extra_rate_windows
                .iter()
                .any(|row| row.id == "kiro-overage-cost" && row.window.is_informational)
        );
    }

    #[test]
    fn disabled_api_overage_removes_stale_cli_rows() {
        let data = serde_json::to_vec(&serde_json::json!({
            "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsageWithPrecision": 10.0,
                "usageLimitWithPrecision": 60.0,
                "currentOveragesWithPrecision": 0.0,
                "nextDateReset": 1798761600.0
            }],
            "overageConfiguration": {"overageStatus": "DISABLED"}
        }))
        .unwrap();
        let limits = parse_usage_limits(&data).unwrap();
        let mut usage = UsageSnapshot::new(RateWindow::new(10.0));
        usage = usage.with_extra_rate_window(
            "kiro-overage-credits",
            "Overage usage",
            RateWindow::informational("stale"),
        );
        let usage = apply_usage_limits(usage, &limits);
        assert!(
            !usage
                .extra_rate_windows
                .iter()
                .any(|row| row.id.starts_with("kiro-overage-"))
        );
    }

    #[test]
    fn bonus_usage_does_not_override_cli_plan_percentage() {
        let data = serde_json::to_vec(&serde_json::json!({
            "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsageWithPrecision": 80.0,
                "usageLimitWithPrecision": 60.0,
                "currentOveragesWithPrecision": 0.0,
                "nextDateReset": 1798761600.0,
                "bonuses": [{}]
            }]
        }))
        .unwrap();
        let limits = parse_usage_limits(&data).unwrap();
        let usage = apply_usage_limits(UsageSnapshot::new(RateWindow::new(42.0)), &limits);
        assert!((usage.primary.used_percent - 42.0).abs() < 0.001);
    }

    #[test]
    fn profile_endpoint_follows_supported_region() {
        assert_eq!(
            endpoint_for_profile_arn("arn:aws:codewhisperer:us-east-1:123:profile/test"),
            Some(US_EAST_ENDPOINT)
        );
        assert_eq!(
            endpoint_for_profile_arn("arn:aws:codewhisperer:eu-central-1:123:profile/test"),
            Some(FRANKFURT_ENDPOINT)
        );
    }

    #[test]
    fn unsupported_profile_arns_have_no_enrichment_endpoint() {
        for profile_arn in [
            "",
            "not-an-arn",
            "arn:aws-cn:codewhisperer:eu-central-1:123:profile/test",
            "arn:aws:s3:eu-central-1:123:profile/test",
            "arn:aws:codewhisperer:ap-southeast-1:123:profile/test",
            "arn:aws:codewhisperer:eu-central-1:123:profile/",
            "arn:aws:codewhisperer:eu-central-1:123:other/test",
            "arn:aws:codewhisperer:eu-central-1:123:profile/test ",
        ] {
            assert_eq!(
                endpoint_for_profile_arn(profile_arn),
                None,
                "{profile_arn:?}"
            );
        }
    }

    fn state_db_with_profile(profile_arn: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("data.sqlite3");
        let connection = rusqlite::Connection::open(&path).expect("open fixture database");
        connection
            .execute_batch("CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT);")
            .expect("create state table");
        let profile = serde_json::json!({ "arn": profile_arn }).to_string();
        connection
            .execute(
                "INSERT INTO state (key, value) VALUES (?1, ?2)",
                [PROFILE_KEY, profile.as_str()],
            )
            .expect("insert profile");
        (dir, path)
    }

    #[test]
    fn reachability_endpoint_follows_the_cli_profile_region() {
        let (_dir, path) =
            state_db_with_profile("arn:aws:codewhisperer:eu-central-1:123:profile/test");

        assert_eq!(reachability_endpoint(&path), FRANKFURT_ENDPOINT);
    }

    #[test]
    fn reachability_endpoint_defaults_to_us_east_without_usable_region_state() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            reachability_endpoint(&dir.path().join("missing.sqlite3")),
            US_EAST_ENDPOINT
        );

        let (_dir, path) =
            state_db_with_profile("arn:aws:codewhisperer:ap-southeast-1:123:profile/test");
        assert_eq!(reachability_endpoint(&path), US_EAST_ENDPOINT);
    }

    #[tokio::test]
    async fn transport_failure_is_a_transport_error_that_keeps_last_good() {
        // Bind and drop a listener to get a local port with nothing listening.
        let port = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            listener.local_addr().expect("local addr").port()
        };

        let error = ensure_reachable_at(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect_err("a closed port must fail the reachability check");

        assert!(matches!(error, ProviderError::Timeout));
        assert!(error.is_transport_failure());
    }

    #[tokio::test]
    async fn any_http_response_counts_as_reachable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 1024];
            let read = socket.read(&mut request).await.expect("read request");
            assert!(read > 0, "the client must send a request before reading");
            socket
                .write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write response");
        });

        ensure_reachable_at(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("an HTTP response proves reachability");
        server.await.expect("server task");
    }
}
