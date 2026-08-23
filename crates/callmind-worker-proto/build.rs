use tonic_prost_build::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/worker.proto");

    // Point prost at a vendored protoc rather than exporting PROTOC: building
    // this workspace should need nothing installed on the host, and setting an
    // environment variable is `unsafe` in edition 2024.
    let mut config = Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);

    tonic_prost_build::configure().compile_with_config(
        config,
        &["proto/worker.proto"],
        &["proto"],
    )?;
    Ok(())
}
