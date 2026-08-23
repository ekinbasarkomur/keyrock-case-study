//! Compiles `proto/orderbook.proto` into Rust at build time. Runs on every
//! `cargo build`, including the Docker dependency-cache stub build — see the
//! Dockerfile's stub stage, which must have `proto/` present before its
//! first `cargo build --release`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Reflection (added in spec 004) needs the compiled schema as raw bytes at
    // runtime, not just generated Rust types — this writes that binary
    // descriptor set alongside the generated code, under OUT_DIR like
    // everything else build.rs produces.
    let out_dir = std::env::var("OUT_DIR")?;
    let descriptor_path = std::path::PathBuf::from(out_dir).join("orderbook_descriptor.bin");
    tonic_prost_build::configure()
        .file_descriptor_set_path(descriptor_path)
        .compile_protos(&["proto/orderbook.proto"], &["proto"])?;
    Ok(())
}
