#![deny(unsafe_code)]

//! Delivery channels. Each one is a thin, synchronous `send()` over an
//! async HTTP call (or an OS toast). Construction validates credentials up
//! front so a typo in `credentials.toml` surfaces at `xrun notify test`
//! time, not at 3am when the run fails.

use xrun_core::Credentials;

use crate::{block, Notification, NotifyError, Priority, HTTP_TIMEOUT};

pub trait Channel: Send + Sync {
    fn name(&self) -> &'static str;
    fn send(&self, n: &Notification) -> Result<(), NotifyError>;
}

/// Names accepted in `[notify].channels`.
pub const KNOWN: &[&str] = &["ntfy", "telegram", "webhook", "desktop"];

/// Resolve a channel name to a configured channel, or explain why not.
pub fn build(name: &str, creds: &Credentials) -> Result<Box<dyn Channel>, NotifyError> {
    match name {
        "ntfy" => Ok(Box::new(NtfyChannel::from_creds(creds)?)),
        "telegram" => Ok(Box::new(TelegramChannel::from_creds(creds)?)),
        "webhook" => Ok(Box::new(WebhookChannel::from_creds(creds)?)),
        "desktop" => Ok(Box::new(DesktopChannel::new()?)),
        other => Err(NotifyError::Other(format!(
            "unknown notify channel `{other}` (known: {})",
            KNOWN.join(", ")
        ))),
    }
}

fn http_client() -> Result<reqwest::Client, NotifyError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("xrun/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| NotifyError::Http(e.to_string()))
}

fn require<'a>(
    channel: &'static str,
    field: &str,
    v: Option<&'a String>,
) -> Result<&'a str, NotifyError> {
    match v.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => Ok(s),
        None => Err(NotifyError::Misconfigured {
            channel,
            reason: format!("`{field}` not set (xrun config set {field} ...)"),
        }),
    }
}

async fn check_status(resp: reqwest::Response) -> Result<(), NotifyError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    let snippet: String = text.chars().take(200).collect();
    Err(NotifyError::Http(format!("HTTP {status}: {snippet}")))
}

// ---------------------------------------------------------------------------
// ntfy
// ---------------------------------------------------------------------------

/// <https://docs.ntfy.sh/publish/> — `POST {url}/{topic}` with the body as
/// the message and `Title` / `Priority` / `Tags` headers.
pub struct NtfyChannel {
    endpoint: String,
    token: Option<String>,
    client: reqwest::Client,
}

pub const NTFY_DEFAULT_URL: &str = "https://ntfy.sh";

impl NtfyChannel {
    pub fn from_creds(creds: &Credentials) -> Result<Self, NotifyError> {
        let topic = require("ntfy", "ntfy.topic", creds.ntfy.topic.as_ref())?;
        let base = creds
            .ntfy
            .url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(NTFY_DEFAULT_URL)
            .trim_end_matches('/');
        Self::new(format!("{base}/{topic}"), creds.ntfy.token.clone())
    }

    pub fn new(endpoint: String, token: Option<String>) -> Result<Self, NotifyError> {
        Ok(Self {
            endpoint,
            token: token.filter(|t| !t.trim().is_empty()),
            client: http_client()?,
        })
    }
}

impl Channel for NtfyChannel {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        // ntfy headers must be Latin-1; the title is user-controlled
        // (run name) so it goes through the `X-Title` fallback of putting
        // it in the JSON body instead. JSON publish endpoint is the root
        // URL with `topic` in the body.
        let (root, topic) = match self.endpoint.rsplit_once('/') {
            Some((r, t)) => (r.to_string(), t.to_string()),
            None => (self.endpoint.clone(), String::new()),
        };
        let mut payload = serde_json::json!({
            "topic": topic,
            "title": n.title,
            "message": if n.body.is_empty() { n.title.clone() } else { n.body.clone() },
            "priority": n.priority.ntfy_level(),
        });
        if !n.tags.is_empty() {
            payload["tags"] = serde_json::json!(n.tags);
        }
        let client = self.client.clone();
        let token = self.token.clone();
        block(async move {
            let mut req = client.post(&root).json(&payload);
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| NotifyError::Http(e.to_string()))?;
            check_status(resp).await
        })
    }
}

