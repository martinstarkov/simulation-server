fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("protoc not found");
    unsafe {
        std::env::set_var("PROTOC", &protoc);
    }

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("gen")
        .file_descriptor_set_path("gen/interface_descriptor.bin")
        .compile_protos(&["proto/interface.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/interface.proto");
    Ok(())
}
