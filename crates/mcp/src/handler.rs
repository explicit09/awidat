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
use tokio::sync::Notify;
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
    Subscriber(SubscriberState),
}

/// Per-token live-subscriber state.
///
/// rmcp 1.8 dispatches each `notifications/progress` frame on an *independent*
/// `tokio::spawn`ed task (`service.rs` `handle_notification`), so those tasks
/// can run out of order and — under load — may not have run at all by the time
/// the originating request's response resolves on the client. Tearing the
/// subscriber down at that point drops any not-yet-run frame. To make teardown
/// wait for a *real* end-of-stream instead of a timing guess, we track how many
/// frames were delivered and the completion `total` the server advertised, and
/// fire `complete` once every expected frame is in.
struct SubscriberState {
    tx: mpsc::Sender<ProgressEvent>,
    /// Frames handed to the channel so far (backlog drain + live).
    delivered: usize,
    /// The `total` the server advertised (the completion target for these
    /// unit-counting progress streams: a final frame carries `progress ==
    /// total`, and there are `total` frames). `None` until a frame with a
    /// `total` is seen; while unknown no hard completion can be computed and
    /// the client's `complete` wait is bounded only by its cap.
    total: Option<usize>,
    /// Fired once `delivered >= total` (all expected frames delivered). The
    /// client awaits this before deregistering, so completion is keyed on
    /// observed delivery of every frame — order-independent and not a timing
    /// window.
    complete: Arc<Notify>,
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
    /// dropped). Returns a guard that deregisters the subscriber on drop, plus
    /// a `Notify` that fires once every advertised frame has been delivered
    /// (see [`SubscriberState`]).
    pub(crate) async fn register_progress(
        &self,
        token: i64,
        tx: mpsc::Sender<ProgressEvent>,
    ) -> (ProgressGuard, Arc<Notify>) {
        let complete = Arc::new(Notify::new());
        let mut map = self.progress.lock().await;
        let mut state = SubscriberState {
            tx,
            delivered: 0,
            total: None,
            complete: complete.clone(),
        };
        if let Some(ProgressSlot::Backlog(backlog)) = map.remove(&token) {
            for event in backlog {
                deliver_into(&mut state, event);
            }
        }
        map.insert(token, ProgressSlot::Subscriber(state));
        let guard = ProgressGuard {
            map: self.progress.clone(),
            token,
            disarmed: false,
        };
        (guard, complete)
    }

    /// Whether the subscriber for `token` has already received its full frame
    /// burst (`delivered >= total`), or no subscriber exists. When this is
    /// false the client awaits the completion `Notify`. Lets the client
    /// short-circuit when every frame was already drained from the backlog at
    /// registration (so `notify_waiters()` fired before anyone was waiting).
    pub(crate) async fn progress_complete(&self, token: i64) -> bool {
        match self.progress.lock().await.get(&token) {
            Some(ProgressSlot::Subscriber(s)) => s.is_complete(),
            _ => true,
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

impl SubscriberState {
    /// Whether every advertised frame has been delivered. Unknown `total`
    /// (server sent no `total`, or no frame yet) is *not* a completion, so the
    /// client keeps awaiting the signal (bounded by its cap) rather than
    /// tearing the subscriber down early.
    fn is_complete(&self) -> bool {
        matches!(self.total, Some(total) if self.delivered >= total)
    }
}

/// Deliver one frame into a subscriber: forward to the channel, update the
/// delivered count, learn the completion `total`, and fire the completion
/// signal once every expected frame is in. Order-independent — the count, not
/// which specific frame arrived, decides completion — so it is robust to
/// rmcp's out-of-order notification tasks.
fn deliver_into(state: &mut SubscriberState, event: ProgressEvent) {
    // A `total` on any frame tells us how many unit-counting frames to expect.
    // Take the max seen in case frames arrive out of order with a growing total.
    if let Some(total) = event.total
        && total.is_finite()
        && total >= 0.0
    {
        let total = total.ceil() as usize;
        state.total = Some(state.total.map_or(total, |cur| cur.max(total)));
    }
    if state.tx.try_send(event).is_ok() {
        state.delivered += 1;
    }
    if state.is_complete() {
        state.complete.notify_waiters();
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
            Some(ProgressSlot::Subscriber(state)) => {
                deliver_into(state, event);
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
