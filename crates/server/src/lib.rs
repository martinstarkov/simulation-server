use std::sync::{Arc, Mutex};

use interface::ServerMode;
use simulator::MySim;

use crate::server::{SimServer, create_custom_server};

pub mod server;

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,interface=debug".into()),
        )
        .with_target(false)
        .try_init(); // no panic if already initialized
}

/// Create a `SimServer` with the default `MySim` simulation.
///
/// This is a convenience wrapper so you can just say
/// `create_server(ServerMode::WithGrpc("127.0.0.1:50051".into()))`
/// instead of manually building an `Arc<Mutex<MySim>>`.
pub fn create_server(mode: ServerMode) -> SimServer<MySim> {
    let sim = Arc::new(Mutex::new(MySim));
    create_custom_server(mode, sim)
}

pub fn create_local_server() -> Arc<SimServer<MySim>> {
    Arc::new(create_server(ServerMode::LocalOnly))
}

pub fn create_remote_server(addr: &str) -> SimServer<MySim> {
    create_server(ServerMode::WithGrpc(addr.to_string()))
}
