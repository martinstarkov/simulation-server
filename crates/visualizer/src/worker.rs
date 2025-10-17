use anyhow::Result;
use bridge::client::Client;
use crossbeam_channel as xchan;
use interface::ServerMsg;
use std::thread;

pub fn spawn_viewer_worker(sim: Client) -> (xchan::Receiver<ServerMsg>, xchan::Sender<()>) {
    let (tx_app, rx_app) = xchan::bounded::<ServerMsg>(1024);
    let (tx_ready, rx_ready) = xchan::bounded::<()>(1);

    thread::spawn(move || -> Result<()> {
        loop {
            if sim.contributing() {
                let _ = sim.step_ready()?;

                // Block until Bevy signals "frame done"
                rx_ready.recv().ok();
            }

            sim.forward_all(&tx_app)?;
        }
    });

    (rx_app, tx_ready)
}
