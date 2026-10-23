//! Grok provider implementation.
//!
//! Bearer logins (`~/.grok/auth.json` from `grok login`) read credits from the
//! Grok CLI credits proxy first, then fall back to the grok.com billing
//! gRPC-web endpoint. Browser cookies use the gRPC-web endpoint only.

pub mod accounts;
mod acp;
mod billing;
mod credits_proxy;
pub mod local_sessions;
mod product_usage;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderInventoryItem, RateWindow, SourceMode, UsageSnapshot,
};

use self::accounts::{GrokAuthKind, ParsedGrokAuthFile};
use self::billing::GrokBillingSnapshot;
use self::credits_proxy::{BearerBilling, CREDITS_PROXY_ENDPOINT};

const BILLING_ENDPOINT: &str = "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";
const BILLING_REQUEST_BODY: [u8; 7] = [0, 0, 0, 0, 2, 0x08, 0];
const REMAINING_RESETS_ENDPOINT: &str =
    "https://grok.com/prod_mc_billing.ConsumerUiSvc/GetRemainingResets";
const CLI_SETTINGS_ENDPOINT: &str = "https://cli-chat-proxy.grok.com/v1/settings";
const RESET_CREDITS_TIMEOUT: Duration = Duration::from_secs(2);
const RESET_CREDITS_JOIN_GRACE: Duration = Duration::from_millis(250);
const GROK_CLI_ACP_ARGS: [&str; 2] = ["agent", "stdio"];
/// Shown to the user when no usable Grok login is left after every auto step.
const GROK_SIGN_IN_HINT: &str = "Grok sign-in required. Run `grok login`, then refresh CodexBar.";

/// Reset-window pace (upstream `GrokProviderDescriptor.pace`, v0.70.0): weekly
/// windows inside their length, see [`reset_window_pace_matches`].
pub(crate) const PACE_CAPABILITY: crate::core::ProviderPaceCapability =
    crate::core::ProviderPaceCapability::reset_window(crate::core::PaceWindowRule::Custom(
        reset_window_pace_matches,
    ));

pub struct GrokProvider {
    client: Client,
    billing_endpoint: String,
    credits_proxy_endpoint: String,
    cli_billing: CliBillingRunner,
}

/// Where the ACP billing fallback gets its answer. `Installed` runs the Grok
/// CLI; tests substitute a scripted child process or a fake to pin the
/// fallback ordering.
#[derive(Clone)]
enum CliBillingRunner {
    Installed,
    #[cfg(test)]
    Process {
        program: PathBuf,
        args: Vec<String>,
        timeouts: acp::AcpTimeouts,
    },
    #[cfg(test)]
    Fake(std::sync::Arc<dyn Fn() -> Result<BearerBilling, ProviderError> + Send + Sync>),
}

impl CliBillingRunner {
    async fn fetch(&self) -> Result<BearerBilling, ProviderError> {
        match self {
            Self::Installed => {
                // The native installer puts grok under ~/.grok/bin, which is
                // often not on PATH. The login flow's resolver checks that
                // location first, so the ACP fallback uses the same answer.
                let program = accounts::login::executable()
                    .map_err(|_| ProviderError::NotInstalled(acp::CLI_NOT_FOUND.to_string()))?;
                acp::fetch_billing(&program, &GROK_CLI_ACP_ARGS, acp::AcpTimeouts::default()).await
            }
            #[cfg(test)]
            Self::Process {
                program,
                args,
                timeouts,
            } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                acp::fetch_billing(program, &args, *timeouts).await
            }
            #[cfg(test)]
            Self::Fake(run) => run(),
        }
    }
}

