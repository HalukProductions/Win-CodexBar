use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use base64::Engine;
use serde_json::{Value, json};

use super::*;
use crate::core::ProviderFetchResult;
use crate::providers::antigravity::{AntigravityProvider, AntigravityStrategyId};

const ACCESS_TOKEN: &str = "synthetic-access-token";
const REFRESHED_TOKEN: &str = "synthetic-refreshed-token";
const REFRESH_TOKEN: &str = "synthetic-refresh-token";
// Split literals on purpose: secret scanners match the full Google OAuth
// client id/secret shape in source text, while the compiled values keep that
// shape so the artifact-discovery parser tests stay meaningful.
const CLIENT_SECRET: &str = concat!("GOCSPX-", "synthetic-test-secret-000000");
const CLIENT_ID: &str = concat!("123456789012-synthetic-test", ".apps.googleusercontent.com");

struct RecordedRequest {
    method: String,
    url: String,
    bearer: Option<String>,
    user_agent: Option<String>,
    json: Option<Value>,
    form: Vec<(&'static str, String)>,
}

type QueuedResponses = HashMap<String, VecDeque<(u16, Vec<u8>)>>;

/// Answers by endpoint name (`loadCodeAssist`, `token`, ...). Each endpoint
/// pops queued responses and repeats the last one; unknown endpoints get 404.
#[derive(Default)]
struct FakeTransport {
    responses: Mutex<QueuedResponses>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl FakeTransport {
    fn respond(self, method: &str, status: u16, body: Value) -> Self {
        self.responses
            .lock()
            .unwrap()
            .entry(method.to_string())
            .or_default()
            .push_back((status, body.to_string().into_bytes()));
        self
    }

    fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.method.clone())
            .collect()
    }

    fn request(&self, method: &str) -> Option<RecordedRequest> {
        let requests = self.requests.lock().unwrap();
        requests
            .iter()
            .find(|request| request.method == method)
            .map(|request| RecordedRequest {
                method: request.method.clone(),
                url: request.url.clone(),
                bearer: request.bearer.clone(),
                user_agent: request.user_agent.clone(),
                json: request.json.clone(),
                form: request.form.clone(),
            })
    }
}

fn endpoint(url: &str) -> String {
    let tail = url.rsplit('/').next().unwrap_or(url);
    tail.strip_prefix("v1internal:").unwrap_or(tail).to_string()
}

#[async_trait]
impl RemoteTransport for FakeTransport {
    async fn post(&self, request: RemoteRequest) -> Result<RemoteResponse, ProviderError> {
        let method = endpoint(&request.url);
        let (json, form) = match request.body {
            RequestBody::Json(body) => (Some(body), Vec::new()),
            RequestBody::Form(fields) => (None, fields),
        };
        self.requests.lock().unwrap().push(RecordedRequest {
            method: method.clone(),
            url: request.url.clone(),
            bearer: request.bearer,
            user_agent: request.user_agent,
            json,
            form,
        });
        let mut responses = self.responses.lock().unwrap();
        let Some(queue) = responses.get_mut(&method) else {
            return Ok(RemoteResponse {
                status: 404,
                body: Vec::new(),
            });
        };
        let (status, body) = if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            queue.front().cloned().unwrap()
        };
        Ok(RemoteResponse { status, body })
    }
}

fn fetcher(transport: FakeTransport, home: &Path) -> RemoteFetcher<FakeTransport> {
    RemoteFetcher {
        transport,
        home: home.to_path_buf(),
        injected_credentials: None,
        configured_client: None,
        client_artifacts: Vec::new(),
        timeout: Duration::from_secs(1),
        onboard_poll_interval: Duration::ZERO,
    }
}

fn injected(transport: FakeTransport, credentials: Value) -> RemoteFetcher<FakeTransport> {
    let mut fetcher = fetcher(transport, Path::new("Z:\\no-such-home"));
    fetcher.injected_credentials = Some(credentials.to_string());
    fetcher
}

fn id_token(claims: Value) -> String {
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
    format!("header.{payload}.signature")
}

fn valid_credentials() -> Value {
    json!({
        "access_token": ACCESS_TOKEN,
        "refresh_token": REFRESH_TOKEN,
        "expiry_date": far_future_ms(),
        "id_token": id_token(json!({ "email": "user@example.com" })),
    })
}

fn far_future_ms() -> f64 {
    (chrono::Utc::now().timestamp_millis() + 3_600_000) as f64
}

