use visualizer::{VisualizerMode, run};

fn main() {
    // Starts a local gRPC server on 127.0.0.1:50051 and connects to it as a remote non-contributing viewer.
    run(VisualizerMode::RemoteNonBlockingWithServer {
        addr: format!("127.0.0.1:50051").into(),
    })
    .expect("visualizer exited");
}
