//! keyrock-case-study — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.
//!
//! Three tasks run concurrently from here: the Binance feed
//! (`feed::run_feed::<Binance>` — the generic driver loop shared by every
//! `Exchange` implementation lives in `src/feed.rs`, not here; this file
//! only constructs `Binance` and spawns it), the aggregator task
//! (`aggregator::run`, step 3 — owns per-venue book state, calls
//! `merge::summarise`, publishes into the watch channel), and the gRPC
//! server (`server::router(rx).serve(addr)`). See the `select!` at the
//! bottom of [`main`] for why all three are supervised together rather than
//! run sequentially or fire-and-forgotten. Bitstamp isn't spawned yet — see
//! `specs/006-bitstamp/plan.md` Phase 4.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use keyrock_case_study::exchange::Venue;
use keyrock_case_study::exchange::binance::Binance;
use keyrock_case_study::model::Book;
use keyrock_case_study::{aggregator, feed, server};
use keyrock_case_study::{config::Config, telemetry};
use tokio::sync::{mpsc, watch};
use tracing::info;

#[derive(Parser)]
#[command(name = "keyrock-case-study", version, about = "Keyrock case study")]
struct Cli {
    /// Traded pair to aggregate, e.g. "ethbtc". Overrides KEYROCK_PAIR.
    #[arg(long)]
    pair: Option<String>,

    /// Port the service binds to. Overrides KEYROCK_PORT.
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
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

    // rustls 0.23+ no longer picks a `CryptoProvider` implicitly from Cargo
    // features alone — the process must install one before the first TLS
    // handshake, or `connect_async` panics deep inside rustls with an
    // unhelpful message. `tokio-tungstenite`'s `rustls-tls-webpki-roots`
    // feature resolves `ring` as the provider; this just activates it. Must
    // happen once, in `main`, before any task that might dial TLS is spawned.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("no CryptoProvider installed yet — this is the first call");

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .with_context(|| format!("invalid bind address {}:{}", config.host, config.port))?;

    // `None` until the aggregator's first published summary —
    // `server::router`'s stream filters that case out rather than
    // publishing a fabricated empty `Summary`.
    let (tx, rx) = watch::channel(None);

    // Bounded, not unbounded: an unbounded channel would hide backpressure
    // (a stuck aggregator silently growing memory instead of visibly
    // slowing the feed) — see `specs/005-aggregator/spec.md` decision 1. 32
    // gives slack for a brief lag without hiding a genuinely stuck consumer.
    let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);

    let pair = config.pair.clone();
    let feed_handle = tokio::spawn(feed::run_feed(Binance, pair, feed_tx));
    let aggregator_handle = tokio::spawn(aggregator::run(feed_rx, tx));
    let server_handle = tokio::spawn(async move { server::router(rx).serve(addr).await });

    // `select!` over all three `JoinHandle`s, not sequential `.await`s and
    // not detached spawns with dropped handles:
    //   - Sequential `.await`s would never reach the second/third handle
    //     while the first is still running, so a dead gRPC server behind a
    //     live feed (or vice versa) would go unnoticed indefinitely.
    //   - Dropping the handles would detach the tasks; a panic in any one
    //     of them would print nothing and `main` could still return exit 0
    //     with nothing actually running.
    // A server serving a dead feed publishes stale prices under a
    // "still live" appearance, which is worse than the process not running
    // at all — so the moment any one task ends, the whole process ends,
    // propagating that task's error (or exiting cleanly if it ended without
    // one).
    tokio::select! {
        res = feed_handle => match res {
            Ok(Ok(())) => {
                info!("feed task ended");
                Ok(())
            }
            Ok(Err(e)) => Err(e).context("feed task failed"),
            Err(e) => Err(e).context("feed task panicked"),
        },
        res = aggregator_handle => match res {
            Ok(()) => {
                info!("aggregator task ended");
                Ok(())
            }
            Err(e) => Err(e).context("aggregator task panicked"),
        },
        res = server_handle => match res {
            Ok(Ok(())) => {
                info!("server task ended");
                Ok(())
            }
            Ok(Err(e)) => Err(e).context("server task failed"),
            Err(e) => Err(e).context("server task panicked"),
        },
    }
}
