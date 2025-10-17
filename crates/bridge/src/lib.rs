pub mod client;
pub mod server;

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,interface=debug".into()),
        )
        .with_target(false)
        .try_init(); // no panic if already initialized
}
