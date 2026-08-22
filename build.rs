//! Compiles `proto/orderbook.proto` into Rust at build time. Runs on every
//! `cargo build`, including the Docker dependency-cache stub build — see the
//! Dockerfile's stub stage, which must have `proto/` present before its
//! first `cargo build --release`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/orderbook.proto")?;
    Ok(())
}
