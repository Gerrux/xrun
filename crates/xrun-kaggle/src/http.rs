#![deny(unsafe_code)]

//! HTTPS client for Kaggle endpoints the upstream `kaggle` CLI doesn't expose.
//!
//! Today's only consumer is `xrun stop` — Kaggle CLI 1.8.x dropped the
//! `kaggle kernels cancel` subcommand and Kaggle's own PR #967 to add it back
//! was closed un-merged, so we have to call the REST endpoint directly.
//!
//! Endpoints (Basic auth with `username:api_key`):
//!
//!   POST /api/v1/kernels/status
//!     body: {"userName": "<owner>", "kernelSlug": "<slug>"}
//!     → `kernelSessionId` (int) when the kernel has an active session
//!
//!   POST /api/v1/kernels/cancel-session/{kernel_session_id}
//!     body: {}
//!     → 200 + `{"errorMessage": null}` on success
//!
//! These aren't in the public OpenAPI surface — the source of truth is the
//! `kagglesdk` package on GitHub. If the schema drifts, the wiremock tests
//! lock the request shape we send so the failure mode stays loud.

use std::time::Duration;

use reqwest::blocking::Client;

use crate::error::KaggleError;

const DEFAULT_API_BASE: &str = "https://www.kaggle.com/api/v1";

#[derive(Clone, Debug)]
pub enum Auth {
    /// Legacy `kaggle.json` style — username + key.
    Basic { username: String, api_key: String },
    /// New token-based auth (`KAGGLE_API_TOKEN`). Sent as Bearer.
    Bearer(String),
}

#[derive(Clone)]
pub struct KaggleApiClient {
    client: Client,
    base: String,
    auth: Auth,
}

impl KaggleApiClient {
    /// Build a client. Returns Err only if the underlying TLS stack fails to
    /// initialise — credentials are not validated until the first request.
    pub fn new(auth: Auth) -> Result<Self, KaggleError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("xrun/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| KaggleError::NotFound(format!("reqwest init: {e}")))?;
        Ok(Self {
            client,
            base: DEFAULT_API_BASE.to_string(),
            auth,
        })
    }

