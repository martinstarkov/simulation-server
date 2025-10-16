use anyhow::Result;
use interface::{
    Simulation,
    interface::{ClientMsg, ServerMsg},
};
use server::server::{Coordinator, SimServer};

/// A minimal local client handle (used only for the in-proc path).
#[derive(Clone)]
pub struct SimClient<S: Simulation> {
    name: String,
    contributing: bool,
    local: Option<LocalLinked<S>>,
}

#[derive(Clone)]
pub struct LocalLinked<S: Simulation> {
    pub coord: Coordinator<S>,
}

/// Create a new simulation client handle.
pub fn create_client<S: Simulation>(name: &str, contributing: bool) -> SimClient<S> {
    SimClient {
        name: name.to_string(),
        contributing,
        local: None,
    }
}

/// Link a client in-process to a running `SimServer` (ultra-low latency).
pub fn link<S: Simulation>(server: &SimServer<S>, client: &mut SimClient<S>) -> Result<()> {
    if client.local.is_some() {
        return Ok(());
    }

    client.local = Some(LocalLinked {
        coord: server.coord.clone(),
    });
    Ok(())
}

impl<S: Simulation> SimClient<S> {
    /// Send a message to the local simulation system through the coordinator.
    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        let local = self.local.as_ref().unwrap();
        local.coord.send_message(msg);
        Ok(())
    }

    /// Register this client with the simulation system.
    pub fn register(&self) -> (u64, crossbeam_channel::Receiver<ServerMsg>) {
        let local = self.local.as_ref().unwrap();
        local.coord.register(&self.name, self.contributing)
    }

    /// Signal that this client is ready for the next simulation step.
    pub fn step_ready(&self, client_id: u64) {
        let local = self.local.as_ref().unwrap();
        local.coord.step_ready(client_id);
    }
}
