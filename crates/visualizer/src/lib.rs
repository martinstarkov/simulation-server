//! visualizer: minimal Bevy viewer wrappers for your simulator.
//!
//! Pick a mode, then call `run(mode)`.

mod app;
mod worker;

use std::sync::Arc;

use anyhow::Result;
use app::run_bevy;
use client::sync_client::{connect_local, connect_remote, connect_remote_stepper};
use interface::ServerMode;
use server::{create_server, init_tracing};
use worker::spawn_viewer_worker;

/// How to run the visualizer.
pub enum VisualizerMode {
    /// Remote non-contributing viewer (does not participate in the step barrier).
    RemoteNonBlocking { addr: String },

    /// Remote contributing viewer (waits at the step barrier each frame).
    RemoteBlocking { addr: String },

    /// Local contributing viewer that **creates** its own in-proc server (no gRPC).
    LocalBlockingWithServer,

    /// Start a gRPC server **in this process** on `addr`
    /// (e.g. "127.0.0.1:50051"), then connect as a remote
    /// **non-contributing** viewer.
    RemoteNonBlockingWithServer { addr: String },

    /// Start a gRPC server **in this process** on `addr`,
    /// then connect as a remote **contributing** viewer (barrier/lockstep).
    RemoteBlockingWithServer { addr: String },
}

/// Start the Bevy visualizer with the chosen mode.
pub fn run(mode: VisualizerMode) -> Result<()> {
    init_tracing();

    match mode {
        // --------------------------
        // LOCAL (blocking, creates new server)
        // --------------------------
        VisualizerMode::LocalBlockingWithServer => {
            // keep server alive for Bevy's lifetime
            let server = Arc::new(create_server(ServerMode::LocalOnly));

            let client = connect_local(&Arc::clone(&server))?;

            let (rx_app, tx_done) = spawn_viewer_worker("viewer-contrib-local", true, client);

            run_bevy(rx_app, tx_done);
        }

        // --------------------------
        // REMOTE (blocking, spawns server)
        // --------------------------
        VisualizerMode::RemoteBlockingWithServer { addr } => {
            let _server = create_server(ServerMode::WithGrpc(addr.to_string()));

            let client = connect_remote(&addr)?;

            let (rx_app, tx_done) =
                spawn_viewer_worker("viewer-contrib-remote-local", true, client);

            run_bevy(rx_app, tx_done);

            drop(_server);
        }

        // --------------------------
        // REMOTE (non-blocking, spawns server)
        // --------------------------
        VisualizerMode::RemoteNonBlockingWithServer { addr } => {
            let _server = create_server(ServerMode::WithGrpc(addr.to_string()));
            let client = connect_remote(&addr)?;

            let (rx_app, tx_done) =
                spawn_viewer_worker("viewer-noncontrib-remote-local", false, client);

            let stepper = connect_remote_stepper(&addr, "stepper", 1);

            run_bevy(rx_app, tx_done);

            stepper.join().unwrap()?;
            drop(_server); // not reached until exit
        }

        // --------------------------
        // REMOTE (non-blocking)
        // --------------------------
        VisualizerMode::RemoteNonBlocking { addr } => {
            let client = connect_remote(&addr)?;

            let (rx_app, tx_done) = spawn_viewer_worker("viewer-noncontrib", false, client);

            run_bevy(rx_app, tx_done);
        }

        // --------------------------
        // REMOTE (blocking)
        // --------------------------
        VisualizerMode::RemoteBlocking { addr } => {
            let client = connect_remote(&addr)?;

            let (rx_app, tx_done) = spawn_viewer_worker("viewer-contrib", true, client);

            run_bevy(rx_app, tx_done);
        }
    }

    Ok(())
}
