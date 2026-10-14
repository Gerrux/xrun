#![deny(unsafe_code)]

//! HTTPS client for Kaggle endpoints xrun calls without the `kaggle` CLI.
//!
//! Two surfaces, both with Basic (`username:api_key`) or Bearer auth:
//!
//!   * `https://www.kaggle.com/api/v1/datasets/...` — dataset view/list,
//!     plain REST paths.
//!   * `https://api.kaggle.com/v1/<service>/<Method>` — the RPC form
//!     `kagglesdk` uses for everything (`KaggleHttpClient._get_request_url`),
//!     always `POST` with a camelCase JSON body. The kernel status lives
//!     here; the old `www.kaggle.com/api/v1/kernels/status` path answers
//!     404 with an HTML page.
//!
//! A running kernel cannot be cancelled from here. `CancelKernelSession`
//! takes a `kernelSessionId`, and no public response carries it: Kaggle
//! closed kaggle-api PRs #1170/#1173/#1174 (Aug 2026) saying the session id
//! "is not currently exposed outside the server". `xrun stop` therefore only
//! asks for the status and tells the user to stop the kernel in the web UI.

use std::time::Duration;

use reqwest::blocking::Client;

use crate::cli::{KernelState, KernelStatus};
use crate::error::KaggleError;
use crate::snapshot::RemoteFile;

const DEFAULT_API_BASE: &str = "https://www.kaggle.com/api/v1";
const DEFAULT_RPC_BASE: &str = "https://api.kaggle.com/v1";

/// How many 429 responses `dataset_files` absorbs (1 s, 2 s, 4 s, 8 s, 16 s
/// unless `Retry-After` says otherwise) before giving up on the listing.
const DATASET_LIST_429_RETRIES: u32 = 5;

#[derive(Clone, Debug)]
pub enum Auth {
    /// Legacy `kaggle.json` style — username + key.
    Basic { username: String, api_key: String },
    /// New token-based auth (`KAGGLE_API_TOKEN`). Sent as Bearer.
    Bearer(String),
}

/// What `KaggleApiClient::wait_dataset_version_above` saw before giving up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionWait {
    /// The number rose above the previous one.
    Reached(u32),
    /// The API kept answering with this number, not above the previous one.
    Stuck(u32),
    /// The API never returned a version number.
    Unknown,
}

