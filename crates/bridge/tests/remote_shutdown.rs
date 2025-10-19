mod common;

use anyhow::Result;
use bridge::{client::Client, init_tracing, server::Server};
use common::TestSim;
use std::{thread, time::Duration};

#[test]
fn remote_client_can_shutdown_server() -> Result<()> {
    init_tracing();

    // Start server with gRPC enabled
    let addr = "127.0.0.1:50051";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());

    // Wait a bit for gRPC service to be listening
    thread::sleep(Duration::from_millis(200));

    // Local + Remote clients
    let local_client = Client::new_local(true, &server)?;
    let remote_client = Client::new_remote(true, addr)?;

    // Remote client will trigger shutdown after a few ticks
    let h_remote = thread::spawn({
        let remote_client = remote_client.clone();
        move || -> Result<()> {
            for i in 0..5 {
                let tick = match remote_client.step_ready() {
                    Ok(t) => t,
                    Err(_) => break,
                };
                println!("[remote-client] tick {}", tick.seq);

                if i == 2 {
                    println!("[remote-client] sending shutdown_server()");
                    remote_client.shutdown_server()?; // 🔹 triggers server shutdown
                    break;
                }

                thread::sleep(Duration::from_millis(50));
            }
            println!("[remote-client] exited cleanly");
            Ok(())
        }
    });

    // Local client will observe server disconnect
    let h_local = thread::spawn({
        let local_client = local_client.clone();
        move || -> Result<()> {
            while let Ok(tick) = local_client.step_ready() {
                println!("[local-client] tick {}", tick.seq);
            }
            println!("[local-client] disconnected (server shutdown)");
            Ok(())
        }
    });

    // Join everything
    h_remote.join().unwrap().unwrap();
    h_local.join().unwrap().unwrap();
    sim_handle.join().unwrap()?;

    println!("[test] remote shutdown finished cleanly");
    Ok(())
}

#[test]
fn remote_server_shutdown_drops_clients() -> Result<()> {
    init_tracing();

    let addr = "127.0.0.1:50065";
    let (server, sim_handle) = Server::new_with_grpc(addr, TestSim::default());
    thread::sleep(Duration::from_millis(200));

    let client_a = Client::new_remote(true, addr)?;
    let client_b = Client::new_remote(true, addr)?;

    let h1 = thread::spawn({
        let client_a = client_a.clone();
        move || -> Result<()> {
            while let Ok(tick) = client_a.step_ready() {
                println!("[remote-A] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(50));
            }
            println!("[remote-A] disconnected cleanly");
            Ok(())
        }
    });

    let h2 = thread::spawn({
        let client_b = client_b.clone();
        move || -> Result<()> {
            while let Ok(tick) = client_b.step_ready() {
                println!("[remote-B] tick {}", tick.seq);
                thread::sleep(Duration::from_millis(60));
            }
            println!("[remote-B] disconnected cleanly");
            Ok(())
        }
    });

    // Allow some ticks, then shutdown server
    thread::sleep(Duration::from_secs(1));
    println!("[test] server initiating shutdown");
    server.shutdown();

    h1.join().unwrap()?;
    h2.join().unwrap()?;
    sim_handle.join().unwrap()?;

    println!("[test] remote_server_shutdown_drops_clients finished cleanly");
    Ok(())
}
