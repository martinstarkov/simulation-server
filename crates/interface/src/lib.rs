//! interface: unified proto + simulator core + sync client

pub mod interface {
    //! Prost/tonic-generated protobuf and service definitions.
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/gen/interface.rs"));
}

/// Trait for custom simulation logic that handles messages other than registration/barrier logic.
pub trait Simulation: Send + Sync + 'static {
    /// Handle a message; return any responses to broadcast to clients.
    fn handle_message(
        &mut self,
        msg: interface::ClientMsg,
    ) -> anyhow::Result<Vec<interface::ServerMsg>>;
}

#[derive(Clone, Debug)]
pub enum ServerMode {
    LocalOnly,
    WithGrpc(String),
}
