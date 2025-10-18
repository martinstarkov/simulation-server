pub mod client;
pub mod server;
pub mod service;

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,interface=debug".into()),
        )
        .with_target(false)
        .try_init(); // no panic if already initialized
}

pub fn safe_block_on<F: std::future::Future>(fut: F) -> F::Output {
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already inside a runtime — block in place
        tokio::task::block_in_place(|| futures::executor::block_on(fut))
    } else {
        // Create a lightweight, single-threaded runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(fut)
    }
}
