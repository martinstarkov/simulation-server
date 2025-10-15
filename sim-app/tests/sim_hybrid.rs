use sim_app::{
    client::{new_client, Mode},
    init_tracing,
};
use std::net::SocketAddr;
use std::time::Duration;

use crate::common::{run_sim, spawn_remote_connector};

mod common;

#[test]
fn test_hybrid_local() -> anyhow::Result<()> {
    init_tracing();

    let addr: SocketAddr = "127.0.0.1:60000".parse()?;

    let app_id = "client-1";

    let client = new_client(Mode::Hybrid, app_id, Some(addr))?;

    run_sim(client, app_id, 5)?;
    Ok(())
}

#[test]
fn test_hybrid_viewer_nonblocking() -> anyhow::Result<()> {
    init_tracing();

    let addr: SocketAddr = "127.0.0.1:60000".parse()?;

    let app_id = "client-1";

    let client = new_client(Mode::Hybrid, app_id, Some(addr))?;

    std::thread::sleep(Duration::from_millis(200)); // let service bind.

    let viewer = spawn_remote_connector(addr, "viewer-1", false, 5);

    run_sim(client, app_id, 5)?;

    let viewer_res = viewer.join().expect("Viewer thread panicked");
    assert!(viewer_res.is_ok(), "Viewer failed: {viewer_res:?}");

    Ok(())
}

#[test]
fn test_hybrid_viewer_blocking() -> anyhow::Result<()> {
    init_tracing();

    let addr: SocketAddr = "127.0.0.1:60000".parse()?;

    let app_id = "client-1";

    let client = new_client(Mode::Hybrid, app_id, Some(addr))?;

    std::thread::sleep(Duration::from_millis(200)); // let service bind.

    let viewer = spawn_remote_connector(addr, "viewer-1", true, 5);

    run_sim(client, app_id, 5)?;

    let viewer_res = viewer.join().expect("Viewer thread panicked");
    assert!(viewer_res.is_ok(), "Viewer failed: {viewer_res:?}");

    Ok(())
}
