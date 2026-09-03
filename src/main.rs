//! rust-crypto-orderbook — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.

use anyhow::Result;
use clap::Parser;
use rust_crypto_orderbook::{config::Config, telemetry};
use tracing::info;

#[derive(Parser)]
#[command(name = "rust-crypto-orderbook", version, about = "Rust order book aggregator")]
struct Cli {
    /// Traded pair to aggregate, e.g. "ethbtc". Overrides ORDERBOOK_PAIR.
    #[arg(long)]
    pair: Option<String>,

    /// Port the service binds to. Overrides ORDERBOOK_PORT.
    #[arg(long)]
    port: Option<u16>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = Config::from_env()?;

    // CLI flags are the more specific, closer-to-the-call-site input, so
    // they win over the env-sourced value when both are given.
    if let Some(pair) = cli.pair {
        config.pair = pair;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    telemetry::init(&config.log_level);

    info!(pair = %config.pair, port = %config.port, "starting");

    Ok(())
}
