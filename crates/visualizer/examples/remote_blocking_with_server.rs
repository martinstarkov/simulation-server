use visualizer::{run, VisualizerMode};

fn main() {
    // Starts a local gRPC server on 127.0.0.1:50052 and connects to it as a remote contributing viewer.
    run(VisualizerMode::RemoteBlockingWithServer {
        addr: "127.0.0.1:50052".into(),
    })
    .expect("visualizer exited");
}
