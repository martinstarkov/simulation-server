//! interface: unified proto + simulator core + sync client

//! Prost/tonic-generated protobuf and service definitions.
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/gen/interface.rs"));

pub use client_msg::Body as ClientMsgBody;
pub use server_msg::Body as ServerMsgBody;

/// Trait for custom simulation logic that handles messages other than registration/barrier logic.
pub trait Simulation: Send + Sync + 'static {
    /// Handle a message; return any responses to broadcast to clients.
    fn handle_message(&mut self, body: ClientMsg) -> anyhow::Result<Vec<ServerMsg>>;

    fn dt(&self) -> f32;
}

#[derive(Clone, Debug)]
pub enum ServerMode {
    LocalOnly,
    WithGrpc(String),
}
