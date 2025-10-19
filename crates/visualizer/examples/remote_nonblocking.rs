use visualizer::{run, VisualizerMode};

fn main() {
    run(VisualizerMode::RemoteNonBlocking {
        addr: "http://127.0.0.1:50051".into(),
    })
    .expect("visualizer exited");
}
