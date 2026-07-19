mod manager_metrics;
mod otel_export_routing_policy;
// codex fork: upstream style, exempt from our -D warnings
#[allow(clippy::useless_borrows_in_formatting)]
mod otlp_http_loopback;
mod runtime_summary;
mod send;
mod snapshot;
mod timing;
mod validation;
