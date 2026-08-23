//! keyrock-case-study — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.
//!
//! Three tasks run concurrently from here: the Binance feed loop (unchanged
//! in behavior from step 1, just moved into [`run_feed`] so it can be
//! spawned), the fake-data writer (`server::run_fake_writer`), and the gRPC
//! server (`server::router(rx).serve(addr)`). See the `select!` at the
//! bottom of [`main`] for why all three are supervised together rather than
//! run sequentially or fire-and-forgotten.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::StreamExt;
use keyrock_case_study::exchange::binance;
use keyrock_case_study::proxy::{self, parse_proxy_addr};
use keyrock_case_study::server;
use keyrock_case_study::{config::Config, telemetry};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls, connect_async};
use tracing::{debug, info, warn};

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

    // `None` until the fake writer's (and, from step 3 on, the real
    // aggregator's) first tick — `server::router`'s stream filters that
    // case out rather than publishing a fabricated empty `Summary`.
    let (tx, rx) = watch::channel(None);

    let pair = config.pair.clone();
    let feed_handle = tokio::spawn(async move { run_feed(pair).await });
    let fake_writer_handle = tokio::spawn(server::run_fake_writer(tx));
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
        res = fake_writer_handle => match res {
            Ok(()) => {
                info!("fake writer task ended");
                Ok(())
            }
            Err(e) => Err(e).context("fake writer task panicked"),
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

/// The Binance feed loop — connect, read frames, log the top of book.
/// Structurally identical to what step 1 ran directly in `main`'s own task;
/// moved into its own function only so it can be `tokio::spawn`ed alongside
/// the fake writer and the gRPC server.
async fn run_feed(pair: String) -> Result<()> {
    let url = binance::connect_url(&pair);
    let (mut ws, _response) = match proxy_addr() {
        Some((proxy_host, proxy_port)) => {
            info!(proxy = %format!("{proxy_host}:{proxy_port}"), "connecting to binance via HTTP CONNECT proxy");
            let tunnel =
                proxy::connect_through_proxy(&proxy_host, proxy_port, binance::HOST, binance::PORT)
                    .await
                    .context("failed to establish CONNECT tunnel to binance through proxy")?;
            client_async_tls(&url, tunnel)
                .await
                .with_context(|| format!("failed to connect to Binance at {url} via proxy"))?
        }
        None => connect_async(&url)
            .await
            .with_context(|| format!("failed to connect to Binance at {url}"))?,
    };
    info!(url = %url, "connected to binance");

    // Single `next()` loop over the bidirectional stream — no `.split()`.
    // This step never writes anything itself beyond what `tokio-tungstenite`
    // answers automatically (pongs), so one task reading is sufficient.
    while let Some(message) = ws.next().await {
        let message = message.context("websocket read failed")?;
        match message {
            Message::Text(text) => {
                if let Some(book) = binance::parse(&text)
                    && let (Some((bid_price, bid_amount)), Some((ask_price, ask_amount))) =
                        (book.bids.first(), book.asks.first())
                {
                    info!(
                        "binance {} | bid {} x {} | ask {} x {} | id {}",
                        pair, bid_price, bid_amount, ask_price, ask_amount, book.last_update_id
                    );
                }
            }
            // `tokio-tungstenite` answers pings automatically; nothing to do
            // for either side of the ping/pong exchange here.
            Message::Ping(_) | Message::Pong(_) => {}
            // No reconnection in this step (that's step 6) — a close frame
            // simply ends the read loop and the process exits.
            Message::Close(_) => {
                info!("binance closed the connection");
                break;
            }
            Message::Binary(_) => {
                debug!("ignoring unexpected binary message");
            }
            // Structurally required for the match to be exhaustive —
            // `tungstenite::Message` has no `#[non_exhaustive]`, even though
            // `.next()` on a client-API stream never actually produces this
            // variant (it's only reachable via the raw-frame API this code
            // doesn't use).
            Message::Frame(_) => {
                debug!("ignoring raw frame message");
            }
        }
    }

    Ok(())
}

/// Reads `HTTPS_PROXY` (preferred) or `HTTP_PROXY` and parses it into a
/// `(host, port)` pair, e.g. `"http://100.64.x.x:3128"` -> `("100.64.x.x",
/// 3128)`. Returns `None` if neither is set, if the value is empty (how
/// `compose.yml` represents "`PROXY_HOST`/`PROXY_PORT` weren't given" —
/// an unset env var and a blank one both mean the same thing here, so
/// there's nothing proxy-specific to special-case), or if the value is set
/// but doesn't parse — a malformed proxy env var is a reason to log a
/// warning and connect directly, not to crash a binary that would
/// otherwise run fine (see `specs/002-binance-feed/revisions.md`, entry 2).
fn proxy_addr() -> Option<(String, u16)> {
    let raw = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .ok()
        .filter(|raw| !raw.is_empty())?;
    match parse_proxy_addr(&raw) {
        Some(addr) => Some(addr),
        None => {
            warn!(value = %raw, "HTTPS_PROXY/HTTP_PROXY is set but not a parseable host:port — connecting directly");
            None
        }
    }
}