fn summary(buckets: Value) -> Value {
    json!({ "groups": [{ "displayName": "Gemini", "buckets": buckets }] })
}

fn session_and_weekly_summary() -> Value {
    summary(json!([
        { "bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.6 },
        { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.8 },
    ]))
}

fn weekly_only_summary() -> Value {
    summary(json!([
        { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.8 },
    ]))
}

fn free_tier() -> Value {
    json!({ "currentTier": { "id": "free-tier" }, "cloudaicompanionProject": "proj-1" })
}

fn consumer_client_rejection() -> Value {
    json!({
        "allowedTiers": [{ "id": "free-tier", "isDefault": true }],
        "ineligibleTiers": [{ "reasonCode": "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT" }],
    })
}

async fn plan_for(code_assist: Value, quota_summary: Value) -> Option<String> {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, code_assist)
        .respond("retrieveUserQuotaSummary", 200, quota_summary);
    injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap_or_else(|error| panic!("fetch failed: {error:?}"))
        .login_method
}

// ── Plan labels (#4571) ─────────────────────────────────────────────

#[tokio::test]
async fn paid_tier_name_wins_over_free_current_tier() {
    let code_assist = json!({
        "currentTier": { "id": "free-tier", "name": "Antigravity" },
        "paidTier": { "id": "g1-pro-tier", "name": "Google AI Pro" },
        "cloudaicompanionProject": "proj-1",
    });
    assert_eq!(
        plan_for(code_assist, weekly_only_summary())
            .await
            .as_deref(),
        Some("Google AI Pro")
    );
}

#[tokio::test]
async fn paid_tier_without_name_is_labelled_paid() {
    let code_assist = json!({
        "currentTier": { "id": "free-tier" },
        "paidTier": { "id": "g1-pro-tier" },
        "cloudaicompanionProject": "proj-1",
    });
    assert_eq!(
        plan_for(code_assist, weekly_only_summary())
            .await
            .as_deref(),
        Some("Paid")
    );
}

#[tokio::test]
async fn measured_five_hour_quota_suppresses_free_label() {
    assert_eq!(
        plan_for(free_tier(), session_and_weekly_summary()).await,
        None
    );
}

#[tokio::test]
async fn weekly_only_quota_keeps_free_label() {
    assert_eq!(
        plan_for(free_tier(), weekly_only_summary())
            .await
            .as_deref(),
        Some("Free")
    );
}

#[tokio::test]
async fn disabled_or_unmeasured_five_hour_quota_keeps_free_label() {
    let disabled = summary(json!([
        { "bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.6, "disabled": true },
        { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.8 },
    ]));
    let unmeasured = summary(json!([
        { "bucketId": "gemini-5h", "window": "5h" },
        { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.8 },
    ]));
    assert_eq!(
        plan_for(free_tier(), disabled).await.as_deref(),
        Some("Free")
    );
    assert_eq!(
        plan_for(free_tier(), unmeasured).await.as_deref(),
        Some("Free")
    );
}

#[tokio::test]
async fn hosted_domain_free_tier_is_workspace() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let mut credentials = valid_credentials();
    credentials["id_token"] = json!(id_token(
        json!({ "email": "user@example.com", "hd": "example.com" })
    ));
    let usage = injected(transport, credentials).fetch().await.unwrap();
    assert_eq!(usage.login_method.as_deref(), Some("Workspace"));
    assert_eq!(usage.account_email.as_deref(), Some("user@example.com"));
}

// ── Consumer OAuth client rejection (837d30dff, 1c70e8966) ──────────

#[tokio::test]
async fn consumer_client_rejection_requires_reauthentication_before_quota_calls() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, consumer_client_rejection())
        .respond(
            "retrieveUserQuotaSummary",
            200,
            session_and_weekly_summary(),
        );
    let fetcher = injected(transport, valid_credentials());
    let error = fetcher.fetch().await.unwrap_err();
    assert!(matches!(error, RemoteError::ReauthenticationRequired));
    assert_eq!(fetcher.transport.methods(), vec!["loadCodeAssist"]);
    let message = error.into_provider_error().to_string();
    assert!(message.contains("Sign in to it again"), "{message}");
}

#[tokio::test]
async fn stored_project_recovers_from_consumer_client_rejection() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, consumer_client_rejection())
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let mut credentials = valid_credentials();
    credentials["project_id"] = json!("stored-project");
    let fetcher = injected(transport, credentials);
    fetcher.fetch().await.unwrap();
    let summary = fetcher
        .transport
        .request("retrieveUserQuotaSummary")
        .unwrap();
    assert_eq!(summary.json, Some(json!({ "project": "stored-project" })));
}