impl GrokProvider {
    pub fn new() -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
            billing_endpoint: BILLING_ENDPOINT.to_string(),
            credits_proxy_endpoint: CREDITS_PROXY_ENDPOINT.to_string(),
            cli_billing: CliBillingRunner::Installed,
        }
    }

    fn auth_file_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("GROK_HOME")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home).join("auth.json"));
        }
        dirs::home_dir().map(|home| home.join(".grok").join("auth.json"))
    }

    #[cfg(test)]
    fn client_for_tests(&self) -> Client {
        self.client.clone()
    }

    #[cfg(test)]
    fn with_billing_endpoint_for_tests(mut self, endpoint: String) -> Self {
        self.billing_endpoint = endpoint;
        self
    }

    #[cfg(test)]
    fn with_credits_proxy_endpoint_for_tests(mut self, endpoint: String) -> Self {
        self.credits_proxy_endpoint = endpoint;
        self
    }

    #[cfg(test)]
    fn with_acp_process_for_tests(
        mut self,
        program: PathBuf,
        args: Vec<String>,
        timeouts: acp::AcpTimeouts,
    ) -> Self {
        self.cli_billing = CliBillingRunner::Process {
            program,
            args,
            timeouts,
        };
        self
    }

    #[cfg(test)]
    fn with_cli_billing_for_tests(
        mut self,
        run: impl Fn() -> Result<BearerBilling, ProviderError> + Send + Sync + 'static,
    ) -> Self {
        self.cli_billing = CliBillingRunner::Fake(std::sync::Arc::new(run));
        self
    }

    async fn load_credentials(kind: GrokAuthKind) -> Result<GrokCredentials, ProviderError> {
        if kind == GrokAuthKind::OAuth {
            let text = accounts::AccountManager::new()
                .map_err(|_| ProviderError::AuthRequired)?
                .ambient_auth_text_for_usage()
                .await?;
            return GrokCredentials::parse_for_kind(&text, kind);
        }
        let path = Self::auth_file_path()
            .ok_or_else(|| ProviderError::NotInstalled("Grok auth path not found".to_string()))?;
        let text = std::fs::read_to_string(&path).map_err(|_| {
            ProviderError::NotInstalled("Grok auth.json not found. Run `grok login`.".to_string())
        })?;
        GrokCredentials::parse_for_kind(&text, kind)
    }

    pub async fn fetch_usage_from_auth_json(
        &self,
        text: &str,
    ) -> Result<crate::providers::grok::accounts::GrokAccountUsage, ProviderError> {
        // A saved row identifies its OAuth entry. An expired OAuth login must
        // not fall back to a different CLI identity in the same auth file.
        let parsed =
            ParsedGrokAuthFile::parse(text).map_err(|e| ProviderError::Parse(e.to_string()))?;
        let kind = if parsed
            .view()
            .map_err(|e| ProviderError::Parse(e.to_string()))?
            .select(GrokAuthKind::OAuth)
            .is_some()
        {
            GrokAuthKind::OAuth
        } else {
            GrokAuthKind::Cli
        };
        let credentials = GrokCredentials::parse_for_kind(text, kind)?;
        let result = self
            .fetch_with_auth(&credentials, kind, &saved_account_usage_context())
            .await?;
        Ok(account_usage_from_result(&result))
    }

    async fn fetch_with_auth(
        &self,
        credentials: &GrokCredentials,
        kind: GrokAuthKind,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        // Validate before starting the optional reset-credit request.
        if credentials.is_expired(Utc::now()) {
            return Err(ProviderError::AuthRequired);
        }
        let reset_lookup = GrokProvider::spawn_remaining_resets(
            ctx,
            Some(credentials.access_token.clone()),
            None,
            self.client.clone(),
        );
        let BearerBilling {
            billing,
            subscription_tier,
            prepaid_balance_usd,
        } = match self
            .fetch_bearer_or_cli_billing(credentials, kind, ctx)
            .await
        {
            Ok(bearer) => bearer,
            Err(error) => {
                reset_lookup.abort();
                return Err(error);
            }
        };
        let plan = if kind == GrokAuthKind::Cli {
            self.fetch_cli_subscription_tier(credentials).await
        } else {
            None
        }
        .or(subscription_tier)
        .or_else(|| credentials.login_method());
        let reset_credits = reset_lookup
            .join(ctx.requires_optional_usage_completeness)
            .await;
        let result = attach_prepaid_balance(
            result_from_billing(
                billing,
                if kind == GrokAuthKind::Cli {
                    "grok-cli"
                } else {
                    "grok-oauth"
                },
                credentials.email.clone(),
                credentials.team_id.clone(),
                plan,
            ),
            prepaid_balance_usd,
        );
        Ok(match reset_credits {
            Some(credits) => result.with_inventory_item(credits),
            None => result,
        })
    }

    /// Current billing for a credential. The bearer path (credits proxy, then
    /// grok.com gRPC-web) runs first. Only when it fails, for the ambient Grok
    /// CLI login, is the installed CLI asked over ACP (upstream #4595). An
    /// isolated fetch never reaches the ambient login, so it gets the bearer
    /// error unchanged.
    async fn fetch_bearer_or_cli_billing(
        &self,
        credentials: &GrokCredentials,
        kind: GrokAuthKind,
        ctx: &FetchContext,
    ) -> Result<BearerBilling, ProviderError> {
        let error = match self.fetch_bearer_billing(credentials).await {
            Ok(bearer) => return Ok(bearer),
            Err(error) => error,
        };
        if kind != GrokAuthKind::Cli || ctx.token_account_isolated {
            return Err(error);
        }
        match self.cli_billing.fetch().await {
            Ok(bearer) => {
                tracing::debug!("Grok billing answered by the CLI ACP fallback");
                Ok(bearer)
            }
            Err(acp_error) => {
                // The original error stays the answer, so auto mode still
                // moves to its next step exactly as before.
                tracing::debug!("Grok CLI ACP billing fallback failed: {acp_error}");
                Err(error)
            }
        }
    }

    /// The ambient Grok CLI login is missing or expired, so no credential can
    /// be sent. The installed CLI may still hold its own valid session, so it
    /// is asked over ACP as a fallback. Isolated fetches never reach the
    /// ambient login. When ACP also fails, the original login error is
    /// returned, not an ACP error.
    async fn fetch_cli_acp_without_login(
        &self,
        login_error: ProviderError,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        if ctx.token_account_isolated || !is_missing_or_expired_login(&login_error) {
            return Err(login_error);
        }
        match self.cli_billing.fetch().await {
            Ok(bearer) => {
                tracing::debug!("Grok billing answered by the CLI ACP fallback without a login");
                Ok(result_from_acp_billing(bearer))
            }
            Err(acp_error) => {
                tracing::debug!("Grok CLI ACP billing fallback failed: {acp_error}");
                Err(login_error)
            }
        }
    }

    async fn fetch_cli_subscription_tier(&self, credentials: &GrokCredentials) -> Option<String> {
        let response = self
            .client
            .get(CLI_SETTINGS_ENDPOINT)
            .timeout(std::time::Duration::from_secs(2))
            .header(
                "Authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("x-xai-token-auth", "xai-grok-cli")
            .header("Accept", "application/json")
            .header("User-Agent", "CodexBar")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: Value = response.json().await.ok()?;
        grok_plan_display_name(
            value
                .get("subscription_tier_display")
                .and_then(Value::as_str),
        )
    }

    async fn fetch_with_cookie(
        &self,
        cookie_header: &str,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let reset_lookup = GrokProvider::spawn_remaining_resets(
            ctx,
            None,
            Some(cookie_header.to_string()),
            self.client.clone(),
        );
        let billing = match self
            .fetch_billing(None, Some(cookie_header.to_string()))
            .await
        {
            Ok(billing) => billing,
            Err(error) => {
                reset_lookup.abort();
                return Err(error);
            }
        };
        let reset_credits = reset_lookup
            .join(ctx.requires_optional_usage_completeness)
            .await;
        // v0.56.0: a browser session is its own principal. Never enrich a
        // successful cookie billing result from ambient auth.json metadata,
        // which may belong to a different account or change during the fetch.
        let result = result_from_cookie_billing(billing);
        Ok(match reset_credits {
            Some(credits) => result.with_inventory_item(credits),
            None => result,
        })
    }

    /// A token-account row may be routed to Cli/OAuth by its usage source
    /// even though the account only carries a cookie. Ambient ~/.grok
    /// credentials belong to whichever account is switched in, so never
    /// read them here.
    async fn fetch_isolated_account(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        match grok_isolated_route(ctx) {
            GrokIsolatedRoute::Bearer(token) => {
                let credentials = GrokCredentials::from_bearer(token);
                self.fetch_with_auth(&credentials, GrokAuthKind::OAuth, ctx)
                    .await
            }
            GrokIsolatedRoute::Cookie(cookie_header) => {
                self.fetch_with_cookie(cookie_header, ctx).await
            }
            GrokIsolatedRoute::Missing => Err(isolated_account_missing_credential()),
        }
    }

    async fn fetch_with_oauth_fallback(
        &self,
        credentials: &GrokCredentials,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        match self
            .fetch_with_auth(credentials, GrokAuthKind::OAuth, ctx)
            .await
        {
            Ok(result) => Ok(result),
            Err(ProviderError::AuthRequired) => {
                if let Some(token) = ctx.api_key.as_deref() {
                    let fallback = GrokCredentials::from_bearer(token);
                    self.fetch_with_auth(&fallback, GrokAuthKind::OAuth, ctx)
                        .await
                } else {
                    Err(ProviderError::AuthRequired)
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch_auto(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        let allow_browser_cookie_fallback = !ctx.auto_prefer_web;
        for step in grok_steps_for_context(
            ctx.api_key
                .as_deref()
                .is_some_and(|token| !token.trim().is_empty()),
            ctx.manual_cookie_header
                .as_deref()
                .is_some_and(|cookie| !cookie.trim().is_empty()),
            allow_browser_cookie_fallback,
            ctx.token_account_isolated,
        ) {
            match step {
                GrokAutoStep::AmbientOAuth => {
                    if let Some(result) = self.try_ambient(GrokAuthKind::OAuth, ctx).await {
                        return result;
                    }
                }
                GrokAutoStep::AmbientCli => {
                    if let Some(result) = self.try_ambient(GrokAuthKind::Cli, ctx).await {
                        return result;
                    }
                }
                GrokAutoStep::ApiKey => {
                    if let Some(token) = ctx.api_key.as_deref() {
                        let credentials = GrokCredentials::from_bearer(token);
                        return self
                            .fetch_with_auth(&credentials, GrokAuthKind::OAuth, ctx)
                            .await;
                    }
                }
                GrokAutoStep::ManualCookie => {
                    if let Some(cookie_header) = &ctx.manual_cookie_header {
                        return self.fetch_with_cookie(cookie_header, ctx).await;
                    }
                }
                GrokAutoStep::CookieRefresh => {
                    return self.fetch_with_cookie_refresh(ctx).await;
                }
            }
        }
        Err(ProviderError::AuthRequired)
    }

    async fn try_ambient(
        &self,
        kind: GrokAuthKind,
        ctx: &FetchContext,
    ) -> Option<Result<ProviderFetchResult, ProviderError>> {
        let loaded = Self::load_credentials(kind).await;
        self.try_ambient_loaded(kind, loaded, ctx).await
    }

    /// Auto-mode step for an ambient login that has already been loaded (or
    /// failed to load). A missing or expired Cli login may still be answered by
    /// the CLI over ACP; if that fails, the step yields `None` as before.
    async fn try_ambient_loaded(
        &self,
        kind: GrokAuthKind,
        loaded: Result<GrokCredentials, ProviderError>,
        ctx: &FetchContext,
    ) -> Option<Result<ProviderFetchResult, ProviderError>> {
        let credentials = match loaded {
            Ok(credentials) => credentials,
            Err(error) if kind == GrokAuthKind::Cli => {
                return match self.fetch_cli_acp_without_login(error, ctx).await {
                    Ok(result) => Some(Ok(result)),
                    Err(_) => None,
                };
            }
            Err(_) => return None,
        };
        match self.fetch_with_auth(&credentials, kind, ctx).await {
            Ok(result) => Some(Ok(result)),
            Err(ProviderError::AuthRequired) => None,
            Err(error) => {
                tracing::debug!("Grok login path failed: {error}");
                None
            }
        }
    }

    /// Cookie refresh path (upstream #2458):
    /// 1. Try last validated cached cookie header (background reuse)
    /// 2. On miss/auth failure: re-import browser cookies, validate, cache
    async fn fetch_with_cookie_refresh(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        use crate::browser::cookie_cache::CookieHeaderCache;

        if let Some(cached) = CookieHeaderCache::load(ProviderId::Grok) {
            match self.fetch_with_cookie(&cached.cookie_header, ctx).await {
                Ok(result) => return Ok(result),
                Err(err) if is_cookie_authentication_failure(&err) => {
                    CookieHeaderCache::clear(ProviderId::Grok);
                }
                Err(err) => return Err(err),
            }
        }

        let cookie_header = crate::providers::browser_cookie_header(&["grok.com"])?;
        let result = self.fetch_with_cookie(&cookie_header, ctx).await?;
        // Best-effort cache write: failing to persist the cookie only costs a
        // re-read from the browser on the next fetch.
        let _cached = CookieHeaderCache::store(ProviderId::Grok, &cookie_header, "browser");
        Ok(result)
    }

    async fn fetch_billing(
        &self,
        authorization: Option<String>,
        cookie_header: Option<String>,
    ) -> Result<GrokBillingSnapshot, ProviderError> {
        let mut request = self
            .client
            .post(&self.billing_endpoint)
            .body(BILLING_REQUEST_BODY.to_vec())
            .header("Origin", "https://grok.com")
            .header("Referer", "https://grok.com/?_s=usage")
            .header("Accept", "*/*")
            .header("Content-Type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .header("x-user-agent", "connect-es/2.1.1")
            .header("User-Agent", "CodexBar");
        if let Some(auth) = authorization {
            request = request.header("Authorization", auth);
        }
        if let Some(cookie) = cookie_header {
            request = request.header("Cookie", cookie);
        }

        let response = request.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "Grok web billing returned status {status}"
            )));
        }
        billing::validate_grpc_headers(&headers)?;
        billing::parse_grpc_web_response(&bytes)
    }

    fn spawn_remaining_resets(
        ctx: &FetchContext,
        access_token: Option<String>,
        cookie_header: Option<String>,
        client: Client,
    ) -> ResetLookup {
        if !ctx.include_credits {
            return ResetLookup::idle();
        }
        ResetLookup {
            task: Some(tokio::spawn(async move {
                Self::fetch_remaining_resets(
                    client,
                    access_token.as_deref(),
                    cookie_header.as_deref(),
                )
                .await
            })),
        }
    }

    /// Fetch optional SuperGrok reset-credit inventory using the same principal
    /// that produced the successful billing result. This is deliberately
    /// best-effort: billing remains valid when this secondary endpoint is down,
    /// malformed, unauthorized, or empty.
    async fn fetch_remaining_resets(
        client: Client,
        access_token: Option<&str>,
        cookie_header: Option<&str>,
    ) -> Option<ProviderInventoryItem> {
        let mut request = client
            .post(REMAINING_RESETS_ENDPOINT)
            .body(vec![0, 0, 0, 0, 0])
            .timeout(RESET_CREDITS_TIMEOUT)
            .header("Origin", "https://grok.com")
            .header("Referer", "https://grok.com/?_s=usage")
            .header("Accept", "*/*")
            .header("Content-Type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .header("x-user-agent", "connect-es/2.1.1")
            .header("User-Agent", "CodexBar");
        if let Some(access_token) = access_token {
            request = request.header("Authorization", format!("Bearer {access_token}"));
        }
        if let Some(cookie_header) = cookie_header {
            request = request.header("Cookie", cookie_header);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!("Grok reset-credit lookup failed: {error}");
                return None;
            }
        };
        if !response.status().is_success() {
            tracing::debug!(status = %response.status(), "Grok reset-credit lookup returned a non-success status");
            return None;
        }
        let headers = response.headers().clone();
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!("Grok reset-credit response read failed: {error}");
                return None;
            }
        };
        if let Err(error) = billing::validate_grpc_headers(&headers) {
            tracing::debug!("Grok reset-credit RPC failed: {error}");
            return None;
        }
        let coupons = match billing::parse_grpc_web_reset_coupons(&bytes, Utc::now()) {
            Ok(coupons) => coupons,
            Err(error) => {
                tracing::debug!("Grok reset-credit response was invalid: {error}");
                return None;
            }
        };
        let next_expiry = coupons.first().map(|coupon| coupon.expires_at);
        let available_count = u32::try_from(coupons.len()).ok()?;
        (!coupons.is_empty()).then_some(ProviderInventoryItem {
            id: "reset-credits".to_string(),
            title: "Limit Reset Credits".to_string(),
            available_count,
            next_expires_at: next_expiry,
        })
    }

    fn detect_cli_version() -> Option<String> {
        let mut command = std::process::Command::new(accounts::login::executable().ok()?);
        command.arg("--version");
        hide_windows_console(&mut command);
        let output = command.output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let trimmed = text
            .lines()
            .next()?
            .trim()
            .strip_prefix("grok ")
            .unwrap_or(text.trim());
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

/// Best-effort reset-credit lookup running alongside the billing request.
/// `abort` and `join` own the handle state; `JoinHandle::abort` is already
/// idempotent on finished tasks.
struct ResetLookup {
    task: Option<tokio::task::JoinHandle<Option<ProviderInventoryItem>>>,
}

impl ResetLookup {
    fn idle() -> Self {
        Self { task: None }
    }

    fn abort(self) {
        if let Some(task) = self.task {
            task.abort();
        }
    }

    /// Wait for the lookup with the policy budget: under
    /// `requires_optional_usage_completeness` the full timeout remains;
    /// otherwise a short grace joins the already-running request.
    async fn join(
        self,
        requires_optional_usage_completeness: bool,
    ) -> Option<ProviderInventoryItem> {
        let mut task = self.task?;
        let budget = if requires_optional_usage_completeness {
            RESET_CREDITS_TIMEOUT
        } else {
            RESET_CREDITS_JOIN_GRACE
        };
        match tokio::time::timeout(budget, &mut task).await {
            Ok(Ok(inventory)) => inventory,
            Ok(Err(_)) | Err(_) => {
                task.abort();
                None
            }
        }
    }
}

#[cfg(windows)]
fn hide_windows_console(command: &mut std::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut std::process::Command) {}

impl Default for GrokProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GrokProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Grok
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        // The sign-in hint is applied to the final result only: auto steps
        // inside the block still match on `AuthRequired` to fall through.
        let result: Result<ProviderFetchResult, ProviderError> = async {
            match ctx.source_mode {
                SourceMode::Auto => self.fetch_auto(ctx).await,
                SourceMode::Web if ctx.token_account_isolated => {
                    match ctx
                        .manual_cookie_header
                        .as_deref()
                        .filter(|cookie| !cookie.trim().is_empty())
                    {
                        Some(cookie_header) => self.fetch_with_cookie(cookie_header, ctx).await,
                        None => Err(isolated_account_missing_credential()),
                    }
                }
                SourceMode::Web => {
                    if let Some(cookie_header) = &ctx.manual_cookie_header {
                        return self.fetch_with_cookie(cookie_header, ctx).await;
                    }
                    self.fetch_with_cookie_refresh(ctx).await
                }
                SourceMode::Cli | SourceMode::OAuth if ctx.token_account_isolated => {
                    self.fetch_isolated_account(ctx).await
                }
                SourceMode::Cli => match Self::load_credentials(GrokAuthKind::Cli).await {
                    Ok(credentials) => {
                        self.fetch_with_auth(&credentials, GrokAuthKind::Cli, ctx)
                            .await
                    }
                    Err(error) => self.fetch_cli_acp_without_login(error, ctx).await,
                },
                SourceMode::OAuth => {
                    // Prefer the switched ~/.grok/auth.json over a leftover token
                    // account so Weekly/notifications follow Grok account Switch.
                    let credentials = match Self::load_credentials(GrokAuthKind::OAuth).await {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            let Some(token) = ctx.api_key.as_deref() else {
                                return Err(error);
                            };
                            GrokCredentials::from_bearer(token)
                        }
                    };
                    self.fetch_with_oauth_fallback(&credentials, ctx).await
                }
            }
        }
        .await;
        result.map_err(sign_in_hint_for_terminal_error)
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![
            SourceMode::Auto,
            SourceMode::Cli,
            SourceMode::OAuth,
            SourceMode::Web,
        ]
    }

    fn detect_version(&self) -> Option<String> {
        Self::detect_cli_version()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrokAutoStep {
    AmbientOAuth,
    AmbientCli,
    ApiKey,
    ManualCookie,
    CookieRefresh,
}

/// Auto must try the switched ~/.grok/auth.json before leftover cookies or
/// token keys. Account rows already fetch per login; Weekly, pace, and
/// notifications use this provider snapshot.
fn grok_auto_steps(
    has_api_key: bool,
    has_manual_cookie: bool,
    allow_browser_cookie_fallback: bool,
) -> Vec<GrokAutoStep> {
    let mut steps = vec![GrokAutoStep::AmbientOAuth, GrokAutoStep::AmbientCli];
    if has_api_key {
        steps.push(GrokAutoStep::ApiKey);
    }
    // Explicit cookies remain available even when automatic browser discovery is disabled.
    if has_manual_cookie {
        steps.push(GrokAutoStep::ManualCookie);
    }
    if allow_browser_cookie_fallback {
        steps.push(GrokAutoStep::CookieRefresh);
    }
    steps
}

/// An isolated token-account fetch (one row per configured account) must
/// read only that account's credential: the ambient ~/.grok login and the
/// browser session belong to whichever account is switched in.
fn grok_steps_for_context(
    has_api_key: bool,
    has_manual_cookie: bool,
    allow_browser_cookie_fallback: bool,
    token_account_isolated: bool,
) -> Vec<GrokAutoStep> {
    let mut steps = grok_auto_steps(
        has_api_key,
        has_manual_cookie,
        allow_browser_cookie_fallback,
    );
    if token_account_isolated {
        steps.retain(|step| matches!(step, GrokAutoStep::ApiKey | GrokAutoStep::ManualCookie));
    }
    steps
}

/// A saved account row is one principal. Its auth text may not be the account
/// switched into `~/.grok`, so the fetch is marked isolated: it must never
/// reach the ambient CLI login, including over ACP.
fn saved_account_usage_context() -> FetchContext {
    FetchContext {
        token_account_isolated: true,
        ..FetchContext::default()
    }
}

fn isolated_account_missing_credential() -> ProviderError {
    ProviderError::Other(
        "This Grok account has no saved token or cookie. Add one in Settings.".to_string(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrokIsolatedRoute<'a> {
    Bearer(&'a str),
    Cookie(&'a str),
    Missing,
}

fn grok_isolated_route(ctx: &FetchContext) -> GrokIsolatedRoute<'_> {
    fn non_empty(value: &Option<String>) -> Option<&str> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
    if let Some(token) = non_empty(&ctx.api_key) {
        return GrokIsolatedRoute::Bearer(token);
    }
    if let Some(cookie_header) = non_empty(&ctx.manual_cookie_header) {
        return GrokIsolatedRoute::Cookie(cookie_header);
    }
    GrokIsolatedRoute::Missing
}

#[derive(Debug, Clone)]
struct GrokCredentials {
    access_token: String,
    auth_mode: Option<String>,
    email: Option<String>,
    team_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

impl GrokCredentials {
    fn from_bearer(token: &str) -> Self {
        Self {
            access_token: token.trim().to_string(),
            auth_mode: Some("oidc".into()),
            email: None,
            team_id: None,
            expires_at: None,
        }
    }

    fn parse_for_kind(text: &str, kind: GrokAuthKind) -> Result<Self, ProviderError> {
        let parsed = ParsedGrokAuthFile::parse(text)
            .map_err(|error| ProviderError::Parse(error.to_string()))?;
        let entry = parsed
            .view()
            .map_err(|error| ProviderError::Parse(error.to_string()))?
            .select(kind)
            .ok_or(ProviderError::AuthRequired)?;
        let access_token = entry.key().ok_or(ProviderError::AuthRequired)?.to_owned();
        let expires_at = entry
            .value()
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
            .map(|dt| dt.with_timezone(&Utc));
        if expires_at.is_some_and(|dt| dt <= Utc::now()) {
            return Err(ProviderError::AuthRequired);
        }
        Ok(Self {
            access_token,
            auth_mode: text_field(entry.value(), "auth_mode"),
            email: text_field(entry.value(), "email"),
            team_id: text_field(entry.value(), "team_id"),
            expires_at,
        })
    }

    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    fn login_method(&self) -> Option<String> {
        match self.auth_mode.as_deref().map(str::to_lowercase).as_deref() {
            Some("oidc") => Some("SuperGrok".to_string()),
            Some("session") => Some("session".to_string()),
            Some(other) => Some(other.to_string()),
            None if self.expires_at.is_some() => Some("Grok".to_string()),
            None => None,
        }
    }
}

fn grok_plan_display_name(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let compact: String = trimmed
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect();
    Some(match compact.as_str() {
        "supergrokheavy" | "heavy" => "SuperGrok Heavy".to_string(),
        "supergrok" => "SuperGrok".to_string(),
        _ => trimmed.to_string(),
    })
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Classify Grok from the full billing-cycle duration, not time remaining.
/// This preserves the upstream #2431/#2566 invariant that a monthly plan near
/// its reset must not become a weekly plan.
fn primary_label_for_cycle_minutes(minutes: u32) -> Option<&'static str> {
    const DAY_MINUTES: u32 = 24 * 60;
    if minutes <= 60 {
        return None;
    }
    let days = (minutes + DAY_MINUTES / 2) / DAY_MINUTES;
    if (4..=12).contains(&days) {
        Some("Weekly")
    } else if (20..=45).contains(&days) {
        Some("Monthly")
    } else {
        None
    }
}

/// Upstream `GrokProviderDescriptor.primaryLabel(window:now:)`: the label of
/// the window's duration, or of the distance to its reset when the duration
/// is unknown.
fn primary_label_for_window(window: &RateWindow, now: DateTime<Utc>) -> Option<&'static str> {
    if let Some(minutes) = window.window_minutes {
        return primary_label_for_cycle_minutes(minutes);
    }
    let resets_at = window.resets_at?;
    let seconds = (resets_at - now).num_milliseconds() as f64 / 1_000.0;
    if seconds <= 3_600.0 {
        return None;
    }
    let days = (seconds / 86_400.0).round();
    if (4.0..=12.0).contains(&days) {
        Some("Weekly")
    } else if (20.0..=45.0).contains(&days) {
        Some("Monthly")
    } else {
        None
    }
}

/// Upstream Grok `ProviderPaceCapability.resetWindowPace`: a window labelled
/// "Weekly" whose reset falls inside its length (7 days when unknown).
pub(crate) fn reset_window_pace_matches(window: &RateWindow, now: DateTime<Utc>) -> bool {
    if primary_label_for_window(window, now) != Some("Weekly") {
        return false;
    }
    let Some(resets_at) = window.resets_at else {
        return false;
    };
    let window_minutes = window.window_minutes.unwrap_or(7 * 24 * 60);
    let until_reset = resets_at - now;
    window_minutes > 0
        && until_reset > chrono::Duration::zero()
        && until_reset <= chrono::Duration::minutes(i64::from(window_minutes))
}

/// Period label for the purchased-credit cost. It ends in "balance", so the
/// shared card treats the cost as balance-only.
const PREPAID_BALANCE_PERIOD: &str = "Prepaid balance";

/// Attach the purchased-credit wallet as a balance-only cost (upstream
/// `GrokUsageSnapshot.toUsageSnapshot`). Quota meters are left untouched. No
/// `limit` is set: a zero limit would read as a full meter, and the balance-only
/// settings view keys on an absent limit.
fn attach_prepaid_balance(
    result: ProviderFetchResult,
    balance_usd: Option<f64>,
) -> ProviderFetchResult {
    match balance_usd {
        Some(balance) => result
            .with_cost(CostSnapshot::new(0.0, "USD", PREPAID_BALANCE_PERIOD).with_balance(balance)),
        None => result,
    }
}

fn result_from_cookie_billing(billing: GrokBillingSnapshot) -> ProviderFetchResult {
    result_from_billing(billing, "grok-browser", None, None, None)
}

fn account_usage_from_result(
    result: &ProviderFetchResult,
) -> crate::providers::grok::accounts::GrokAccountUsage {
    let primary = &result.usage.primary;
    let usage_available = !primary.is_informational && primary.used_percent.is_finite();
    crate::providers::grok::accounts::GrokAccountUsage {
        status: if usage_available {
            accounts::GrokUsageStatus::Ready
        } else {
            accounts::GrokUsageStatus::Unavailable
        },
        usage_available,
        used_percent: usage_available.then_some(primary.used_percent),
        plan: result
            .usage
            .login_method
            .clone()
            .or(result.usage.primary_label.clone()),
        window_minutes: primary.window_minutes,
        resets_at: primary.resets_at,
    }
}

fn result_from_billing(
    billing: GrokBillingSnapshot,
    source_label: &str,
    email: Option<String>,
    team_id: Option<String>,
    login_method: Option<String>,
) -> ProviderFetchResult {
    // Dynamic cadence is provider-owned and comes only from a complete billing
    // cycle. A reset timestamp alone is insufficient because monthly quotas can
    // have only a few days remaining.
    let primary_label = billing
        .window_minutes
        .and_then(primary_label_for_cycle_minutes);
    let published_percent = billing.used_percent.filter(|_| {
        billing.used_percent_is_wire_published || billing.used_percent_is_implicit_zero
    });
    let primary = match published_percent {
        Some(used_percent) => RateWindow::with_details(
            used_percent,
            billing.window_minutes,
            billing.resets_at,
            None,
        ),
        None => {
            let mut window = RateWindow::informational("Usage unavailable");
            window.resets_at = billing.resets_at;
            window.window_minutes = billing.window_minutes;
            window
        }
    };
    let mut usage = UsageSnapshot::new(primary);
    if let Some(label) = primary_label {
        usage = usage.with_primary_label(label);
    }
    usage.account_email = email;
    usage.account_organization = team_id;
    usage.login_method = login_method;
    // Reset-credit enrichment adds inventory only, so the breakdown survives it.
    product_usage::display_details(&billing.product_usage)
        .into_iter()
        .fold(
            ProviderFetchResult::new(usage, source_label),
            |result, detail| result.with_display_detail(Some(detail)),
        )
}

/// Result for a CLI login-less ACP answer. The CLI answered from its own
/// login, so no account identity is known and no reset-credit lookup runs
/// (that lookup needs the token CodexBar does not have).
fn result_from_acp_billing(bearer: BearerBilling) -> ProviderFetchResult {
    let BearerBilling {
        billing,
        subscription_tier,
        prepaid_balance_usd,
    } = bearer;
    attach_prepaid_balance(
        result_from_billing(billing, "grok-cli", None, None, subscription_tier),
        prepaid_balance_usd,
    )
}

/// A Cli login that is absent (no auth file or no Cli entry) or expired.
/// Parse errors are a malformed file, not a missing login, so they do not
/// qualify for the ACP fallback.
fn is_missing_or_expired_login(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::AuthRequired | ProviderError::NotInstalled(_)
    )
}

/// Maps a sign-in failure that reaches the caller to an error that names the
/// fix. `OAuth` keeps the NeedsAuthentication state and the auth error
/// category (the shape Claude uses for a missing login), unlike `NotInstalled`.
fn sign_in_hint_for_terminal_error(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::AuthRequired => ProviderError::OAuth(GROK_SIGN_IN_HINT.to_string()),
        other => other,
    }
}

/// Whether a cookie-path error should invalidate the cached browser session.
fn is_cookie_authentication_failure(err: &ProviderError) -> bool {
    matches!(err, ProviderError::AuthRequired)
}

/// Decide the next cookie-refresh step given cache presence and last error.
/// Pure helper for unit tests of the #2458 refresh flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CookieRefreshAction {
    UseCached,
    ReimportBrowser,
    GiveUp,
}

fn cookie_refresh_action(
    has_cached_header: bool,
    last_error: Option<&ProviderError>,
) -> CookieRefreshAction {
    match last_error {
        None if has_cached_header => CookieRefreshAction::UseCached,
        None => CookieRefreshAction::ReimportBrowser,
        Some(err) if is_cookie_authentication_failure(err) => CookieRefreshAction::ReimportBrowser,
        Some(ProviderError::NoCookies) => CookieRefreshAction::ReimportBrowser,
        Some(_) => CookieRefreshAction::GiveUp,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "credits_proxy_tests.rs"]
mod credits_proxy_tests;

#[cfg(test)]
#[path = "product_usage_tests.rs"]
mod product_usage_tests;

#[cfg(test)]
#[path = "cli_fallback_tests.rs"]
mod cli_fallback_tests;
