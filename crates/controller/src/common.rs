use std::thread;
use std::time::Duration;

use anyhow::Result;
use bridge::client::Client;

pub fn spawn_controller_thread<C: Client>(
    cycles: usize,
    work_ms: u64,
    sim: C,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || -> Result<()> {
        for _ in 0..cycles {
            let tick = sim.step_ready()?;

            println!("[Client: {}] doing work: {}", sim.id(), tick.seq);

            std::thread::sleep(Duration::from_millis(work_ms));
        }

        Ok(())
    })
}
