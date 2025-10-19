//! interface: unified proto + simulator core + sync client

//! Prost/tonic-generated protobuf and service definitions.
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/gen/interface.rs"));

pub use client_msg::Body as ClientMsgBody;
pub use server_msg::Body as ServerMsgBody;

use anyhow::Result;

/// Trait for custom simulation logic that handles messages other than registration/barrier logic.
pub trait Simulation: Send + Sync + 'static {
    /// Handle a message from a client. Return zero or more `ServerMsg`s to broadcast.
    fn handle_message(&mut self, msg: ClientMsg) -> Result<Vec<ServerMsg>>;

    /// Advance the simulation by one step (dt).
    fn step(&mut self) -> Result<()>;

    /// Get the latest tick (step counter + simulation time).
    fn get_tick(&self) -> Tick;
}