#[tokio::test]
async fn returned_project_or_current_tier_is_not_a_client_rejection() {
    let mut returned_project = consumer_client_rejection();
    returned_project["cloudaicompanionProject"] = json!({ "id": "returned-project" });
    let mut current_tier = consumer_client_rejection();
    current_tier["currentTier"] = json!({ "id": "free-tier" });

    for code_assist in [returned_project, current_tier] {
        let transport = FakeTransport::default()
            .respond("loadCodeAssist", 200, code_assist)
            .respond(
                "onboardUser",
                200,
                json!({ "response": { "cloudaicompanionProject": "onboarded" } }),
            )
            .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
        injected(transport, valid_credentials())
            .fetch()
            .await
            .unwrap_or_else(|error| panic!("fetch failed: {error:?}"));
    }
}

#[tokio::test]
async fn other_ineligible_reason_onboards_the_default_tier() {
    let code_assist = json!({
        "allowedTiers": [
            { "id": "legacy-tier" },
            { "id": "free-tier", "isDefault": true },
        ],
        "ineligibleTiers": [{ "reasonCode": "SOMETHING_ELSE" }],
    });
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, code_assist)
        .respond(
            "onboardUser",
            200,
            json!({ "response": { "cloudaicompanionProject": { "id": "onboarded" } } }),
        )
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let fetcher = injected(transport, valid_credentials());
    fetcher.fetch().await.unwrap();
    let onboard = fetcher.transport.request("onboardUser").unwrap();
    assert_eq!(onboard.json.unwrap()["tierId"], json!("free-tier"));
    let summary = fetcher
        .transport
        .request("retrieveUserQuotaSummary")
        .unwrap();
    assert_eq!(summary.json, Some(json!({ "project": "onboarded" })));
}

// ── Request identity and errors ─────────────────────────────────────

#[tokio::test]
async fn quota_requests_carry_bearer_and_hub_user_agent() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let fetcher = injected(transport, valid_credentials());
    fetcher.fetch().await.unwrap();
    let request = fetcher.transport.request("loadCodeAssist").unwrap();
    assert_eq!(request.bearer.as_deref(), Some(ACCESS_TOKEN));
    let user_agent = request.user_agent.unwrap();
    assert!(
        user_agent.starts_with("antigravity/hub/2.9.1 windows/"),
        "{user_agent}"
    );
    assert_eq!(
        request.json.unwrap()["metadata"]["ideType"],
        json!("ANTIGRAVITY")
    );
}

#[tokio::test]
async fn quota_summary_reads_the_daily_host_and_other_calls_stay_on_stable_host() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let fetcher = injected(transport, valid_credentials());
    fetcher.fetch().await.unwrap();
    let summary = fetcher
        .transport
        .request("retrieveUserQuotaSummary")
        .unwrap();
    assert_eq!(
        summary.url,
        "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary"
    );
    let code_assist = fetcher.transport.request("loadCodeAssist").unwrap();
    assert_eq!(
        code_assist.url,
        "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"
    );
}

#[tokio::test]
async fn summary_fallback_models_request_stays_on_stable_host() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("fetchAvailableModels", 200, model(0.5));
    let fetcher = injected(transport, valid_credentials());
    fetcher.fetch().await.unwrap();
    let summary = fetcher
        .transport
        .request("retrieveUserQuotaSummary")
        .unwrap();
    assert_eq!(
        summary.url,
        "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary"
    );
    let models = fetcher.transport.request("fetchAvailableModels").unwrap();
    assert_eq!(
        models.url,
        "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels"
    );
}

#[tokio::test]
async fn unauthorized_response_is_session_expired_without_body() {
    let transport = FakeTransport::default().respond(
        "loadCodeAssist",
        401,
        json!({ "error": { "message": "leaked user@example.com proj-secret" } }),
    );
    let error = injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap_err();
    assert!(matches!(error, RemoteError::SessionExpired));
    let error = error.into_provider_error();
    assert!(matches!(error, ProviderError::OAuthExpired(_)));
    let message = error.to_string();
    assert!(!message.contains("proj-secret"), "{message}");
}