// ---------------------------------------------------------------------------
// Telegram
// ---------------------------------------------------------------------------

/// Bot API `sendMessage`. Plain text (no parse_mode) so run names with
/// underscores don't need escaping.
pub struct TelegramChannel {
    api_url: String,
    chat_id: String,
    client: reqwest::Client,
}

/// One inbound message, as returned by [`TelegramChannel::poll_updates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub chat_id: String,
    pub text: String,
}

pub const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

impl TelegramChannel {
    pub fn from_creds(creds: &Credentials) -> Result<Self, NotifyError> {
        let token = require(
            "telegram",
            "telegram.bot_token",
            creds.telegram.bot_token.as_ref(),
        )?;
        let chat = require(
            "telegram",
            "telegram.chat_id",
            creds.telegram.chat_id.as_ref(),
        )?;
        Self::new(
            format!("{TELEGRAM_API_BASE}/bot{token}/sendMessage"),
            chat.to_string(),
        )
    }

    /// `api_url` is the full `…/bot<token>/sendMessage` URL (override the
    /// host for tests).
    pub fn new(api_url: String, chat_id: String) -> Result<Self, NotifyError> {
        Ok(Self {
            api_url,
            chat_id,
            client: http_client()?,
        })
    }
}

impl TelegramChannel {
    /// The chat this channel is bound to. Inbound commands from any other
    /// chat are ignored by the watchdog.
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    /// `getUpdates` since `offset` (exclusive of already-seen ids: pass
    /// `last_update_id + 1`). Short-poll (`timeout=0`), so this returns
    /// immediately — the watchdog runs on a schedule, not a long-poll loop.
    pub fn poll_updates(&self, offset: Option<i64>) -> Result<Vec<TelegramUpdate>, NotifyError> {
        let base = self.api_url.trim_end_matches("/sendMessage").to_string();
        let mut url = format!("{base}/getUpdates?timeout=0&allowed_updates=%5B%22message%22%5D");
        if let Some(o) = offset {
            url.push_str(&format!("&offset={o}"));
        }
        let client = self.client.clone();
        block(async move {
            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| NotifyError::Http(e.to_string()))?;
            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| NotifyError::Http(format!("HTTP {status}: {e}")))?;
            if !status.is_success() || body.get("ok") != Some(&serde_json::Value::Bool(true)) {
                return Err(NotifyError::Http(format!(
                    "HTTP {status}: {}",
                    body.get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("getUpdates failed")
                )));
            }
            let mut out = Vec::new();
            for u in body
                .get("result")
                .and_then(|r| r.as_array())
                .into_iter()
                .flatten()
            {
                let Some(id) = u.get("update_id").and_then(|v| v.as_i64()) else {
                    continue;
                };
                let Some(msg) = u.get("message") else {
                    continue;
                };
                let chat_id = match msg.get("chat").and_then(|c| c.get("id")) {
                    Some(serde_json::Value::Number(n)) => n.to_string(),
                    Some(serde_json::Value::String(s)) => s.clone(),
                    _ => continue,
                };
                let text = msg
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                out.push(TelegramUpdate {
                    update_id: id,
                    chat_id,
                    text,
                });
            }
            Ok(out)
        })
    }
}

impl Channel for TelegramChannel {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        let payload = serde_json::json!({
            "chat_id": self.chat_id,
            "text": n.plain_text(),
            "disable_notification": n.priority == Priority::Low,
        });
        let client = self.client.clone();
        let url = self.api_url.clone();
        block(async move {
            let resp = client
                .post(&url)
                .json(&payload)
                .send()
                .await
                .map_err(|e| NotifyError::Http(e.to_string()))?;
            check_status(resp).await
        })
    }
}

// ---------------------------------------------------------------------------
// Generic webhook
// ---------------------------------------------------------------------------

/// JSON POST. The payload carries both `text` (Slack incoming webhooks) and
/// `content` (Discord) so either URL works unmodified, plus the structured
/// fields for your own endpoint.
pub struct WebhookChannel {
    url: String,
    client: reqwest::Client,
}

impl WebhookChannel {
    pub fn from_creds(creds: &Credentials) -> Result<Self, NotifyError> {
        let url = require("webhook", "webhook.url", creds.webhook.url.as_ref())?;
        Self::new(url.to_string())
    }

