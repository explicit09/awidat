//! W6.A1 — one-shot 127.0.0.1:8419 HTTP listener for OAuth callbacks.
//!
//! The publishing OAuth flow redirects the browser to
//! `http://127.0.0.1:8419/oauth/callback?code=…&state=…`. Without a
//! local listener the browser shows "connection refused" and the user
//! has to copy the URL bar's `code` query param into a paste form by
//! hand. This module spins up a single-connection HTTP server so the
//! redirect lands directly in the app.
//!
//! # Why hand-rolled HTTP
//!
//! We're parsing *one* request line, returning *one* response. Pulling
//! in `hyper` / `axum` for that is unjustified weight. The protocol
//! work boils down to:
//!
//! - Read until `\r\n\r\n` (end of headers).
//! - Pluck the `GET /oauth/callback?…` request line.
//! - Extract `code` / `state` from the query string.
//! - Write a fixed 200 OK with a short HTML body.
//!
//! Roughly 150 LOC; no transitive dependency footprint.
//!
//! # Lifecycle
//!
//! Single connection only. The `TcpListener` is dropped after the
//! response flushes, freeing port 8419 for the next OAuth attempt.
//! Concurrent OAuth flows are rejected with [`OAuthListenerError::PortInUse`]
//! — one provider connection at a time, by design.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::time::timeout;

/// Loopback address the OAuth redirect resolves to. Matches
/// [`super::oauth::REDIRECT_URI`].
pub const LISTEN_ADDR: &str = "127.0.0.1:8419";

/// Default upper bound on how long [`listen_for_callback`] waits for
/// the browser to redirect. Five minutes is generous — most providers
/// land the redirect in <30s — but accommodates a slow OAuth consent
/// screen on a fresh browser profile.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What the listener hands back when the redirect arrives intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCapture {
    /// Authorisation code the provider returned. Pass straight to
    /// `complete_oauth`.
    pub code: String,
    /// Echoed-back state nonce. Caller has already validated this
    /// matches the expected value before [`listen_for_callback`]
    /// returns — surfaced so consumers can log/correlate if needed.
    pub state: String,
}

/// Why the listener gave up. The Tauri command layer maps each variant
/// to a kind-stable string the frontend can match on for fallback
/// behaviour (e.g. drop back to the paste-the-code form on `PortInUse`
/// or `Timeout`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum OAuthListenerError {
    /// `127.0.0.1:8419` was already bound — another OAuth flow is in
    /// flight, or another app squatted the port.
    #[error("port {LISTEN_ADDR} already in use — close other OAuth flows and retry")]
    PortInUse,
    /// The redirect arrived but `state` didn't match what
    /// [`super::oauth::fresh_state`] handed out. Classic CSRF defence —
    /// the user's session is suspect, do not exchange the code.
    #[error("OAuth state mismatch — possible CSRF, ignoring redirect")]
    StateMismatch,
    /// The request line didn't parse as a `GET /oauth/callback?…`. The
    /// browser opened the wrong page, a curl-style probe hit the port,
    /// or the redirect URL was malformed.
    #[error("invalid OAuth callback request: {0}")]
    InvalidRequest(String),
    /// No connection landed inside the deadline. User probably closed
    /// the browser tab.
    #[error("timed out waiting for OAuth callback")]
    Timeout,
    /// Socket I/O blew up mid-handshake.
    #[error("OAuth listener I/O error: {0}")]
    Io(String),
}

impl OAuthListenerError {
    /// Short stable identifier the Tauri layer can serialise. Mirrors
    /// the pattern `ProviderError::kind` uses so the wire shape is
    /// uniform across publishing errors.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::PortInUse => "port_in_use",
            Self::StateMismatch => "state_mismatch",
            Self::InvalidRequest(_) => "invalid_request",
            Self::Timeout => "timeout",
            Self::Io(_) => "io",
        }
    }
}

