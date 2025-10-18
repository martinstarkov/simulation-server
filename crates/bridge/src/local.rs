use crossbeam_channel::{Receiver, Sender};
use interface::{ClientMsg, ClientMsgBody, ServerMsg};

use crate::{client::Client, server::SharedServer};

pub struct LocalClient {
    client_id: u64,
    to_server: Sender<ClientMsg>,
    from_server: Receiver<ServerMsg>,
}

impl LocalClient {
    pub fn new(server: &SharedServer, step_participant: bool) -> anyhow::Result<Self> {
        let (client_id, to_server, from_server) = server.register_client(step_participant);
        Ok(Self {
            client_id,
            to_server,
            from_server,
        })
    }
}

impl Client for LocalClient {
    fn send(&self, body: ClientMsgBody) {
        let _ = self.to_server.send(ClientMsg {
            client_id: self.client_id,
            body: Some(body),
        });
    }

    fn recv(&self) -> anyhow::Result<ServerMsg> {
        self.from_server
            .recv()
            .map_err(|e| anyhow::anyhow!("failed to receive from server: {}", e))
    }

    fn try_recv(&self) -> Option<ServerMsg> {
        self.from_server.try_recv().ok()
    }

    fn iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.iter().map(|msg| msg))
    }

    fn try_iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_> {
        Box::new(self.from_server.try_iter().map(|msg| msg))
    }
}
