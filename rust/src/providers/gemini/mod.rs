//! Gemini provider implementation
//!
//! The default source reads Gemini CLI quotas from Google's Cloud Code API
//! using the OAuth credentials the Gemini CLI stores in
//! ~/.gemini/oauth_creds.json. The explicit `web` source reads the Gemini app
//! 5-hour and weekly limits with the browser account (`web.rs`); Auto never
//! falls back to it.

mod api;
mod web;

use async_trait::async_trait;

use crate::browser::cookies::CookieRequestScope;
use crate::core::{
    FetchContext, ManualEmptyCookiePolicy, Provider, ProviderError, ProviderFetchResult,
    ProviderId, SourceMode,
};

pub use api::GeminiApi;
pub use web::GeminiWeb;

/// Gemini provider for fetching AI usage limits
pub struct GeminiProvider {
    api: GeminiApi,
    web: GeminiWeb,
}

impl GeminiProvider {
    pub fn new() -> Self {
        Self {
            api: GeminiApi::new(),
            web: GeminiWeb::new(),
        }
    }
}

impl Default for GeminiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        if ctx.source_mode == SourceMode::Web {
            tracing::debug!("Fetching Gemini app usage via browser session");
            let usage = self.web.fetch(ctx).await?;
            return Ok(ProviderFetchResult::new(usage, "web"));
        }

        tracing::debug!("Fetching Gemini usage via API");

        match self.api.fetch_quota(ctx).await {
            Ok(usage) => Ok(ProviderFetchResult::new(usage, "cli")),
            Err(e) => {
                tracing::warn!("Gemini API fetch failed: {}", e);
                Err(e)
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Cli, SourceMode::Web]
    }

    /// Gemini app limits belong to a browser account; a cookie must never
    /// turn the CLI source into the app source.
    fn web_is_opt_in(&self) -> bool {
        true
    }

    fn explicit_web_survives_cookie_off(&self) -> bool {
        true
    }

    fn manual_empty_cookie_policy(&self) -> ManualEmptyCookiePolicy {
        ManualEmptyCookiePolicy::FailClosedWeb
    }

    /// The shell would import only the first browser's cookies; the app
    /// source tries each browser so a signed-out session can move on.
    fn owns_browser_cookie_resolution(&self) -> bool {
        true
    }

    fn cookie_request_scope(&self) -> Option<CookieRequestScope> {
        Some(web::COOKIE_SCOPE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_is_an_explicit_source_next_to_the_cli() {
        let provider = GeminiProvider::new();
        assert_eq!(
            provider.available_sources(),
            vec![SourceMode::Auto, SourceMode::Cli, SourceMode::Web]
        );
        assert!(provider.web_is_opt_in());
        assert!(provider.explicit_web_survives_cookie_off());
        assert_eq!(
            provider.manual_empty_cookie_policy(),
            ManualEmptyCookiePolicy::FailClosedWeb
        );
        assert!(provider.owns_browser_cookie_resolution());
        assert_eq!(provider.cookie_request_scope(), Some(web::COOKIE_SCOPE));
    }

    #[tokio::test]
    async fn web_source_with_cookies_unavailable_never_reads_the_cli_credential() {
        let ctx = FetchContext {
            source_mode: SourceMode::Web,
            manual_cookie_missing: true,
            ..FetchContext::default()
        };
        let error = GeminiProvider::new()
            .fetch_usage(&ctx)
            .await
            .expect_err("no session is available");
        assert!(
            matches!(
                &error,
                ProviderError::BrowserSignInRequired { message, .. }
                    if message == web::COOKIES_UNAVAILABLE
            ),
            "got {error:?}"
        );
    }
}