    /// Override the API base URL (test-only).
    #[cfg(any(test, feature = "mock"))]
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Apply the configured auth to a request builder.
    fn with_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match &self.auth {
            Auth::Basic { username, api_key } => req.basic_auth(username, Some(api_key)),
            Auth::Bearer(token) => req.bearer_auth(token),
        }
    }

    /// Resolve `<owner>/<slug>` to the active kernel session id.
    /// Returns `Ok(None)` when the kernel exists but has no active session
    /// (already finished, never started, …) — the caller treats that as a
    /// successful no-op cancel.
    pub fn kernel_session_id(&self, kernel_slug: &str) -> Result<Option<i64>, KaggleError> {
        let (owner, slug) = kernel_slug.split_once('/').ok_or_else(|| {
            KaggleError::ParseError(format!(
                "expected kernel slug in <owner>/<name> form, got: {kernel_slug}"
            ))
        })?;
        let url = format!("{}/kernels/status", self.base);
        let body = serde_json::json!({
            "userName": owner,
            "kernelSlug": slug,
        });
        let resp = self
            .with_auth(self.client.post(&url))
            .json(&body)
            .send()
            .map_err(|e| KaggleError::NotFound(format!("kernels/status request: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| KaggleError::ParseError(format!("read response body: {e}")))?;
        if !status.is_success() {
            return Err(KaggleError::CliFailure {
                exit_code: status.as_u16() as i32,
                stderr: format!("POST /kernels/status → {status}: {text}"),
            });
        }
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            KaggleError::ParseError(format!("kernels/status JSON parse: {e}\nbody: {text}"))
        })?;

        // The endpoint also exposes camelCase variants — try both.
        let session_id = json
            .get("kernelSessionId")
            .or_else(|| json.get("kernel_session_id"))
            .and_then(|v| v.as_i64())
            .filter(|id| *id > 0);
        Ok(session_id)
    }

    /// Cancel an active kernel session by id. Idempotent — a 404 / already-
    /// cancelled response is folded into Ok(()).
    pub fn cancel_session(&self, kernel_session_id: i64) -> Result<(), KaggleError> {
        let url = format!("{}/kernels/cancel-session/{}", self.base, kernel_session_id);
        let resp = self
            .with_auth(self.client.post(&url))
            .json(&serde_json::json!({}))
            .send()
            .map_err(|e| KaggleError::NotFound(format!("cancel-session request: {e}")))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();

        if status.is_success() {
            // Body may contain {"errorMessage": "..."} even on 200 if the
            // session was already terminal. Treat any non-empty errorMessage
            // as informational, not fatal — the kernel is no longer running
            // either way.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(msg) = v.get("errorMessage").and_then(|m| m.as_str()) {
                    if !msg.is_empty() {
                        tracing::info!(
                            "kaggle cancel-session {kernel_session_id}: {msg} (treating as success)"
                        );
                    }
                }
            }
            return Ok(());
        }
        if status.as_u16() == 404 {
            // Session expired / never existed — caller asked to cancel a run
            // that's already gone. Idempotent no-op.
            return Ok(());
        }
        Err(KaggleError::CliFailure {
            exit_code: status.as_u16() as i32,
            stderr: format!("POST /kernels/cancel-session/{kernel_session_id} → {status}: {text}"),
        })
    }

    /// Resolve session id for `<owner>/<slug>` and cancel it. Returns Ok even
    /// when no active session exists (treated as already-stopped).
    pub fn cancel_kernel(&self, kernel_slug: &str) -> Result<CancelOutcome, KaggleError> {
        match self.kernel_session_id(kernel_slug)? {
            Some(id) => {
                self.cancel_session(id)?;
                Ok(CancelOutcome::Cancelled(id))
            }
            None => Ok(CancelOutcome::NoActiveSession),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled(i64),
    NoActiveSession,
}

/// Convert credentials stored in xrun config into the auth variant the API
/// client expects. Returns `None` when no creds are configured.
pub fn auth_from_credentials(
    creds: &xrun_core::config::credentials::KaggleCredentials,
) -> Option<Auth> {
    if let (Some(user), Some(key)) = (&creds.username, &creds.key) {
        return Some(Auth::Basic {
            username: user.clone(),
            api_key: key.clone(),
        });
    }
    if let Some(token) = &creds.token {
        return Some(Auth::Bearer(token.clone()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn basic_auth() -> Auth {
        Auth::Basic {
            username: "alice".into(),
            api_key: "secret".into(),
        }
    }

    async fn server() -> MockServer {
        MockServer::start().await
    }

    /// Run a closure on a fresh OS thread that's free of tokio runtime
    /// context, so the inner reqwest::blocking::Client can drop its private
    /// runtime without tripping tokio's "drop a runtime from within an
    /// asynchronous context" panic.
    fn off_runtime<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
        std::thread::spawn(f).join().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_id_returns_some_when_active() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .and(body_json(json!({
                "userName": "alice",
                "kernelSlug": "my-kernel",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "kernelSessionId": 12345,
                "status": "running",
            })))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let id = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .kernel_session_id("alice/my-kernel")
                .unwrap()
        });
        assert_eq!(id, Some(12345));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_id_returns_none_when_no_active_session() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "kernelSessionId": null,
                "status": "complete",
            })))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let id = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .kernel_session_id("alice/my-kernel")
                .unwrap()
        });
        assert_eq!(id, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_id_propagates_auth_errors() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let result = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .kernel_session_id("alice/my-kernel")
        });
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("401"), "got: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_session_succeeds_on_200() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/kernels/cancel-session/\d+"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let res = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .cancel_session(12345)
        });
        assert!(res.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_session_treats_404_as_success() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/kernels/cancel-session/\d+"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let res = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .cancel_session(99)
        });
        assert!(res.is_ok(), "404 should be idempotent no-op");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_session_propagates_500() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/kernels/cancel-session/\d+"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let res = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .cancel_session(1)
        });
        let err = res.unwrap_err();
        assert!(format!("{err}").contains("500"), "got: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_kernel_resolves_then_cancels() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"kernelSessionId": 4242})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("POST"))
            .and(path("/kernels/cancel-session/4242"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let outcome = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .cancel_kernel("alice/x")
                .unwrap()
        });
        assert_eq!(outcome, CancelOutcome::Cancelled(4242));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_kernel_no_active_session_is_noop() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let outcome = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .cancel_kernel("alice/x")
                .unwrap()
        });
        assert_eq!(outcome, CancelOutcome::NoActiveSession);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bearer_auth_uses_authorization_header() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path("/kernels/status"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer my-token",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let id = off_runtime(move || {
            KaggleApiClient::new(Auth::Bearer("my-token".to_string()))
                .unwrap()
                .with_base_url(url)
                .kernel_session_id("alice/x")
                .unwrap()
        });
        assert_eq!(id, None);
    }

    #[test]
    fn auth_from_credentials_prefers_basic_when_complete() {
        use xrun_core::config::credentials::KaggleCredentials;
        let creds = KaggleCredentials {
            token: Some("ignored".into()),
            username: Some("alice".into()),
            key: Some("k".into()),
        };
        let auth = auth_from_credentials(&creds).unwrap();
        assert!(matches!(auth, Auth::Basic { .. }));
    }

    #[test]
    fn auth_from_credentials_falls_back_to_bearer() {
        use xrun_core::config::credentials::KaggleCredentials;
        let creds = KaggleCredentials {
            token: Some("t".into()),
            username: None,
            key: None,
        };
        let auth = auth_from_credentials(&creds).unwrap();
        assert!(matches!(auth, Auth::Bearer(_)));
    }

    #[test]
    fn auth_from_credentials_returns_none_when_empty() {
        use xrun_core::config::credentials::KaggleCredentials;
        let creds = KaggleCredentials::default();
        assert!(auth_from_credentials(&creds).is_none());
    }
}
