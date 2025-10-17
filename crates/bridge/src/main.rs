// use anyhow::Result;
// use bridge::server::Server;
// use clap::Parser;
// use simulator::MySim;
// use std::thread;
// use std::time::Duration;
// use tracing::info;

// /// Standalone simulator server.
// ///
// /// By default it runs in gRPC mode listening on 127.0.0.1:50051.
// /// Use `--local` to run a local-only in-proc coordinator (no gRPC).
// #[derive(Parser, Debug)]
// #[command(author, version, about, long_about = None)]
// struct Args {
//     /// Run without gRPC networking (local-only mode).
//     #[arg(long, default_value = "false")]
//     local: bool,

//     /// Listen address for gRPC mode (ignored if --local).
//     #[arg(long, default_value = "127.0.0.1:50051")]
//     addr: String,
// }

// #[tokio::main(flavor = "multi_thread", worker_threads = 2)]
// async fn main() -> Result<()> {
//     // Setup tracing subscriber
//     tracing_subscriber::fmt().with_env_filter("info").init();

//     let args = Args::parse();

//     if args.local {
//         info!("[Server] Starting in local mode (without gRPC)");
//         let _server = Server::new(MySim::default());
//         // keep alive forever
//         loop {
//             thread::sleep(Duration::from_secs(3600));
//         }
//     } else {
//         let addr = args.addr;
//         info!("[Server] Starting with gRPC on {addr}");
//         let _server = Server::new_with_grpc(&addr, MySim::default());
//         // keep alive forever
//         loop {
//             thread::sleep(Duration::from_secs(3600));
//         }
//     }
// }

// use bridge::client::Client;
// use bridge::remote::{RemoteClient, safe_block_on};
// use bridge::server::Server;
// use bridge::service::ForwarderSvc;
// use interface::{ClientMsg, ClientMsgBody, ParamUpdate};
// use std::net::SocketAddr;
// use std::sync::Arc;
// use std::thread;

// fn main() -> anyhow::Result<()> {
//     // -------------------------
//     // 1️⃣ Start the synchronous server core
//     // -------------------------
//     let server = Server::new();
//     //server.clone().start(); // Launch app logic (echo uppercase)

//     // -------------------------
//     // 2️⃣ Start the gRPC bridge on another thread
//     // -------------------------
//     let svc = ForwarderSvc::new(server.clone());
//     let addr: SocketAddr = "127.0.0.1:50051".parse().unwrap();

//     thread::spawn(move || {
//         println!("Starting gRPC service on {addr}");
//         // Run tonic service (async)
//         safe_block_on(async move {
//             svc.run(addr).await;
//         });
//     });

//     // Give the server a moment to start
//     std::thread::sleep(std::time::Duration::from_millis(200));

//     // -------------------------
//     // 3️⃣ Create remote clients (crossbeam + forwarding thread)
//     // -------------------------
//     let client_a = RemoteClient::connect("http://127.0.0.1:50051")?;
//     let client_b = RemoteClient::connect("http://127.0.0.1:50051")?;

//     // -------------------------
//     // 4️⃣ Send messages from each remote client
//     // -------------------------
//     client_a.send(ClientMsgBody::ParamUpdate(ParamUpdate {
//         key: "wind".to_string(),
//         f32_value: 72.0,
//     }));
//     client_b.send(ClientMsgBody::ParamUpdate(ParamUpdate {
//         key: "wind_2".to_string(),
//         f32_value: 73.0,
//     }));

//     // -------------------------
//     // 5️⃣ Receive responses from the server
//     // -------------------------
//     let resp_a = client_a.recv()?;
//     let resp_b = client_b.recv()?;

//     println!("Client A received: {:?}", resp_a.body);
//     println!("Client B received: {:?}", resp_b.body);

//     Ok(())
// }

use std::sync::Arc;

use bridge::{client::Client, local::LocalClient, server::Server};
use interface::{ClientMsgBody, ParamUpdate};

fn main() -> anyhow::Result<()> {
    // -------------------------
    // 1️⃣ Create and start the synchronous Server
    // -------------------------
    let server = Server::new();

    // Start the server’s logic loop — here it just echoes uppercase payloads.
    //server.clone().start();

    // -------------------------
    // 2️⃣ Create a LocalClient directly connected to the Server
    // -------------------------
    let client = LocalClient::new(server.clone(), 0);

    // -------------------------
    // 3️⃣ Send a message to the Server
    // -------------------------
    client.send(ClientMsgBody::ParamUpdate(ParamUpdate {
        key: "wind".to_string(),
        f32_value: 72.0,
    }));

    println!("Receiving...");
    // -------------------------
    // 4️⃣ Receive the response
    // -------------------------
    let resp = client.try_recv();

    if let Some(r) = resp {
        println!("Local client received: {:?}", r.body.unwrap());
    } else {
        println!("Got nothing...");
    }

    Ok(())
}