/// Bind 127.0.0.1:8419, accept a single connection, parse the
/// redirect, and validate the state nonce. Returns the captured code
/// on success, an [`OAuthListenerError`] otherwise.
///
/// The listener is dropped before this function returns — port 8419
/// is freed in time for a subsequent OAuth attempt without a delay.
///
/// `expected_state` is the nonce returned by
/// [`super::oauth::fresh_state`] when the authorisation URL was built.
/// `timeout_duration` defaults to [`DEFAULT_TIMEOUT`] for production
/// callers; tests pass tight values to exercise the timeout path.
pub async fn listen_for_callback(
    expected_state: String,
    timeout_duration: Duration,
) -> Result<OAuthCapture, OAuthListenerError> {
    listen_for_callback_at(LISTEN_ADDR, expected_state, timeout_duration).await
}

/// Address-parameterised variant of [`listen_for_callback`]. Production
/// always uses [`LISTEN_ADDR`]; tests pass `"127.0.0.1:0"` so each
/// concurrent test grabs an ephemeral port instead of fighting over
/// 8419 (Cargo runs lib tests in parallel by default).
///
/// Note: when an ephemeral address is used the caller can't predict
/// which port to dial — tests use the returned listener's `local_addr`
/// via the test-only [`listen_on_bound`] entry point below.
async fn listen_for_callback_at(
    addr: &str,
    expected_state: String,
    timeout_duration: Duration,
) -> Result<OAuthCapture, OAuthListenerError> {
    // Synchronous bind so the port-in-use error surfaces before we
    // hand back to the caller — they can choose to fall back to the
    // paste form without waiting on a timeout.
    let listener = TcpListener::bind(addr).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => OAuthListenerError::PortInUse,
        _ => OAuthListenerError::Io(e.to_string()),
    })?;

    listen_on_bound(listener, expected_state, timeout_duration).await
}

/// Drive an already-bound listener through one accept + parse + reply
/// cycle. Split out so tests can `bind("127.0.0.1:0")` themselves,
/// read the assigned port via [`TcpListener::local_addr`] for the
/// client, and then hand the listener in.
async fn listen_on_bound(
    listener: TcpListener,
    expected_state: String,
    timeout_duration: Duration,
) -> Result<OAuthCapture, OAuthListenerError> {
    match timeout(timeout_duration, accept_one(&listener, &expected_state)).await {
        Ok(result) => result,
        Err(_) => Err(OAuthListenerError::Timeout),
    }
}

async fn accept_one(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<OAuthCapture, OAuthListenerError> {
    let (stream, _peer) = listener
        .accept()
        .await
        .map_err(|e| OAuthListenerError::Io(e.to_string()))?;

    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| OAuthListenerError::Io(e.to_string()))?;

    let parsed = parse_request_line(&request_line)?;

    // We don't actually care about the rest of the headers — but we
    // do need to drain them so the browser's request is fully consumed
    // before we write the response. Otherwise some browsers (Safari)
    // log a "connection reset" before parsing the body.
    loop {
        let mut header_line = String::new();
        match reader.read_line(&mut header_line).await {
            Ok(0) => break,
            Ok(_) => {
                if header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
                    break;
                }
            }
            Err(_) => break, // tolerate broken pipes — we already have the request line
        }
    }

    if parsed.state != expected_state {
        // Write a different body so the user knows on the browser side
        // that something went wrong — they're not staring at a success
        // page while the backend silently dropped the redirect.
        let _ = write_response(&mut write_half, ERROR_BODY_STATE).await;
        return Err(OAuthListenerError::StateMismatch);
    }

    write_response(&mut write_half, SUCCESS_BODY)
        .await
        .map_err(|e| OAuthListenerError::Io(e.to_string()))?;

    Ok(OAuthCapture {
        code: parsed.code,
        state: parsed.state,
    })
}

/// Parsed `code` / `state` pair from the request line. Internal —
/// callers only see the public [`OAuthCapture`].
#[derive(Debug)]
struct ParsedCallback {
    code: String,
    state: String,
}

