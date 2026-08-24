//! The generic feed driver loop: connect, optionally subscribe, read frames,
//! parse into a [`Book`], send it down `tx` to the aggregator task. One loop
//! shared by every [`Exchange`] implementation — the loop never branches on
//! which venue it's driving; only `Exchange`'s four methods vary per venue.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls, connect_async};
use tracing::{debug, info, warn};

use crate::exchange::{Exchange, Venue};
use crate::model::Book;
use crate::proxy::{self, parse_proxy_addr};

/// Connects to `exchange` for `pair`, reads frames, and sends each parsed
/// [`Book`] down `tx`. Structurally identical to what `005-aggregator`'s
/// `src/main.rs::run_feed` did by hand for Binance alone — this is that
/// same loop, generalised over any `Exchange` implementation via `E`.
pub async fn run_feed<E: Exchange>(
    exchange: E,
    pair: String,
    tx: mpsc::Sender<(Venue, Book)>,
) -> Result<()> {
    let url = exchange.connect_url(&pair);
    let (mut ws, _response) = match proxy_addr() {
        Some((proxy_host, proxy_port)) => {
            info!(
                proxy = %format!("{proxy_host}:{proxy_port}"),
                venue = %exchange.venue(),
                "connecting via HTTP CONNECT proxy"
            );
            let (target_host, target_port) = parse_connect_target(&url);
            let tunnel = proxy::connect_through_proxy(
                &proxy_host,
                proxy_port,
                &target_host,
                target_port,
            )
            .await
            .with_context(|| {
                format!("failed to establish CONNECT tunnel to {target_host}:{target_port} through proxy")
            })?;
            client_async_tls(&url, tunnel).await.with_context(|| {
                format!(
                    "failed to connect to {} at {url} via proxy",
                    exchange.venue()
                )
            })?
        }
        None => connect_async(&url)
            .await
            .with_context(|| format!("failed to connect to {} at {url}", exchange.venue()))?,
    };
    info!(url = %url, venue = %exchange.venue(), "connected");

    if let Some(msg) = exchange.subscribe_message(&pair) {
        ws.send(Message::Text(msg.into())).await?;
    }

    // Single `next()` loop over the bidirectional stream — no `.split()`.
    // The subscribe write above happens once, before this loop starts, so
    // read and write never overlap; `split()` would cost a mutex on the
    // shared socket for nothing. It would matter if we sent periodic pings
    // ourselves or subscribed dynamically at runtime — we don't;
    // `tungstenite` already answers pings automatically.
    while let Some(message) = ws.next().await {
        let message = message.context("websocket read failed")?;
        match message {
            Message::Text(text) => {
                if let Some(book) = exchange.parse(&text) {
                    if let (Some((bid_price, bid_amount)), Some((ask_price, ask_amount))) =
                        (book.bids.first(), book.asks.first())
                    {
                        // debug, not info: Binance alone pushes ~10/s, and this
                        // is per-tick diagnostic detail, not a state-change
                        // event — at info it floods `docker compose up`'s
                        // interleaved stdout and corrupts the client's
                        // in-place redraw. `RUST_LOG=debug` still shows it.
                        debug!(
                            "{} {} | bid {} x {} | ask {} x {} | id {}",
                            exchange.venue(),
                            pair,
                            bid_price,
                            bid_amount,
                            ask_price,
                            ask_amount,
                            book.last_update_id
                        );
                    }
                    // A bounded `.send(...).await` naturally backpressures
                    // the feed if the aggregator falls behind — see
                    // `specs/005-aggregator/spec.md` decision 1. A failed
                    // send (aggregator's `Receiver` gone) doesn't kill this
                    // loop: the aggregator ending already ends the whole
                    // process via `select!`, so there's nothing extra to do
                    // here.
                    let _ = tx.send((exchange.venue(), book)).await;
                }
            }
            // `tokio-tungstenite` answers pings automatically; nothing to do
            // for either side of the ping/pong exchange here.
            Message::Ping(_) | Message::Pong(_) => {}
            // No reconnection in this step (that's step 6) — a close frame
            // simply ends the read loop and the process exits.
            Message::Close(_) => {
                info!(venue = %exchange.venue(), "connection closed");
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

/// Parses `(target_host, target_port)` out of a `connect_url()`-shaped
/// string (`wss://host[:port]/path`) for the proxy CONNECT tunnel. The
/// `Exchange` trait only exposes the full URL, not host/port separately —
/// see `specs/006-bitstamp/plan.md`, "Plan Review Notes" for why this lives
/// here instead of on the trait.
//
// A deliberate trade: the trait exposes a URL, so host and port are
// re-derived from it here rather than adding a fifth `connect_target()`
// method. Keeps the trait at four methods; costs one small parse of a
// string we produced.
fn parse_connect_target(url: &str) -> (String, u16) {
    let without_scheme = url.strip_prefix("wss://").unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(443)),
        None => (authority.to_string(), 443),
    }
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
