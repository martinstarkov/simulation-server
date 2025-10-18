use anyhow::Result;
use interface::{ClientMsgBody, ServerMsg};

pub trait Client: Send + Sync {
    fn send(&self, msg: ClientMsgBody);
    fn recv(&self) -> Result<ServerMsg>;
    fn try_recv(&self) -> Option<ServerMsg>;

    /// Returns a blocking iterator over incoming messages.
    /// Ends when the server disconnects.
    fn iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_>;

    /// Returns a non-blocking iterator over currently available messages.
    fn try_iter(&self) -> Box<dyn Iterator<Item = ServerMsg> + '_>;
}
