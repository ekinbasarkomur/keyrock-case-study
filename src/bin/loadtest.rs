//! Load test: opens `--clients` gRPC connections against a running
//! `--addr` server, subscribes each to BookSummary, counts arrivals, and
//! after `--duration-secs` prints the aggregate receive rate.
//!
//! CPU isn't sampled here — `docker stats` externally is simpler and needs
//! no extra dependency.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use keyrock_case_study::orderbook::Empty;
use keyrock_case_study::orderbook::orderbook_aggregator_client::OrderbookAggregatorClient;
use tokio::task::JoinSet;
use tokio_stream::StreamExt;

#[derive(Parser)]
#[command(
    name = "loadtest",
    version,
    about = "keyrock-case-study load test — opens N gRPC subscribers against a running server"
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

    // Relaxed is enough — only the final sum after all tasks stop matters.
    let total_received = Arc::new(AtomicU64::new(0));

    // Stagger connects by a few ms each — firing all at once reliably
    // tripped connection resets at 500 clients, measuring the connect-path
    // stampede rather than sustained load.
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

    // Every client loops forever on its stream, so aborting the set is how
    // this binary stops.
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

/// Connects one client, subscribes to BookSummary, and increments
/// `total_received` for every message until the stream ends or aborts.
async fn subscribe_and_count(addr: &str, total_received: &AtomicU64) -> Result<()> {
    let mut client = OrderbookAggregatorClient::connect(addr.to_string()).await?;
    let mut stream = client.book_summary(Empty {}).await?.into_inner();

    while let Some(item) = stream.next().await {
        item?;
        total_received.fetch_add(1, Ordering::Relaxed);
    }

    Ok(())
}
