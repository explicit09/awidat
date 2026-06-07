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
    /// Live subscriber — forward directly.
    Subscriber(mpsc::Sender<ProgressEvent>),
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
        if let Some(ProgressSlot::Backlog(backlog)) = map.remove(&token) {
            for event in backlog {
                let _ = tx.try_send(event);
            }
        }
        map.insert(token, ProgressSlot::Subscriber(tx));
        ProgressGuard {
            map: self.progress.clone(),
            token,
        }
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
            Some(ProgressSlot::Subscriber(tx)) => {
                let _ = tx.try_send(event);
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
pub(crate) struct ProgressGuard {
    map: ProgressMap,
    token: i64,
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        // We can't `await` the lock from Drop; spawn a one-shot to deregister.
        // The map is Arc'd so the spawned future stays valid.
        let map = self.map.clone();
        let token = self.token;
        tokio::spawn(async move {
            map.lock().await.remove(&token);
        });
    }
}