#[tokio::test]
async fn api_error_message_omits_response_body_and_token() {
    let transport = FakeTransport::default().respond(
        "loadCodeAssist",
        500,
        json!({ "error": "internal proj-secret" }),
    );
    let error = injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap_err();
    let debug = format!("{error:?}");
    let message = error.into_provider_error().to_string();
    for text in [&debug, &message] {
        assert!(!text.contains("proj-secret"), "{text}");
        assert!(!text.contains(ACCESS_TOKEN), "{text}");
    }
    assert!(message.contains("HTTP 500"), "{message}");
}

#[test]
fn decode_failure_is_a_fixed_category_without_response_text() {
    let error = decode::<FetchAvailableModelsResponse>(br#"{"models": "proj-secret-leak"}"#)
        .err()
        .expect("decode must fail");
    let debug = format!("{error:?}");
    let message = error.into_provider_error().to_string();
    for text in [&debug, &message] {
        assert!(text.contains("unexpected response shape"), "{text}");
        assert!(!text.contains("proj-secret-leak"), "{text}");
    }
}

#[tokio::test]
async fn missing_credentials_is_not_logged_in() {
    let home = tempfile::tempdir().unwrap();
    let fetcher = fetcher(FakeTransport::default(), home.path());
    assert!(!fetcher.has_credentials());
    let error = fetcher.fetch().await.unwrap_err();
    assert!(matches!(error, RemoteError::NotLoggedIn));
    assert!(fetcher.transport.methods().is_empty());
}

// ── Token refresh ───────────────────────────────────────────────────

fn write_plain_credentials(home: &Path, credentials: &Value) -> PathBuf {
    let path = credentials_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, credentials.to_string()).unwrap();
    path
}

#[tokio::test]
async fn expired_token_refreshes_with_stored_client_and_persists() {
    let home = tempfile::tempdir().unwrap();
    let path = write_plain_credentials(
        home.path(),
        &json!({
            "accessToken": ACCESS_TOKEN,
            "refreshToken": REFRESH_TOKEN,
            "expiresAt": 0.0,
            "clientId": CLIENT_ID,
            "clientSecret": CLIENT_SECRET,
            "projectId": "proj-1",
        }),
    );
    let transport = FakeTransport::default()
        .respond(
            "token",
            200,
            json!({ "access_token": REFRESHED_TOKEN, "expires_in": 3600 }),
        )
        .respond("loadCodeAssist", 200, free_tier())
        .respond("retrieveUserQuotaSummary", 200, weekly_only_summary());
    let fetcher = fetcher(transport, home.path());
    assert!(fetcher.has_credentials());
    fetcher.fetch().await.unwrap();

    assert_eq!(
        fetcher.transport.methods()[..2],
        ["token".to_string(), "loadCodeAssist".to_string()]
    );
    let refresh = fetcher.transport.request("token").unwrap();
    assert!(refresh.bearer.is_none());
    assert!(refresh.form.contains(&("client_id", CLIENT_ID.to_string())));
    assert!(
        refresh
            .form
            .contains(&("grant_type", "refresh_token".to_string()))
    );
    let load = fetcher.transport.request("loadCodeAssist").unwrap();
    assert_eq!(load.bearer.as_deref(), Some(REFRESHED_TOKEN));

    let saved: OAuthCredentials =
        serde_json::from_str(&crate::secure_file::read_string(&path).unwrap()).unwrap();
    assert_eq!(saved.access_token.as_deref(), Some(REFRESHED_TOKEN));
    assert_eq!(saved.refresh_token.as_deref(), Some(REFRESH_TOKEN));
    assert_eq!(saved.client_id.as_deref(), Some(CLIENT_ID));
    assert!(saved.expiry_date.unwrap() > chrono::Utc::now().timestamp_millis() as f64);
}

#[tokio::test]
async fn failed_refresh_is_session_expired() {
    for status in [400_u16, 401] {
        let mut credentials = valid_credentials();
        credentials["expiry_date"] = json!(0.0);
        let transport =
            FakeTransport::default().respond("token", status, json!({ "error": "invalid_grant" }));
        let mut fetcher = injected(transport, credentials);
        fetcher.configured_client = Some(OAuthClient {
            client_id: CLIENT_ID.to_string(),
            client_secret: CLIENT_SECRET.to_string(),
        });
        let error = fetcher.fetch().await.unwrap_err();
        assert!(
            matches!(error, RemoteError::SessionExpired),
            "HTTP {status}"
        );
        assert_eq!(fetcher.transport.methods(), vec!["token"]);
    }
}

