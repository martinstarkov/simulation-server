use anyhow::Result;
use client::client::SyncSimClient;
use crossbeam_channel as xchan;
use interface::ServerMsg;
use std::thread;

pub fn spawn_viewer_worker(sim: SyncSimClient) -> (xchan::Receiver<ServerMsg>, xchan::Sender<()>) {
    let (tx_app, rx_app) = xchan::bounded::<ServerMsg>(1024);
    let (tx_ready, rx_ready) = xchan::bounded::<()>(1);

    thread::spawn(move || -> Result<()> {
        loop {
            if sim.contributing() {
                sim.step_ready()?;

                // Block until Bevy signals "frame done"
                rx_ready.recv().ok();
            }

            sim.forward_until_tick(&tx_app)?;
        }
    });

    (rx_app, tx_ready)
}
