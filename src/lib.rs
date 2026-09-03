//! rust-crypto-orderbook — library crate.
//!
//! Everything testable lives here. `src/main.rs` is a thin shell that parses
//! arguments and calls into this crate — logic in main.rs isn't reachable
//! from tests/.

pub mod aggregator;
pub mod config;
pub mod exchange;
pub mod feed;
pub mod merge;
pub mod model;
pub mod proxy;
pub mod server;
pub mod telemetry;

/// Generated from `proto/orderbook.proto`.
pub mod orderbook {
    tonic::include_proto!("orderbook");
}