#[tokio::test]
async fn transient_refresh_failure_is_retryable_not_session_expired() {
    for status in [429_u16, 500] {
        let mut credentials = valid_credentials();
        credentials["expiry_date"] = json!(0.0);
        let transport = FakeTransport::default().respond(
            "token",
            status,
            json!({ "error": "backend proj-secret" }),
        );
        let mut fetcher = injected(transport, credentials);
        fetcher.configured_client = Some(OAuthClient {
            client_id: CLIENT_ID.to_string(),
            client_secret: CLIENT_SECRET.to_string(),
        });
        let error = fetcher.fetch().await.unwrap_err();
        assert!(
            matches!(error, RemoteError::TransientApi(found) if found == status),
            "HTTP {status}: {error:?}"
        );
        assert_eq!(fetcher.transport.methods(), vec!["token"]);
        let provider_error = error.into_provider_error();
        assert!(
            matches!(provider_error, ProviderError::OAuthTransient(_)),
            "HTTP {status}: {provider_error:?}"
        );
        let message = provider_error.to_string();
        assert!(message.contains(&format!("HTTP {status}")), "{message}");
        assert!(!message.contains("proj-secret"), "{message}");
    }
}

#[tokio::test]
async fn expired_token_without_any_client_reports_missing_client() {
    let mut credentials = valid_credentials();
    credentials["expiry_date"] = json!(0.0);
    let fetcher = injected(FakeTransport::default(), credentials);
    let error = fetcher.fetch().await.unwrap_err();
    assert!(matches!(error, RemoteError::MissingOAuthClient));
    assert!(fetcher.transport.methods().is_empty());
}

// ── Model quota fallbacks ───────────────────────────────────────────

fn model(fraction: f64) -> Value {
    json!({
        "models": {
            "claude-sonnet": {
                "displayName": "Claude Sonnet",
                "quotaInfo": { "remainingFraction": fraction },
            },
        },
    })
}

#[tokio::test]
async fn unavailable_summary_falls_back_to_available_models() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("fetchAvailableModels", 200, model(0.5));
    let fetcher = injected(transport, valid_credentials());
    let usage = fetcher.fetch().await.unwrap();
    assert!((usage.primary.used_percent - 50.0).abs() < 0.01);
    assert_eq!(usage.login_method.as_deref(), Some("Free"));
    assert!(fetcher.transport.request("retrieveUserQuota").is_none());
}

#[tokio::test]
async fn forbidden_models_fall_back_to_user_quota_buckets() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("fetchAvailableModels", 403, json!({}))
        .respond(
            "retrieveUserQuota",
            200,
            json!({ "buckets": [
                { "modelId": "claude-sonnet", "remainingFraction": 0.75 },
                { "modelId": "claude-sonnet", "remainingFraction": 0.25 },
            ] }),
        );
    let usage = injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap();
    assert!((usage.primary.used_percent - 75.0).abs() < 0.01);
}

#[tokio::test]
async fn full_model_quotas_need_quota_endpoint_confirmation() {
    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("fetchAvailableModels", 200, model(1.0))
        .respond("retrieveUserQuota", 403, json!({}));
    let usage = injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap();
    assert!(usage.primary.is_informational);
    assert_eq!(
        usage.primary.reset_description.as_deref(),
        Some(LIMITS_NOT_AVAILABLE)
    );

    let transport = FakeTransport::default()
        .respond("loadCodeAssist", 200, free_tier())
        .respond("fetchAvailableModels", 200, model(1.0))
        .respond(
            "retrieveUserQuota",
            200,
            json!({ "buckets": [{ "modelId": "CLAUDE-SONNET", "remainingFraction": 0.9 }] }),
        );
    let usage = injected(transport, valid_credentials())
        .fetch()
        .await
        .unwrap();
    assert!((usage.primary.used_percent - 10.0).abs() < 0.01);
}

// ── Client discovery ────────────────────────────────────────────────

#[test]
fn artifact_with_one_client_yields_it() {
    let text = format!(
        "x vs/platform/cloudCode/common/oauthClient.js const a=\"{CLIENT_ID}\",b=\"{CLIENT_SECRET}\";"
    );
    let client = parse_client_from_artifact(text.as_bytes()).unwrap();
    assert_eq!(client.client_id, CLIENT_ID);
    assert_eq!(client.client_secret, CLIENT_SECRET);

    let mut binary = vec![0xff_u8, 0x00];
    binary.extend_from_slice(CLIENT_ID.as_bytes());
    binary.push(0);
    binary.extend_from_slice(CLIENT_SECRET.as_bytes());
    binary.push(0);
    assert!(parse_client_from_artifact(&binary).is_some());
}

