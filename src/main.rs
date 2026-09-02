//! keyrock-case-study — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate.
//! Everything testable lives in the library crate.
//!
//! Four tasks run concurrently: the Binance, Bitstamp, and Kraken feeds
//! (`feed::run_feed`, one generic driver loop shared by every venue), the
//! aggregator task, and the gRPC server. Supervised together via `JoinSet`
//! at the bottom of `main` — see that loop's comment for why.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use keyrock_case_study::exchange::Venue;
use keyrock_case_study::exchange::binance::Binance;
use keyrock_case_study::exchange::bitstamp::Bitstamp;
// Research-only (012-kraken, not merged into main) — see
// specs/012-kraken/spec.md.
use keyrock_case_study::exchange::kraken::Kraken;
use keyrock_case_study::model::Book;
use keyrock_case_study::{aggregator, feed, server};
use keyrock_case_study::{config::Config, telemetry};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

/// Which supervised task a `TaskResult` came from. `JoinSet` doesn't tell
/// you which task ended, so the identity travels with the result.
#[derive(Debug, Clone, Copy)]
enum Component {
    Feed(Venue),
    Aggregator,
    Server,
}

/// Normalized outcome every spawned task produces, so `join_next()` can be
/// awaited in one loop regardless of which task finished.
type TaskResult = (Component, Result<()>);

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

    // CLI flags win over env-sourced values when both are given.
    if let Some(pair) = cli.pair {
        config.pair = pair;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    telemetry::init(&config.log_level);

    info!(pair = %config.pair, port = %config.port, "starting");

    // rustls 0.23+ needs an explicit CryptoProvider install before the
    // first TLS handshake, or connect_async panics. Must run once, before
    // any TLS-dialing task is spawned.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("no CryptoProvider installed yet — this is the first call");

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .with_context(|| format!("invalid bind address {}:{}", config.host, config.port))?;

    // None until the aggregator's first published summary.
    let (tx, rx) = watch::channel(None);

    // Bounded, not unbounded — an unbounded channel would hide backpressure
    // instead of visibly slowing the feed. 32 gives slack for a brief lag.
    let (feed_tx, feed_rx) = mpsc::channel::<(Venue, Book)>(32);

    let pair = config.pair.clone();
    let binance_tx = feed_tx.clone();
    let bitstamp_tx = feed_tx.clone();
    let kraken_tx = feed_tx;

    // Each task tags its outcome with a Component before handing it to
    // the JoinSet, since JoinSet alone doesn't say which task finished.
    let mut tasks: JoinSet<TaskResult> = JoinSet::new();
    let binance_pair = pair.clone();
    let bitstamp_pair = pair.clone();
    let aggregator_pair = pair.clone();
    tasks.spawn(async move {
        let res = feed::run_feed(Binance, binance_pair, binance_tx).await;
        (Component::Feed(Venue::Binance), res)
    });
    tasks.spawn(async move {
        let res = feed::run_feed(Bitstamp, bitstamp_pair, bitstamp_tx).await;
        (Component::Feed(Venue::Bitstamp), res)
    });
    tasks.spawn(async move {
        let res = feed::run_feed(Kraken::new(), pair, kraken_tx).await;
        (Component::Feed(Venue::Kraken), res)
    });
    tasks.spawn(async move {
        aggregator::run(feed_rx, tx, aggregator_pair).await;
        (Component::Aggregator, Ok(()))
    });
    tasks.spawn(async move {
        let res = server::router(rx)
            .serve(addr)
            .await
            .context("server task failed");
        (Component::Server, res)
    });

    // Await the JoinSet, not sequential awaits or detached spawns — the
    // moment any one task ends, the whole process ends. A server serving a
    // dead feed publishing stale prices is worse than not running at all.
    match tasks.join_next().await {
        Some(Ok((component, Ok(())))) => {
            match component {
                Component::Feed(venue) => info!(?venue, "feed task ended"),
                Component::Aggregator => info!("aggregator task ended"),
                Component::Server => info!("server task ended"),
            }
            Ok(())
        }
        Some(Ok((component, Err(e)))) => match component {
            Component::Feed(venue) => Err(e).with_context(|| format!("{venue:?} feed task failed")),
            Component::Aggregator => Err(e).context("aggregator task failed"),
            Component::Server => Err(e).context("server task failed"),
        },
        Some(Err(join_err)) if join_err.is_panic() => {
            error!(error = %join_err, "task panicked");
            Err(join_err).context("task panicked")
        }
        Some(Err(join_err)) => {
            warn!(error = %join_err, "task was cancelled");
            Err(join_err).context("task was cancelled")
        }
        None => {
            // Unreachable — tasks were just spawned above — but join_next()
            // returns Option, so handle it rather than unwrap().
            info!("task set was empty");
            Ok(())
        }
    }
}