/// Pull `code` and `state` out of a `GET /oauth/callback?…` request
/// line. Accepts any query-string order; rejects missing params and
/// non-`/oauth/callback` paths.
fn parse_request_line(line: &str) -> Result<ParsedCallback, OAuthListenerError> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| OAuthListenerError::InvalidRequest("empty request line".into()))?;
    let target = parts
        .next()
        .ok_or_else(|| OAuthListenerError::InvalidRequest("missing request target".into()))?;

    if method != "GET" {
        return Err(OAuthListenerError::InvalidRequest(format!(
            "unsupported method {method}",
        )));
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    if path != "/oauth/callback" {
        return Err(OAuthListenerError::InvalidRequest(format!(
            "unexpected path {path}",
        )));
    }

    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    let mut error_param: Option<String> = None;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k {
            "code" => code = Some(percent_decode(v)),
            "state" => state = Some(percent_decode(v)),
            "error" => error_param = Some(percent_decode(v)),
            _ => {}
        }
    }

    // Providers return `?error=access_denied` when the user clicks
    // Cancel on the consent screen. Surface that as InvalidRequest so
    // the frontend can show "Authorization was denied" rather than a
    // generic "missing code" error.
    if let Some(err) = error_param {
        return Err(OAuthListenerError::InvalidRequest(format!(
            "provider returned error: {err}",
        )));
    }

    let code = code.ok_or_else(|| OAuthListenerError::InvalidRequest("missing code".into()))?;
    let state = state.ok_or_else(|| OAuthListenerError::InvalidRequest("missing state".into()))?;

    if code.is_empty() {
        return Err(OAuthListenerError::InvalidRequest("empty code".into()));
    }

    Ok(ParsedCallback { code, state })
}

/// Decode a percent-encoded query value. Mirrors
/// [`super::oauth::percent_encode`] so a round-trip through the
/// browser yields the original code. We tolerate malformed `%`
/// sequences by passing the byte through — providers' codes are
/// always ASCII-safe in practice, so the worst case is a noisier
/// payload that the provider's token endpoint will reject anyway.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h as u8) << 4) | (l as u8));
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| {
        // Bytes weren't valid UTF-8 — fall back to a lossy decode so
        // we don't lose the code. Real OAuth codes are ASCII.
        String::from_utf8_lossy(e.as_bytes()).into_owned()
    })
}

const SUCCESS_BODY: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\">",
    "<title>Awidat — Authorization complete</title>",
    "<style>",
    "body{font:14px -apple-system,system-ui,sans-serif;",
    "background:#0a0a0a;color:#e5e5e5;display:flex;",
    "align-items:center;justify-content:center;height:100vh;margin:0}",
    ".card{text-align:center;padding:32px 48px;",
    "border:1px solid #262626;border-radius:8px;background:#171717}",
    ".check{font-size:32px;color:#4ade80;margin-bottom:8px}",
    "</style></head><body>",
    "<div class=\"card\">",
    "<div class=\"check\">&#10003;</div>",
    "<div>Authorization complete</div>",
    "<div style=\"margin-top:6px;color:#a3a3a3;font-size:13px\">",
    "You can close this tab and return to Awidat.",
    "</div></div></body></html>",
);

const ERROR_BODY_STATE: &str = concat!(
    "<!doctype html><html><head><meta charset=\"utf-8\">",
    "<title>Awidat — Authorization rejected</title>",
    "<style>",
    "body{font:14px -apple-system,system-ui,sans-serif;",
    "background:#0a0a0a;color:#e5e5e5;display:flex;",
    "align-items:center;justify-content:center;height:100vh;margin:0}",
    ".card{text-align:center;padding:32px 48px;",
    "border:1px solid #262626;border-radius:8px;background:#171717}",
    ".x{font-size:32px;color:#f87171;margin-bottom:8px}",
    "</style></head><body>",
    "<div class=\"card\">",
    "<div class=\"x\">&#10007;</div>",
    "<div>Authorization rejected</div>",
    "<div style=\"margin-top:6px;color:#a3a3a3;font-size:13px\">",
    "State mismatch — possible CSRF. Restart the Connect flow.",
    "</div></div></body></html>",
);

