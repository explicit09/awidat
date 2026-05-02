//! Streaming Anthropic Messages-API client.
//!
//! Single public entry point: [`Client::messages_stream`]. Submits the
//! request, returns a `Stream<Item = Result<StreamEvent, ClientError>>`.
//! The agent loop consumes this stream until `Done` (or an error).

use std::time::Duration;

use async_stream::try_stream;
use futures::Stream;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::mpsc;

use super::messages::MessagesRequest;
use super::sse::forward_sse;
use super::stream::{StreamEvent, StreamParseError, StreamParser};
use super::{ANTHROPIC_API_VERSION, DEFAULT_BASE_URL};

/// Errors talking to the Messages API.
#[derive(Debug, Error)]
pub enum ClientError {
    /// HTTP / network failure.
    #[error("network error: {0}")]
    Network(String),
    /// Server returned a non-2xx with an error body.
    #[error("API returned {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from the server.
        message: String,
    },
    /// Stream closed before `message_stop`. Treat as retryable.
    #[error("stream closed prematurely: {0}")]
    StreamClosed(String),
    /// No event arrived within the idle window.
    #[error("idle timeout after {0:?} with no event")]
    Timeout(Duration),
    /// Parser error — malformed frame from the server.
    #[error("stream parse error: {0}")]
    Parse(#[from] StreamParseError),
    /// API key missing — neither env var nor keychain has it.
    #[error("ANTHROPIC_API_KEY not set in env or keychain")]
    MissingApiKey,
}

/// How to reach the API.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Base URL. Override only for testing.
    pub base_url: String,
    /// Anthropic version header value.
    pub api_version: String,
    /// Idle timeout: terminate the stream if no event arrives within this
    /// window. 90s matches Codex's `codex-rs/codex-client/src/lib.rs`.
    pub idle_timeout: Duration,
    /// Request connect+TLS+request timeout.
    pub request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            api_version: ANTHROPIC_API_VERSION.into(),
            idle_timeout: Duration::from_secs(90),
            request_timeout: Duration::from_secs(600),
        }
    }
}

/// Streaming Anthropic client.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: String,
    config: ClientConfig,
}

impl Client {
    /// Build a client. `api_key` is the bearer for `x-api-key`. Use
    /// [`Self::from_env_or_keychain`] for the production path.
    pub fn new(api_key: impl Into<String>, config: ClientConfig) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| ClientError::Network(e.to_string()))?;
        Ok(Self {
            http,
            api_key: api_key.into(),
            config,
        })
    }

    /// Resolve the API key from `ANTHROPIC_API_KEY` env var or the
    /// `awidat`-service keychain entry. Errors with [`ClientError::MissingApiKey`]
    /// if neither is set.
    pub fn from_env_or_keychain(config: ClientConfig) -> Result<Self, ClientError> {
        let key = awidat_secrets::get(
            awidat_secrets::env_vars::ANTHROPIC_API_KEY,
            awidat_secrets::accounts::ANTHROPIC_API_KEY,
        )
        .map_err(|e| ClientError::Network(format!("secret resolution failed: {e}")))?
        .ok_or(ClientError::MissingApiKey)?;
        Self::new(key, config)
    }

    /// Submit a streaming Messages request.
    ///
    /// Returns a stream of [`StreamEvent`]s. The stream terminates after
    /// emitting either a [`StreamEvent::Done`] (success) or an `Err` (any
    /// failure mode).
    pub fn messages_stream(
        &self,
        request: MessagesRequest,
    ) -> impl Stream<Item = Result<StreamEvent, ClientError>> + Send + use<> {
        let http = self.http.clone();
        let api_key = self.api_key.clone();
        let url = format!("{}/v1/messages", self.config.base_url);
        let api_version = self.config.api_version.clone();
        let idle_timeout = self.config.idle_timeout;

        try_stream! {
            let mut req = request;
            req.stream = true;
            let resp = http
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", &api_version)
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .json(&req)
                .send()
                .await
                .map_err(|e| ClientError::Network(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                let message = parse_api_error(&body).unwrap_or(body);
                Err(ClientError::Api { status, message })?;
                return;
            }

            // Spawn the SSE pump; consume frames from the channel and parse.
            let byte_stream = resp.bytes_stream();
            let (tx, mut rx) = mpsc::channel::<Result<String, ClientError>>(64);
            forward_sse(byte_stream, idle_timeout, tx);

            let mut parser = StreamParser::new();
            while let Some(item) = rx.recv().await {
                let data = item?;
                for ev in parser.push(&data)? {
                    let done = matches!(ev, StreamEvent::Done { .. });
                    yield ev;
                    if done {
                        return;
                    }
                }
            }
            // Channel closed without a Done event — propagate as
            // StreamClosed so the caller can retry.
            Err(ClientError::StreamClosed(
                "SSE channel closed without message_stop".into(),
            ))?;
        }
    }
}

/// Server-error envelope: `{"type":"error","error":{"type":"...","message":"..."}}`.
#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    #[serde(rename = "type")]
    _ty: String,
    message: String,
}

fn parse_api_error(body: &str) -> Option<String> {
    serde_json::from_str::<ApiErrorBody>(body)
        .ok()
        .map(|b| b.error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_extracts_message() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens too low"}}"#;
        assert_eq!(parse_api_error(body).as_deref(), Some("max_tokens too low"));
    }

    #[test]
    fn parse_error_returns_none_for_unrelated_body() {
        assert!(parse_api_error("not json").is_none());
        assert!(parse_api_error(r#"{"unrelated":1}"#).is_none());
    }

    #[test]
    fn config_defaults_to_anthropic() {
        let c = ClientConfig::default();
        assert_eq!(c.base_url, DEFAULT_BASE_URL);
        assert_eq!(c.api_version, ANTHROPIC_API_VERSION);
    }
}
