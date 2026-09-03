//! rust-crypto-orderbook — library crate.
//!
//! Everything testable lives here. `src/main.rs` is a thin shell that parses
//! arguments and calls into this crate.
//!
//! WHY THE SPLIT: an integration test in `tests/` can only `use` a library
//! crate. Logic written directly in `main.rs` is reachable from unit tests in
//! that file and from nowhere else — so it silently stops being covered the
//! moment the test suite grows past one file.

pub mod config;
pub mod exchange;
pub mod model;
pub mod proxy;
pub mod telemetry;

/// Generated from `proto/orderbook.proto`. Nothing consumes these types
/// until the gRPC server lands; including them here means the generated
/// code is actually compiled, so the build pipeline is verified rather
/// than assumed.
pub mod orderbook {
    tonic::include_proto!("orderbook");
}
