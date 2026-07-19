//! `ClientHandler` impl that routes server-initiated notifications and
//! advertises our `ClientInfo` to the handshake.
//!
//! `rmcp` requires the client to provide a handler implementing
//! [`rmcp::ClientHandler`]; the default impl is a no-op for every callback
//! and reports `ClientInfo::default()`. We override [`MontageHandler::get_info`]
//! so the handshake announces our app name+version, and override
//! [`MontageHandler::on_progress`] so per-request progress subscribers
//! (registered via [`MontageHandler::register_progress`]) see
//! `notifications/progress` frames keyed to their progress token.
//!
//! All other server-initiated notifications (logging, cancellation echoes,
//! resource list changes, …) get the default no-op for now. Wire them up
//! when an agent feature actually needs them.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ClientHandler;
use rmcp::model::{ClientInfo, NumberOrString, ProgressNotificationParam, ProgressToken};
use rmcp::service::{NotificationContext, RoleClient};
use tokio::sync::Mutex;
use tokio::sync::mpsc;

/// One `notifications/progress` event from the server.
#[derive(Debug, Clone)]
pub struct ProgressEvent {
    /// Monotonically-increasing progress value. Spec doesn't fix units —
    /// percent (0–100), bytes processed, frames, etc.
    pub progress: f64,
    /// Optional total. When present, `progress / total` is a fraction.
    pub total: Option<f64>,
    /// Optional human-readable status message.
    pub message: Option<String>,
}

/// State for one progress token: either a live subscriber, or a small
/// backlog of events that arrived before the caller registered.
///
/// The backlog exists because `rmcp` allocates the progress token *inside*
/// `send_request_with_option`, so the caller can only learn the token (and
/// register a subscriber) *after* the request has been sent. With a fast
/// server, several `notifications/progress` frames can arrive in that
/// window. Without a backlog we'd silently drop them — see the
/// `progress_notifications_are_routed_to_subscriber` test.
enum ProgressSlot {
    /// No subscriber yet — buffer up to BACKLOG_CAP events.
    Backlog(Vec<ProgressEvent>),
    /// Live subscriber — forward directly. `delivered` counts frames handed
    /// to the channel (backlog drain + live), so the post-response drain in
    /// the client can wait until that count stops changing rather than
    /// racing a wall-clock timeout.
    Subscriber {
        tx: mpsc::Sender<ProgressEvent>,
        delivered: usize,
    },
}

const BACKLOG_CAP: usize = 64;

/// Subscriber registry for progress notifications. Keyed by the numeric
/// progress token rmcp injected on the originating request.
type ProgressMap = Arc<Mutex<HashMap<i64, ProgressSlot>>>;

/// `ClientHandler` for montage. Cheaply cloneable — both the
/// `RunningService` and the per-call subscription registration share one.
#[derive(Clone, Default)]
pub(crate) struct MontageHandler {
    progress: ProgressMap,
    /// `ClientInfo` returned to rmcp during the handshake. `None` until
    /// [`Self::set_client_info`] is called by `Client::initialize`.
    client_info: Arc<Mutex<Option<ClientInfo>>>,
}

impl MontageHandler {
    /// Pre-load the `ClientInfo` rmcp will emit during the handshake.
    /// Must be called before `serve(transport)` is awaited.
    pub(crate) async fn set_client_info(&self, info: ClientInfo) {
        *self.client_info.lock().await = Some(info);
    }

    /// Register a progress subscriber under the given numeric token.
    /// Drains any backlog accumulated between request send and registration
    /// onto `tx` (best-effort — events that don't fit the channel are
    /// dropped). Returns a guard that deregisters the subscriber on drop.
    pub(crate) async fn register_progress(
        &self,
        token: i64,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> ProgressGuard {
        let mut map = self.progress.lock().await;
        let mut delivered = 0usize;
        if let Some(ProgressSlot::Backlog(backlog)) = map.remove(&token) {
            for event in backlog {
                if tx.try_send(event).is_ok() {
                    delivered += 1;
                }
            }
        }
        map.insert(token, ProgressSlot::Subscriber { tx, delivered });
        ProgressGuard {
            map: self.progress.clone(),
            token,
            disarmed: false,
        }
    }

    /// Number of frames delivered to the subscriber's channel for `token` so
    /// far, or `None` if no live subscriber is registered. Used by the
    /// client's post-response drain to detect quiescence deterministically.
    pub(crate) async fn delivered_count(&self, token: i64) -> Option<usize> {
        match self.progress.lock().await.get(&token) {
            Some(ProgressSlot::Subscriber { delivered, .. }) => Some(*delivered),
            _ => None,
        }
    }

    /// Synchronously (from an async context) deregister the subscriber for
    /// `token`. Unlike the `ProgressGuard` `Drop` fallback — which must spawn
    /// because `Drop` can't `await` — this removes the entry deterministically
    /// before the caller returns, so no late notification can resurrect an
    /// orphaned backlog after teardown.
    pub(crate) async fn deregister_progress(&self, token: i64) {
        self.progress.lock().await.remove(&token);
    }
}

impl ClientHandler for MontageHandler {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let ProgressToken(NumberOrString::Number(token)) = params.progress_token else {
            // String tokens aren't part of our surface (rmcp's default
            // provider emits numeric ones). Drop silently.
            return;
        };
        let event = ProgressEvent {
            progress: params.progress,
            total: params.total,
            message: params.message,
        };
        let mut map = self.progress.lock().await;
        match map.get_mut(&token) {
            Some(ProgressSlot::Subscriber { tx, delivered }) => {
                if tx.try_send(event).is_ok() {
                    *delivered += 1;
                }
            }
            Some(ProgressSlot::Backlog(backlog)) => {
                if backlog.len() < BACKLOG_CAP {
                    backlog.push(event);
                }
            }
            None => {
                // First event for this token — start a backlog. The
                // subscriber will register Real Soon Now and drain it.
                map.insert(token, ProgressSlot::Backlog(vec![event]));
            }
        }
    }

    fn get_info(&self) -> ClientInfo {
        // We can't async-lock from a sync callback. The handler is set up
        // before `serve()` is called, so a try_lock with fallback to
        // default is safe.
        match self.client_info.try_lock() {
            Ok(guard) => guard.clone().unwrap_or_default(),
            Err(_) => ClientInfo::default(),
        }
    }
}

/// RAII guard that deregisters a progress subscriber when dropped.
///
/// The preferred teardown path is [`ProgressGuard::disarm`] plus an explicit
/// `deregister_progress` on the handler, which removes the entry
/// deterministically from an async context after the post-response drain.
/// The `Drop` impl is only a best-effort safety net for panics / early
/// returns that skip the explicit path; it must spawn because `Drop` can't
/// `await`.
pub(crate) struct ProgressGuard {
    map: ProgressMap,
    token: i64,
    disarmed: bool,
}

impl ProgressGuard {
    /// The token this guard tears down.
    pub(crate) fn token(&self) -> i64 {
        self.token
    }

    /// Suppress the `Drop` fallback because the caller deregisters
    /// synchronously via `deregister_progress` instead.
    pub(crate) fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // We can't `await` the lock from Drop; spawn a one-shot to deregister.
        // The map is Arc'd so the spawned future stays valid.
        let map = self.map.clone();
        let token = self.token;
        tokio::spawn(async move {
            map.lock().await.remove(&token);
        });
    }
}
