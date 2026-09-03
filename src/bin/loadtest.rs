//! rust-crypto-orderbook load test — a small tool, not a benchmarking
//! framework (see `specs/011-measurement/spec.md`'s explicit Out of Scope).
//!
//! Opens `--clients` independent gRPC connections against an
//! already-running `--addr` server, subscribes each to `BookSummary`, and
//! discards every message while counting arrivals. Never starts its own
//! server — the point is to measure a real, independently-running server
//! under real production load, not an in-process shortcut. After
//! `--duration-secs`, prints the aggregate receive rate and exits.
//!
//! CPU is deliberately *not* sampled here: this project already runs the
//! server under Docker, so `docker stats <container>` sampled externally
//! during a run is the simplest accurate source, with no new dependency and
//! no self-profiling code in this binary.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use rust_crypto_orderbook::orderbook::Empty;
use rust_crypto_orderbook::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use tokio::task::JoinSet;
use tokio_stream::StreamExt;

#[derive(Parser)]
#[command(
    name = "loadtest",
    version,
    about = "rust-crypto-orderbook load test — opens N gRPC subscribers against a running server"
)]
struct Cli {
    /// gRPC server address, e.g. http://127.0.0.1:50051.
    #[arg(long, default_value = "http://127.0.0.1:50051")]
    addr: String,

    /// Number of independent subscriber connections to open.
    #[arg(long, default_value_t = 100)]
    clients: usize,

    /// How long to run before reporting and exiting.
    #[arg(long, default_value_t = 60)]
    duration_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Shared across every client task via `Arc` — each task only ever
    // increments its own share of the total, so a plain `Ordering::Relaxed`
    // add is enough (no ordering relationship between clients' counts needs
    // to be observed, only the final sum after every task is stopped).
    let total_received = Arc::new(AtomicU64::new(0));

    // Staggering the connects by a few ms each, rather than firing all
    // `--clients` connection attempts inside the same tokio poll tick,
    // matters in practice: a genuine instantaneous burst of hundreds of TCP
    // connects against one address reliably tripped connection resets (seen
    // live at 500 clients — most connections reset before the stream even
    // opened) with the server's CPU staying near-idle throughout, which
    // means it was measuring how the connect path handles a stampede, not
    // the sustained per-subscriber load Piece 4 is actually after. Real
    // subscribers also don't all dial in on the same tick.
    let mut tasks: JoinSet<()> = JoinSet::new();
    for client_id in 0..cli.clients {
        let addr = cli.addr.clone();
        let total_received = Arc::clone(&total_received);
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(5) * client_id as u32).await;
            if let Err(err) = subscribe_and_count(&addr, &total_received).await {
                eprintln!("client {client_id}: connect/stream failed: {err:#}");
            }
        });
    }

    println!(
        "connected (or attempting to connect) {} clients to {}, running for {}s",
        cli.clients, cli.addr, cli.duration_secs
    );

    tokio::time::sleep(Duration::from_secs(cli.duration_secs)).await;

    // Every client task loops forever on its stream (matching the demo
    // client's own "reconnect forever" shape) — aborting the whole set is
    // how this binary actually stops, not a cooperative shutdown signal
    // threaded through every task.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}

    let total = total_received.load(Ordering::Relaxed);
    let rate = total as f64 / cli.duration_secs as f64;
    println!(
        "{} clients, {}s: {total} messages received, {rate:.2} msg/s aggregate",
        cli.clients, cli.duration_secs
    );

    Ok(())
}

/// Connects one client to `addr`, subscribes to `BookSummary`, and
/// increments `total_received` for every message that arrives until the
/// stream ends or this task is aborted by the caller.
async fn subscribe_and_count(addr: &str, total_received: &AtomicU64) -> Result<()> {
    let mut client = OrderbookAggregatorClient::connect(addr.to_string()).await?;
    let mut stream = client.book_summary(Empty {}).await?.into_inner();

    while let Some(item) = stream.next().await {
        item?;
        total_received.fetch_add(1, Ordering::Relaxed);
    }

    Ok(())
}