#[test]
fn artifact_with_several_clients_is_ambiguous() {
    let other_id = concat!(
        "999999999999-other-synthetic",
        ".apps.googleusercontent.com"
    );
    let other_secret = concat!("GOCSPX-", "other-synthetic-secret-00000");
    let text = format!("{CLIENT_ID} {CLIENT_SECRET} {other_id} {other_secret}");
    assert!(parse_client_from_artifact(text.as_bytes()).is_none());
    assert!(parse_client_from_artifact(b"no client here").is_none());
}

// ── Auto ordering and offline detail ────────────────────────────────

fn credentialed(transport: FakeTransport) -> (tempfile::TempDir, RemoteFetcher<FakeTransport>) {
    let home = tempfile::tempdir().unwrap();
    write_plain_credentials(home.path(), &valid_credentials());
    let fetcher = fetcher(transport, home.path());
    (home, fetcher)
}

fn offline() -> Option<ProviderFetchResult> {
    Some(super::super::tests::offline_result())
}

#[tokio::test]
async fn auto_uses_oauth_after_live_runtime_fails() {
    let (_home, fetcher) = credentialed(
        FakeTransport::default()
            .respond("loadCodeAssist", 200, free_tier())
            .respond("retrieveUserQuotaSummary", 200, weekly_only_summary()),
    );
    let result = AntigravityProvider::resolve_with_remote_fallback(
        Err(LiveFailure::not_running()),
        Some(&fetcher),
        offline(),
    )
    .await
    .unwrap();
    assert_eq!(result.source_label, AntigravityStrategyId::OAuth.as_str());
}

#[tokio::test]
async fn auto_skips_oauth_without_credentials_or_after_live_success() {
    let home = tempfile::tempdir().unwrap();
    let fetcher = fetcher(FakeTransport::default(), home.path());
    let result = AntigravityProvider::resolve_with_remote_fallback(
        Err(LiveFailure::not_running()),
        Some(&fetcher),
        offline(),
    )
    .await
    .unwrap();
    assert_eq!(result.source_label, AntigravityStrategyId::Offline.as_str());
    assert!(fetcher.transport.methods().is_empty());

    let (_home, fetcher) = credentialed(FakeTransport::default());
    let live = offline().unwrap();
    let result = AntigravityProvider::resolve_with_remote_fallback(Ok(live), Some(&fetcher), None)
        .await
        .unwrap();
    assert_eq!(result.source_label, AntigravityStrategyId::Offline.as_str());
    assert!(fetcher.transport.methods().is_empty());
}

#[tokio::test]
async fn blank_injected_credentials_do_not_count_as_credentials() {
    let home = tempfile::tempdir().unwrap();
    let mut fetcher = fetcher(FakeTransport::default(), home.path());
    fetcher.injected_credentials = Some("   ".to_string());
    assert!(!fetcher.has_credentials());
    let result = AntigravityProvider::resolve_with_remote_fallback(
        Err(LiveFailure::not_running()),
        Some(&fetcher),
        offline(),
    )
    .await
    .unwrap();
    assert_eq!(result.source_label, AntigravityStrategyId::Offline.as_str());
    assert!(fetcher.transport.methods().is_empty());
}

#[tokio::test]
async fn auto_oauth_reauthentication_shows_sign_in_hint_next_to_offline_data() {
    let (_home, fetcher) = credentialed(FakeTransport::default().respond(
        "loadCodeAssist",
        200,
        consumer_client_rejection(),
    ));
    let result = AntigravityProvider::resolve_with_remote_fallback(
        Err(LiveFailure::not_running()),
        Some(&fetcher),
        offline(),
    )
    .await
    .unwrap();
    assert_eq!(result.source_label, AntigravityStrategyId::Offline.as_str());
    let detail = format!("{:?}", result.display_details());
    assert!(
        detail.contains(REAUTHENTICATION_REQUIRED_MESSAGE),
        "{detail}"
    );
}

#[tokio::test]
async fn auto_oauth_failure_without_offline_history_surfaces_oauth_error() {
    let (_home, fetcher) =
        credentialed(FakeTransport::default().respond("loadCodeAssist", 401, json!({})));
    let error = AntigravityProvider::resolve_with_remote_fallback(
        Err(LiveFailure::not_running()),
        Some(&fetcher),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ProviderError::OAuthExpired(_)));
}
