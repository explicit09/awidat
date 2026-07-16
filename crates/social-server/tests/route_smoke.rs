//! R16 — route-registration smoke: every route in the production router must
//! answer an unauthenticated probe with a clean auth/validation error (4xx or
//! benign 2xx), never a 5xx/panic.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{serve, state_with};
use montage_social_server::{ServerConfig, StoreHandle};

/// The full route table from `build_router`, with path params filled in.
/// Keep in sync with `crates/social-server/src/lib.rs::build_router`.
const ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("GET", "/providers"),
    ("POST", "/artifacts/upload-url"),
    ("GET", "/public/artifacts/probe.mp4"),
    ("POST", "/oauth/begin/youtube"),
    ("GET", "/oauth/callback/youtube"),
    ("GET", "/social/accounts"),
    ("POST", "/social/oauth/start/youtube"),
    ("POST", "/social/accounts/acct_x/disconnect"),
    ("GET", "/social/accounts/acct_x/audit"),
    ("POST", "/social/targets/bind"),
    ("POST", "/social/targets/update"),
    ("POST", "/social/targets/validate"),
    ("POST", "/social/targets/schedule"),
    ("GET", "/social/jobs/job_x"),
    ("POST", "/social/jobs/job_x/cancel"),
    ("POST", "/social/jobs/job_x/retry"),
    ("POST", "/social/jobs/job_x/fire"),
    ("POST", "/social/jobs/job_x/poll"),
    ("POST", "/social/jobs/job_x/reschedule"),
    ("POST", "/social/jobs/job_x/upload-url"),
    ("POST", "/social/jobs/job_x/upload-complete"),
    ("POST", "/internal/tick"),
    ("POST", "/internal/cron/poll-processing"),
    ("POST", "/internal/cron/refresh-tokens"),
];

/// R16.10 — every registered route responds to an unauthenticated probe with a
/// non-5xx status (auth or validation error, not a panic or internal error).
#[tokio::test]
async fn every_route_answers_unauthenticated_probe_without_5xx() {
    let config = ServerConfig {
        // Non-empty secrets so the bearer gates fail closed (empty-string
        // secrets would let an unauthenticated probe through bearer_auth).
        service_shared_secret: "smoke-secret".into(),
        desktop_auth_token: "smoke-desktop".into(),
        ..ServerConfig::default()
    };
    let base = serve(state_with(config, StoreHandle::in_memory())).await;
    let client = reqwest::Client::new();

    let mut failures = Vec::new();
    for (verb, route) in ROUTES {
        let url = format!("{base}{route}");
        let req = match *verb {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            other => panic!("unsupported probe verb {other}"),
        };
        let status = req.send().await.unwrap().status().as_u16();
        if status >= 500 {
            failures.push(format!("{verb} {route} -> {status}"));
        }
    }
    assert!(
        failures.is_empty(),
        "routes answered an unauthenticated probe with 5xx:\n{}",
        failures.join("\n")
    );
}
