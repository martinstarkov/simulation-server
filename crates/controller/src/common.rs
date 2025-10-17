use std::thread;
use std::time::Duration;

use anyhow::Result;
use client::client::SyncSimClient;

pub fn spawn_controller_thread(
    cycles: usize,
    work_ms: u64,
    sim: SyncSimClient,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || -> Result<()> {
        for _ in 0..cycles {
            sim.step_ready()?;

            let tick = sim.wait_for_tick()?;

            println!("[Client: {}] doing work: {}", sim.id(), tick.seq);

            std::thread::sleep(Duration::from_millis(work_ms));
        }

        Ok(())
    })
}