#[derive(Clone)]
pub struct KaggleApiClient {
    client: Client,
    base: String,
    rpc_base: String,
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
            rpc_base: DEFAULT_RPC_BASE.to_string(),
            auth,
        })
    }

    /// Point both the REST and the RPC surface at one mock server (test-only;
    /// their paths don't overlap).
    #[cfg(any(test, feature = "mock"))]
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self.rpc_base = self.base.clone();
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

    /// State of the kernel's latest session via the `GetKernelSessionStatus`
    /// RPC — the call behind `kaggle kernels status`.
    ///
    /// Exactly one request: a 429 comes back as an `Err` carrying the code,
    /// not a retry. `xrun stop` and the poller's early-fail path call this
    /// once per stop, and looping on a throttled account only digs deeper.
    pub fn kernel_session_status(&self, kernel_slug: &str) -> Result<KernelStatus, KaggleError> {
        let (owner, slug) = kernel_slug.split_once('/').ok_or_else(|| {
            KaggleError::ParseError(format!(
                "expected kernel slug in <owner>/<name> form, got: {kernel_slug}"
            ))
        })?;
        let url = format!(
            "{}/kernels.KernelsApiService/GetKernelSessionStatus",
            self.rpc_base
        );
        let body = serde_json::json!({
            "userName": owner,
            "kernelSlug": slug,
        });
        let resp = self
            .with_auth(self.client.post(&url))
            .json(&body)
            .send()
            .map_err(|e| KaggleError::NotFound(format!("GetKernelSessionStatus request: {e}")))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if !status.is_success() {
            return Err(KaggleError::CliFailure {
                exit_code: status.as_u16() as i32,
                stderr: format!(
                    "GetKernelSessionStatus {kernel_slug} → {status}: {}",
                    error_excerpt(&text)
                ),
            });
        }
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            KaggleError::ParseError(format!(
                "GetKernelSessionStatus {kernel_slug}: not JSON: {e}"
            ))
        })?;
        // kagglesdk's EnumSerializer writes the bare name ("RUNNING"); the
        // ordinal is accepted too in case the server ever sends it.
        let raw = match json.get("status") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => match n.as_u64() {
                Some(0) => "QUEUED".into(),
                Some(1) => "RUNNING".into(),
                Some(2) => "COMPLETE".into(),
                Some(3) => "ERROR".into(),
                Some(4) => "CANCEL_REQUESTED".into(),
                Some(5) => "CANCEL_ACKNOWLEDGED".into(),
                Some(6) => "NEW_SCRIPT".into(),
                _ => n.to_string(),
            },
            // Proto3 JSON omits a field at its default, and the default of
            // KernelWorkerStatus is QUEUED.
            None => "QUEUED".into(),
            Some(other) => other.to_string(),
        };
        let raw = raw.trim_start_matches("KernelWorkerStatus.").to_string();
        let state = match raw.as_str() {
            "QUEUED" => KernelState::Queued,
            "RUNNING" => KernelState::Running,
            "COMPLETE" => KernelState::Complete,
            // Same fold as the CLI parser: a cancel is a terminal failure.
            "ERROR" | "CANCEL_REQUESTED" | "CANCEL_ACKNOWLEDGED" => KernelState::Error,
            _ => KernelState::Unknown,
        };
        let failure_message = json
            .get("failureMessage")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(KernelStatus {
            error_message: if state == KernelState::Error {
                failure_message.clone()
            } else {
                None
            },
            status: state,
            run_seconds: None,
            raw_state: Some(format!("KernelWorkerStatus.{raw}")),
            failure_message,
        })
    }

    /// Resolve the current version number for a dataset.
    ///
    /// Kaggle's `datasets status` CLI tells us when the latest blob is
    /// indexed, but the *kernel-creation* API resolves "latest" via a separate
    /// cache that lags several minutes (Issue 1 in field-issues log). To stop
    /// kernels pinning to the previous version, we fetch the current version
    /// number here and pass it explicitly to kernel-metadata as
    /// `<owner>/<slug>/<N>`.
    ///
    /// Returns Ok(None) when the endpoint shape doesn't expose a version
    /// (private dataset, schema drift) — caller falls back to unpinned.
    pub fn dataset_current_version(&self, slug: &str) -> Result<Option<u32>, KaggleError> {
        let (owner, name) = slug.split_once('/').ok_or_else(|| {
            KaggleError::ParseError(format!(
                "expected dataset slug in <owner>/<name> form, got: {slug}"
            ))
        })?;
        let url = format!("{}/datasets/view/{}/{}", self.base, owner, name);
        let resp = self
            .with_auth(self.client.get(&url))
            .send()
            .map_err(|e| KaggleError::NotFound(format!("datasets/view request: {e}")))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let text = resp.text().unwrap_or_default();
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let n = json
            .get("currentVersionNumber")
            .or_else(|| json.get("current_version_number"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        Ok(n)
    }

    /// List every file Kaggle holds for the current version of a dataset.
    ///
    /// Walks `GET /datasets/list/<owner>/<name>` through `nextPageToken`
    /// until the server stops paging. Used after `xrun dataset push` to
    /// confirm the upload actually landed: the CLI exits 0 and `datasets
    /// status` says `ready` even when the version came out empty or holds
    /// unextracted archives, so only the remote file list tells the truth.
    ///
    /// Unlike `dataset_current_version`, an HTTP error here is an `Err`:
    /// the caller must distinguish "could not verify" from "verified empty".
    pub fn dataset_files(&self, slug: &str) -> Result<Vec<RemoteFile>, KaggleError> {
        self.dataset_files_at(slug, None)
    }

    /// Poll `dataset_current_version` until it rises above `previous` (any
    /// number counts when `previous` is `None`, i.e. a new slug).
    ///
    /// `datasets status` answers `ready` for the version Kaggle already
    /// serves, so on an existing slug it says nothing about whether the push
    /// produced a new one; the version number does. An error or a missing
    /// number from the API is retried like "not there yet".
    pub fn wait_dataset_version_above(
        &self,
        slug: &str,
        previous: Option<u32>,
        timeout: Duration,
        poll_interval: Duration,
    ) -> VersionWait {
        let started = std::time::Instant::now();
        let mut last_seen: Option<u32> = None;
        loop {
            match self.dataset_current_version(slug) {
                Ok(Some(n)) if previous.is_none_or(|p| n > p) => return VersionWait::Reached(n),
                Ok(Some(n)) => last_seen = Some(n),
                Ok(None) => {}
                Err(e) => tracing::info!("dataset '{slug}' version not available yet: {e}"),
            }
            if started.elapsed() >= timeout {
                return match last_seen {
                    Some(n) => VersionWait::Stuck(n),
                    None => VersionWait::Unknown,
                };
            }
            std::thread::sleep(poll_interval);
        }
    }

    /// `dataset_files` for a given version (`None` — the current one).
    /// Pinning the number avoids reading the old version's list while the
    /// listing for the new one is still catching up.
    pub fn dataset_files_at(
        &self,
        slug: &str,
        version: Option<u32>,
    ) -> Result<Vec<RemoteFile>, KaggleError> {
        let (owner, name) = slug.split_once('/').ok_or_else(|| {
            KaggleError::ParseError(format!(
                "expected dataset slug in <owner>/<name> form, got: {slug}"
            ))
        })?;
        let url = format!("{}/datasets/list/{}/{}", self.base, owner, name);
        let mut files = Vec::new();
        let mut page_token: Option<String> = None;
        let mut throttled = 0u32;
        // Hard stop so a server that keeps returning the same token can't
        // spin us forever. 200 files/page × 1000 pages is far beyond any
        // dataset xrun stages.
        for _ in 0..1000 {
            // Kaggle's default page is 20 files; a 13 865-file cache then
            // takes ~700 requests and trips the 429 limiter about 20 s in.
            // 200 is the documented maximum.
            let mut req = self.client.get(&url).query(&[("pageSize", "200")]);
            if let Some(v) = version {
                req = req.query(&[("datasetVersionNumber", v.to_string())]);
            }
            if let Some(token) = &page_token {
                req = req.query(&[("pageToken", token.as_str())]);
            }
            let resp = self
                .with_auth(req)
                .send()
                .map_err(|e| KaggleError::NotFound(format!("datasets/list request: {e}")))?;
            let status = resp.status();
            if status.as_u16() == 429 && throttled < DATASET_LIST_429_RETRIES {
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1u64 << throttled);
                throttled += 1;
                std::thread::sleep(Duration::from_secs(wait.min(30)));
                continue;
            }
            let text = resp.text().unwrap_or_default();
            if !status.is_success() {
                return Err(KaggleError::CliFailure {
                    exit_code: status.as_u16() as i32,
                    stderr: format!("datasets/list {slug}: HTTP {status}: {text}"),
                });
            }
            let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                KaggleError::ParseError(format!("datasets/list {slug}: not JSON: {e}"))
            })?;
            let page = json
                .get("datasetFiles")
                .or_else(|| json.get("dataset_files"))
                .or_else(|| json.get("files"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for f in &page {
                let Some(name) = f.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let total_bytes = f
                    .get("totalBytes")
                    .or_else(|| f.get("total_bytes"))
                    .and_then(|v| {
                        v.as_u64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    });
                files.push(RemoteFile {
                    name: name.trim_start_matches('/').to_string(),
                    total_bytes,
                });
            }
            let next = json
                .get("nextPageToken")
                .or_else(|| json.get("next_page_token"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            match next {
                Some(t) if Some(&t) != page_token.as_ref() => page_token = Some(t),
                _ => return Ok(files),
            }
        }
        Ok(files)
    }
}

/// A short, single-line form of an error body for warnings: the RPC error's
/// `message` when it is JSON, a size note for an HTML page (Kaggle answers an
/// unknown path with its whole site), otherwise the first 200 characters.
fn error_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with('<') {
        return format!("<HTML page, {} bytes>", trimmed.len());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let msg = v
            .pointer("/error/message")
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str());
        if let Some(m) = msg {
            return m.to_string();
        }
    }
    let line = trimmed.lines().next().unwrap_or("");
    line.chars().take(200).collect()
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
    use wiremock::matchers::{body_json, method, path};
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
    async fn dataset_files_follows_page_tokens_and_normalises_entries() {
        let mock = server().await;
        // Page 1: no token → two files, one with totalBytes as a string and a
        // leading slash, plus a nextPageToken.
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .and(wiremock::matchers::query_param_is_missing("pageToken"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "datasetFiles": [
                    {"name": "train/a.npz", "totalBytes": 10},
                    {"name": "/val/b.npz", "totalBytes": "20"}
                ],
                "nextPageToken": "p2"
            })))
            .mount(&mock)
            .await;
        // Page 2: one more file, empty token → stop.
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .and(wiremock::matchers::query_param("pageToken", "p2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "datasetFiles": [{"name": "val/c.npz"}],
                "nextPageToken": ""
            })))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let files = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .dataset_files("alice/cache")
                .unwrap()
        });
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["train/a.npz", "val/b.npz", "val/c.npz"]);
        assert_eq!(files[0].total_bytes, Some(10));
        assert_eq!(files[1].total_bytes, Some(20));
        assert_eq!(files[2].total_bytes, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dataset_files_requests_max_page_size_and_retries_429() {
        let mock = server().await;
        // First call is throttled; the retry (same request) succeeds.
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .and(wiremock::matchers::query_param("pageSize", "200"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "0")
                    .set_body_string(r#"{"code":429,"message":"TooManyRequests"}"#),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .and(wiremock::matchers::query_param("pageSize", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "datasetFiles": [{"name": "a.npz", "totalBytes": 1}]
            })))
            .expect(1)
            .mount(&mock)
            .await;

        let url = mock.uri();
        let files = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .dataset_files("alice/cache")
                .unwrap()
        });
        assert_eq!(files.len(), 1);
        mock.verify().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dataset_files_at_pins_the_version() {
        let mock = server().await;
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .and(wiremock::matchers::query_param("datasetVersionNumber", "4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "datasetFiles": [{"name": "a.npz", "totalBytes": 1}]
            })))
            .expect(1)
            .mount(&mock)
            .await;

        let url = mock.uri();
        let files = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .dataset_files_at("alice/cache", Some(4))
                .unwrap()
        });
        assert_eq!(files.len(), 1);
        mock.verify().await;
    }

    async fn mount_version(mock: &MockServer, n: u32, times: Option<u64>) {
        let m = Mock::given(method("GET"))
            .and(path("/datasets/view/alice/cache"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "currentVersionNumber": n })),
            );
        match times {
            Some(t) => m.up_to_n_times(t).mount(mock).await,
            None => m.mount(mock).await,
        }
    }

    fn wait_version(url: String, previous: Option<u32>) -> VersionWait {
        off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .wait_dataset_version_above(
                    "alice/cache",
                    previous,
                    Duration::from_millis(300),
                    Duration::from_millis(10),
                )
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_dataset_version_reports_stuck_when_number_does_not_grow() {
        // The ArboForge incident: the push never reached Kaggle, the number
        // stays where it was before the push.
        let mock = server().await;
        mount_version(&mock, 3, None).await;
        assert_eq!(wait_version(mock.uri(), Some(3)), VersionWait::Stuck(3));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_dataset_version_returns_the_new_number() {
        let mock = server().await;
        mount_version(&mock, 3, Some(2)).await;
        mount_version(&mock, 4, None).await;
        assert_eq!(wait_version(mock.uri(), Some(3)), VersionWait::Reached(4));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_dataset_version_new_slug_accepts_first_number() {
        let mock = server().await;
        mount_version(&mock, 1, None).await;
        assert_eq!(wait_version(mock.uri(), None), VersionWait::Reached(1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_dataset_version_unknown_when_api_gives_no_number() {
        let mock = server().await;
        Mock::given(method("GET"))
            .and(path("/datasets/view/alice/cache"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock)
            .await;
        assert_eq!(wait_version(mock.uri(), Some(3)), VersionWait::Unknown);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dataset_files_http_error_is_err_not_empty() {
        // 403 must surface as "could not verify", never as "zero files".
        let mock = server().await;
        Mock::given(method("GET"))
            .and(path("/datasets/list/alice/cache"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&mock)
            .await;

        let url = mock.uri();
        let res = off_runtime(move || {
            KaggleApiClient::new(basic_auth())
                .unwrap()
                .with_base_url(url)
                .dataset_files("alice/cache")
        });
        let err = res.expect_err("403 must be an error");
        assert!(err.to_string().contains("403"), "{err}");
    }

    const STATUS_RPC: &str = "/kernels.KernelsApiService/GetKernelSessionStatus";

    fn session_status(url: String, auth: Auth) -> Result<KernelStatus, KaggleError> {
        off_runtime(move || {
            KaggleApiClient::new(auth)
                .unwrap()
                .with_base_url(url)
                .kernel_session_status("alice/my-kernel")
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_status_posts_the_rpc_and_reads_running() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path(STATUS_RPC))
            .and(body_json(json!({
                "userName": "alice",
                "kernelSlug": "my-kernel",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "RUNNING"})))
            .expect(1)
            .mount(&mock)
            .await;

        let st = session_status(mock.uri(), basic_auth()).unwrap();
        assert_eq!(st.status, KernelState::Running);
        assert_eq!(st.raw_state.as_deref(), Some("KernelWorkerStatus.RUNNING"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_status_maps_terminal_states() {
        for (body, state) in [
            (json!({"status": "COMPLETE"}), KernelState::Complete),
            (
                json!({"status": "ERROR", "failureMessage": "OOM"}),
                KernelState::Error,
            ),
            (json!({"status": "CANCEL_ACKNOWLEDGED"}), KernelState::Error),
            // Proto3 JSON drops the enum default, QUEUED.
            (json!({}), KernelState::Queued),
            (json!({"status": 1}), KernelState::Running),
        ] {
            let mock = server().await;
            Mock::given(method("POST"))
                .and(path(STATUS_RPC))
                .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
                .mount(&mock)
                .await;
            let st = session_status(mock.uri(), basic_auth()).unwrap();
            assert_eq!(st.status, state, "{body}");
            if state == KernelState::Error && body.get("failureMessage").is_some() {
                assert_eq!(st.error_message.as_deref(), Some("OOM"));
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_status_html_404_is_a_short_error() {
        let mock = server().await;
        let page = format!("<!DOCTYPE html><html>{}</html>", "x".repeat(5000));
        Mock::given(method("POST"))
            .and(path(STATUS_RPC))
            .respond_with(ResponseTemplate::new(404).set_body_string(page))
            .mount(&mock)
            .await;

        let err = session_status(mock.uri(), basic_auth())
            .unwrap_err()
            .to_string();
        assert!(err.contains("404"), "{err}");
        assert!(err.contains("<HTML page"), "{err}");
        assert!(err.len() < 300, "whole page leaked into the error: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_status_surfaces_the_rpc_error_message() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path(STATUS_RPC))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "error": {"code": 403, "message": "Permission 'kernels.get' was denied"}
            })))
            .mount(&mock)
            .await;

        let err = session_status(mock.uri(), basic_auth())
            .unwrap_err()
            .to_string();
        assert!(err.contains("403"), "{err}");
        assert!(err.contains("kernels.get"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kernel_session_status_does_not_retry_429() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path(STATUS_RPC))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
            .expect(1)
            .mount(&mock)
            .await;

        let err = session_status(mock.uri(), basic_auth()).unwrap_err();
        assert!(err.to_string().contains("429"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bearer_auth_uses_authorization_header() {
        let mock = server().await;
        Mock::given(method("POST"))
            .and(path(STATUS_RPC))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer my-token",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "COMPLETE"})))
            .mount(&mock)
            .await;

        let st = session_status(mock.uri(), Auth::Bearer("my-token".to_string())).unwrap();
        assert_eq!(st.status, KernelState::Complete);
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
