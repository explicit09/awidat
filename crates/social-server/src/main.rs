//! Thin binary entry point — all server logic lives in the library so
//! route-level tests can construct the router hermetically. See `lib.rs`.

#[tokio::main]
async fn main() {
    montage_social_server::run().await;
}