async fn write_response<W>(write_half: &mut W, body: &str) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         \r\n\
         {body}",
        len = body.len(),
        body = body,
    );
    write_half.write_all(response.as_bytes()).await?;
    write_half.flush().await?;
    write_half.shutdown().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;

    /// Bind to an ephemeral port so concurrent test invocations can't
    /// collide on 8419. Returns the live listener + its bound address.
    async fn bind_ephemeral() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let addr = listener.local_addr().expect("local_addr");
        (listener, addr)
    }

    /// Help test bodies issue a GET against a specific listener
    /// address without hardcoding the URL string everywhere.
    async fn send_get(addr: std::net::SocketAddr, query: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.expect("connect to listener");
        let request = format!(
            "GET /oauth/callback?{query} HTTP/1.1\r\nHost: {addr}\r\n\r\n",
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response).await;
        response
    }

    #[tokio::test]
    async fn listen_returns_code_from_valid_redirect() {
        let (listener, addr) = bind_ephemeral().await;
        let server = tokio::spawn(listen_on_bound(
            listener,
            "the-expected-state".to_string(),
            Duration::from_secs(5),
        ));
        let response = send_get(addr, "code=auth-123&state=the-expected-state").await;
        let result = server.await.expect("join server task");
        let capture = result.expect("listen succeeds");
        assert_eq!(capture.code, "auth-123");
        assert_eq!(capture.state, "the-expected-state");
        assert!(
            response.contains("200 OK") && response.contains("Authorization complete"),
            "browser response: {response}",
        );
    }

    #[tokio::test]
    async fn rejects_state_mismatch() {
        let (listener, addr) = bind_ephemeral().await;
        let server = tokio::spawn(listen_on_bound(
            listener,
            "expected".to_string(),
            Duration::from_secs(5),
        ));
        let response = send_get(addr, "code=auth-456&state=wrong").await;
        let err = server
            .await
            .expect("join server task")
            .expect_err("state mismatch must fail");
        assert!(
            matches!(err, OAuthListenerError::StateMismatch),
            "got {err:?}",
        );
        assert!(
            response.contains("Authorization rejected"),
            "browser should see the rejection page; got: {response}",
        );
    }

    #[tokio::test]
    async fn times_out_when_no_callback() {
        let (listener, _addr) = bind_ephemeral().await;
        let result = listen_on_bound(
            listener,
            "doesnt-matter".to_string(),
            Duration::from_millis(150),
        )
        .await;
        let err = result.expect_err("must time out");
        assert!(matches!(err, OAuthListenerError::Timeout), "got {err:?}");
    }

    #[tokio::test]
    async fn second_bind_reports_port_in_use() {
        // Hold the port via a stable bind, then ask the listener to
        // bind the same address — must report PortInUse rather than
        // a generic Io error so the frontend can show the right copy.
        let (held_listener, addr) = bind_ephemeral().await;
        let result = listen_for_callback_at(
            &addr.to_string(),
            "doesnt-matter".to_string(),
            Duration::from_millis(100),
        )
        .await;
        let err = result.expect_err("must reject");
        assert!(matches!(err, OAuthListenerError::PortInUse), "got {err:?}");
        drop(held_listener);
    }

    #[test]
    fn parse_request_line_extracts_code_and_state() {
        let parsed = parse_request_line("GET /oauth/callback?code=abc&state=xyz HTTP/1.1\r\n")
            .expect("parse ok");
        assert_eq!(parsed.code, "abc");
        assert_eq!(parsed.state, "xyz");
    }

    #[test]
    fn parse_request_line_decodes_percent_encoding() {
        let parsed =
            parse_request_line("GET /oauth/callback?code=a%2Fb%3Dc&state=x%2By HTTP/1.1\r\n")
                .expect("parse ok");
        assert_eq!(parsed.code, "a/b=c");
        assert_eq!(parsed.state, "x+y");
    }

    #[test]
    fn parse_request_line_surfaces_provider_error() {
        let err = parse_request_line(
            "GET /oauth/callback?error=access_denied&state=x HTTP/1.1\r\n",
        )
        .expect_err("error param must reject");
        match err {
            OAuthListenerError::InvalidRequest(msg) => {
                assert!(msg.contains("access_denied"), "{msg}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_request_line_rejects_missing_code() {
        let err = parse_request_line("GET /oauth/callback?state=x HTTP/1.1\r\n")
            .expect_err("must reject");
        assert_eq!(err.kind(), "invalid_request");
    }

    #[test]
    fn parse_request_line_rejects_wrong_path() {
        let err = parse_request_line("GET /not-our-path?code=x&state=y HTTP/1.1\r\n")
            .expect_err("must reject");
        assert_eq!(err.kind(), "invalid_request");
    }

    #[test]
    fn parse_request_line_rejects_non_get() {
        let err = parse_request_line("POST /oauth/callback?code=x&state=y HTTP/1.1\r\n")
            .expect_err("must reject");
        assert_eq!(err.kind(), "invalid_request");
    }
}
