mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn local_client_can_shutdown_server() -> Result<()> {
    init_tracing();

    // Start server (local-only, no gRPC)
    let (server, sim_handle) = Server::new_local(TestSim::default());

    // Create two local clients
    let client_a = Client::new_local(true, &server)?;
    let client_b = Client::new_local(true, &server)?;

    // Spawn one controller thread
    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            for i in 0..5 {
                let tick = match client_a.step_ready() {
                    Ok(t) => t,
                    Err(_) => break,
                };

                println!("[client A] tick {}", tick.seq);

                if i == 2 {
                    println!("[client A] sending shutdown_server()");
                    client_a.shutdown_server()?; // 🔹 this should stop the server
                    break;
                }

                thread::sleep(Duration::from_millis(50));
            }
            println!("[client A] exiting cleanly");
            Ok(())
        }
    });

    // Second controller runs normally until the server shuts down
    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            while let Ok(tick) = client_b.step_ready() {
                println!("[client B] tick {}", tick.seq);
            }
            println!("[client B] disconnected (server shutdown)");
            Ok(())
        }
    });

    // Wait for all to finish
    h1.join().unwrap().unwrap();
    h2.join().unwrap().unwrap();
    sim_handle.join().unwrap()?;

    println!("[test] local shutdown finished cleanly");
    Ok(())
}

#[test]
fn server_initiated_shutdown_stops_clients() -> Result<()> {
    init_tracing();

    let (server, sim_handle) = Server::new_local(TestSim::default());
    let client_a = Client::new_local(true, &server)?;
    let client_b = Client::new_local(true, &server)?;

    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            while let Ok(tick) = client_a.step_ready() {
                println!("[client A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(30));
            }
            println!("[client A] exited cleanly (server shutdown)");
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            while let Ok(tick) = client_b.step_ready() {
                println!("[client B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(40));
            }
            println!("[client B] exited cleanly (server shutdown)");
            Ok(())
        }
    });

    // Let the simulation run for a bit
    thread::sleep(Duration::from_millis(300));
    println!("[test] server initiating shutdown");
    server.shutdown();

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    sim_handle.join().unwrap()?;

    println!("[test] server_initiated_shutdown_stops_clients finished cleanly");
    Ok(())
}
