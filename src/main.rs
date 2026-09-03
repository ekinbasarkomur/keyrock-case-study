//! rust-crypto-orderbook — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.
//!
//! This step's read loop is driven directly here, in the `main` task — no
//! `tokio::spawn`, no `.split()` on the websocket stream. A single feed, a
//! single task, is the entire concurrency story until step 3/4 add a second
//! venue (see `specs/002-binance-feed/spec.md`).

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::StreamExt;
use rust_crypto_orderbook::exchange::binance;
use rust_crypto_orderbook::proxy::{self, parse_proxy_addr};
use rust_crypto_orderbook::{config::Config, telemetry};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls, connect_async};
use tracing::{debug, info, warn};

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
    // feature resolves `ring` as the provider; this just activates it.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("no CryptoProvider installed yet — this is the first call");

    let url = binance::connect_url(&config.pair);
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

    // Single `next()` loop over the bidirectional stream — no `.split()`,
    // no `tokio::spawn`. This step never writes anything itself beyond what
    // `tokio-tungstenite` answers automatically (pongs), so one task reading
    // is sufficient.
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
                        config.pair,
                        bid_price,
                        bid_amount,
                        ask_price,
                        ask_amount,
                        book.last_update_id
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
