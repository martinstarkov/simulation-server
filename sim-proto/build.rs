fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use vendored protoc so PATH/PROTOC isn't needed
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc not found");
    std::env::set_var("PROTOC", &protoc);

    // Write generated files to a stable location checked into target crate
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/gen")
        .compile_protos(&["../proto/simulator.proto"], &["../proto"])?;
    println!("cargo:rerun-if-changed=../proto/simulator.proto");
    Ok(())
}