    pub fn new(url: String) -> Result<Self, NotifyError> {
        Ok(Self {
            url,
            client: http_client()?,
        })
    }

    pub fn payload(n: &Notification) -> serde_json::Value {
        let text = n.plain_text();
        serde_json::json!({
            "source": "xrun",
            "kind": n.kind.as_str(),
            "priority": n.priority.as_str(),
            "run_id": n.run_id,
            "title": n.title,
            "body": n.body,
            "ts": chrono::Utc::now().to_rfc3339(),
            "text": text,
            "content": text,
        })
    }
}

impl Channel for WebhookChannel {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        let payload = Self::payload(n);
        let client = self.client.clone();
        let url = self.url.clone();
        block(async move {
            let resp = client
                .post(&url)
                .json(&payload)
                .send()
                .await
                .map_err(|e| NotifyError::Http(e.to_string()))?;
            check_status(resp).await
        })
    }
}

// ---------------------------------------------------------------------------
// Desktop toast
// ---------------------------------------------------------------------------

/// OS notification. Only useful while you're at the machine; kept because
/// it's free and it's what the TUI can't do while it isn't running.
pub struct DesktopChannel;

impl DesktopChannel {
    pub fn new() -> Result<Self, NotifyError> {
        #[cfg(feature = "desktop")]
        {
            Ok(Self)
        }
        #[cfg(not(feature = "desktop"))]
        {
            Err(NotifyError::Misconfigured {
                channel: "desktop",
                reason: "xrun was built without the `desktop` feature".into(),
            })
        }
    }
}

impl Channel for DesktopChannel {
    fn name(&self) -> &'static str {
        "desktop"
    }

    #[cfg(feature = "desktop")]
    fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        let mut toast = notify_rust::Notification::new();
        toast.summary(&n.title).body(&n.body).appname("xrun");
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let urgency = match n.priority {
                Priority::Low => notify_rust::Urgency::Low,
                Priority::Default | Priority::High => notify_rust::Urgency::Normal,
                Priority::Urgent => notify_rust::Urgency::Critical,
            };
            toast.urgency(urgency);
        }
        toast
            .show()
            .map(|_| ())
            .map_err(|e| NotifyError::Other(format!("desktop toast failed: {e}")))
    }

    #[cfg(not(feature = "desktop"))]
    fn send(&self, _n: &Notification) -> Result<(), NotifyError> {
        Err(NotifyError::Misconfigured {
            channel: "desktop",
            reason: "xrun was built without the `desktop` feature".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    #[test]
    fn ntfy_requires_topic() {
        let creds = Credentials::default();
        let err = NtfyChannel::from_creds(&creds).err().expect("must fail");
        assert!(err.to_string().contains("ntfy.topic"), "{err}");
    }

    #[test]
    fn ntfy_defaults_base_url_and_strips_slash() {
        let mut creds = Credentials::default();
        creds.ntfy.topic = Some("xrun-abc".into());
        let ch = NtfyChannel::from_creds(&creds).unwrap();
        assert_eq!(ch.endpoint, "https://ntfy.sh/xrun-abc");
        creds.ntfy.url = Some("https://ntfy.example.com/".into());
        let ch = NtfyChannel::from_creds(&creds).unwrap();
        assert_eq!(ch.endpoint, "https://ntfy.example.com/xrun-abc");
    }

    #[test]
    fn telegram_requires_both_fields() {
        let mut creds = Credentials::default();
        creds.telegram.bot_token = Some("123:abc".into());
        let err = TelegramChannel::from_creds(&creds)
            .err()
            .expect("must fail");
        assert!(err.to_string().contains("telegram.chat_id"), "{err}");
    }

    #[test]
    fn unknown_channel_name_is_rejected() {
        let err = build("pager", &Credentials::default()).err().unwrap();
        assert!(err.to_string().contains("unknown notify channel"));
    }

    #[test]
    fn webhook_payload_has_slack_and_discord_fields() {
        let n = Notification::new(Kind::RunDone, "k", "title").body("body");
        let p = WebhookChannel::payload(&n);
        assert_eq!(p["text"], "title\n\nbody");
        assert_eq!(p["content"], "title\n\nbody");
        assert_eq!(p["kind"], "run.done");
        assert_eq!(p["source"], "xrun");
    }
}
