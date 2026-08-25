//! The generic feed driver loop: connect, optionally subscribe, read frames,
//! parse into a [`Book`], send it down `tx` to the aggregator task. One loop
//! shared by every [`Exchange`] implementation — the loop never branches on
//! which venue it's driving; only `Exchange`'s four methods vary per venue.
//!
//! As of step 7, `run_feed` never returns on a closed or failed connection —
//! it reconnects forever with jittered exponential backoff (see [`Backoff`]).
//! A feed ending is no longer a fatal event for the process (that's
//! `src/main.rs`'s job to decide); this file's only job is to keep trying.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async_tls, connect_async};
use tracing::{debug, info, warn};

use crate::exchange::{Exchange, Venue};
use crate::model::Book;
use crate::proxy::{self, parse_proxy_addr};

/// A connection is considered "stable" once it's been held open longer than
/// this — see [`Backoff::reset_if_stable`]'s doc comment for why the reset is
/// gated on this instead of on `connect()` merely succeeding.
const STABILITY_WINDOW: Duration = Duration::from_secs(30);

/// Jittered exponential backoff: 1s, 2s, 4s, 8s, 16s, then capped at 30s.
///
/// Deliberately takes the "current instant" as a parameter on
/// [`Backoff::reset_if_stable`] rather than calling `Instant::now()`
/// internally — this is what lets the stability-reset tests below supply a
/// fixed/fake clock instead of needing `tokio::time::pause`.
struct Backoff {
    attempt: u32,
}

impl Backoff {
    fn new() -> Self {
        Backoff { attempt: 0 }
    }

    /// The nominal (pre-jitter) delay for the *current* attempt, then
    /// advances to the next attempt. 2^attempt seconds, capped at 30s:
    /// attempt 0 -> 1s, 1 -> 2s, 2 -> 4s, 3 -> 8s, 4 -> 16s, 5+ -> 30s.
    fn next_delay(&mut self) -> Duration {
        let secs = 1u64.checked_shl(self.attempt).unwrap_or(u64::MAX);
        let capped = secs.min(30);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs(capped)
    }

    /// Jitters `delay` by a multiplier drawn uniformly from `0.5x`-`1.5x`.
    /// Without this, every client disconnected by the same outage would
    /// retry on the identical schedule and hammer the exchange back in
    /// lockstep the moment it recovers — see spec.md Piece 2.
    fn jittered(delay: Duration) -> Duration {
        let multiplier: f64 = rand::random_range(0.5..1.5);
        delay.mul_f64(multiplier)
    }

    /// THE subtle piece (spec.md Piece 3). Resetting the backoff the moment
    /// `connect()` succeeds looks correct and is not: a venue that accepts a
    /// connection and then drops it immediately (overloaded, half-banned, or
    /// a stream that never yields data) turns into
    /// `connect ok -> reset -> wait 1s -> connect ok -> drop -> reset -> ...`
    /// — one attempt per second, which is exactly Binance's rate-limit
    /// boundary (300 attempts / 5 minutes), and the backoff never actually
    /// engages because it's reset every single cycle.
    ///
    /// The fix: only reset once the connection has been held open longer
    /// than `STABILITY_WINDOW`, not merely established. `now` is passed in
    /// rather than read via `Instant::now()` so this can be unit tested with
    /// a fixed clock.
    fn reset_if_stable(&mut self, connected_at: Instant, now: Instant) {
        if now.duration_since(connected_at) > STABILITY_WINDOW {
            self.attempt = 0;
        }
    }
}

/// Connects to `exchange` for `pair`, reads frames, and sends each parsed
/// [`Book`] down `tx`. Structurally identical to what `005-aggregator`'s
/// `src/main.rs::run_feed` did by hand for Binance alone — this is that
/// same loop, generalised over any `Exchange` implementation via `E`.
///
/// As of step 7, this never returns except on panic: a closed socket or a
/// failed connect attempt is followed by a jittered backoff wait, then
/// another attempt, forever. See [`Backoff`] for the wait sequence and the
/// stability-gated reset.
pub async fn run_feed<E: Exchange>(
    exchange: E,
    pair: String,
    tx: mpsc::Sender<(Venue, Book)>,
) -> Result<()> {
    let mut backoff = Backoff::new();

    loop {
        let (connected_at, result) = run_once(&exchange, &pair, &tx).await;
        match &result {
            Ok(()) => {
                info!(venue = %exchange.venue(), "feed loop ended cleanly, reconnecting");
            }
            Err(e) => {
                warn!(venue = %exchange.venue(), error = %e, "feed connection failed");
            }
        }

        // Gated on how long the connection was actually held, not on
        // `connect()` having succeeded at all — see `Backoff::reset_if_stable`.
        // `connected_at` is `None` when the connect attempt itself failed
        // (never established), in which case there's nothing to judge for
        // stability and the backoff simply keeps advancing.
        if let Some(connected_at) = connected_at {
            backoff.reset_if_stable(connected_at, Instant::now());
        }

        let wait = Backoff::jittered(backoff.next_delay());
        info!(venue = %exchange.venue(), wait_secs = wait.as_secs_f64(), "reconnecting after backoff");
        tokio::time::sleep(wait).await;
    }
}

/// One connect-subscribe-read cycle. Returns `(connected_at, result)`:
/// `connected_at` is `Some` once the websocket handshake actually completed
/// (regardless of whether the cycle then succeeded or failed), `None` if the
/// connect attempt itself never got that far — `run_feed`'s stability-gated
/// reset needs to know which happened, not just whether the cycle overall
/// was `Ok`.
async fn run_once<E: Exchange>(
    exchange: &E,
    pair: &str,
    tx: &mpsc::Sender<(Venue, Book)>,
) -> (Option<Instant>, Result<()>) {
    match run_once_inner(exchange, pair, tx).await {
        Ok(connected_at) => (Some(connected_at), Ok(())),
        Err((connected_at, e)) => (connected_at, Err(e)),
    }
}

/// The actual connect-subscribe-read body. Returns `Ok(connected_at)` when
/// the socket closes normally, or `Err((connected_at, error))` — connected_at
/// is `Some` if the handshake had completed before the failure, `None` if
/// the failure happened during the connect attempt itself.
async fn run_once_inner<E: Exchange>(
    exchange: &E,
    pair: &str,
    tx: &mpsc::Sender<(Venue, Book)>,
) -> std::result::Result<Instant, (Option<Instant>, anyhow::Error)> {
    let url = exchange.connect_url(pair);
    let connect_result = match proxy_addr() {
        Some((proxy_host, proxy_port)) => {
            info!(
                proxy = %format!("{proxy_host}:{proxy_port}"),
                venue = %exchange.venue(),
                "connecting via HTTP CONNECT proxy"
            );
            (async {
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
                })
            })
            .await
        }
        None => connect_async(&url)
            .await
            .with_context(|| format!("failed to connect to {} at {url}", exchange.venue())),
    };
    let (mut ws, _response) = connect_result.map_err(|e| (None, e))?;
    let connected_at = Instant::now();
    info!(url = %url, venue = %exchange.venue(), "connected");

    // Subscribe *inside* the reconnect loop (this function is re-entered on
    // every reconnect attempt) — Bitstamp's subscription is per-connection,
    // so without re-sending it here every reconnect the socket would open,
    // no error would appear, and no data would ever arrive again. Binance's
    // subscription is baked into the URL and returns `None` here, so this is
    // a no-op for it.
    if let Some(msg) = exchange.subscribe_message(pair) {
        ws.send(Message::Text(msg.into()))
            .await
            .map_err(|e| (Some(connected_at), anyhow::Error::from(e)))?;
    }

    // Single `next()` loop over the bidirectional stream — no `.split()`.
    // The subscribe write above happens once, before this loop starts, so
    // read and write never overlap; `split()` would cost a mutex on the
    // shared socket for nothing. It would matter if we sent periodic pings
    // ourselves or subscribed dynamically at runtime — we don't;
    // `tungstenite` already answers pings automatically.
    while let Some(message) = ws.next().await {
        let message = message.map_err(|e| {
            (
                Some(connected_at),
                anyhow::Error::from(e).context("websocket read failed"),
            )
        })?;
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
                    // process via the `JoinSet` supervisor, so there's
                    // nothing extra to do here.
                    let _ = tx.send((exchange.venue(), book)).await;
                }
            }
            // `tokio-tungstenite` answers pings automatically; nothing to do
            // for either side of the ping/pong exchange here.
            Message::Ping(_) | Message::Pong(_) => {}
            // A close frame ends this connect-cycle; `run_feed`'s outer loop
            // backs off and reconnects rather than the process exiting.
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

    Ok(connected_at)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches unbounded growth, or a missing cap: the sequence must be
    /// exactly 1s, 2s, 4s, 8s, 16s, then 30s forever after — never higher.
    #[test]
    fn backoff_grows_and_caps() {
        let mut backoff = Backoff::new();
        let expected_secs = [1, 2, 4, 8, 16, 30, 30, 30, 30];
        for &expected in &expected_secs {
            assert_eq!(backoff.next_delay(), Duration::from_secs(expected));
        }
        // Keep going well past any plausible attempt count in a real run —
        // it must never exceed 30s, ever.
        for _ in 0..1_000 {
            assert!(backoff.next_delay() <= Duration::from_secs(30));
        }
    }

    /// THE single most valuable test in this packet (spec.md Piece 3). If the
    /// reset were gated on `connect()` succeeding rather than on the
    /// connection having proven itself stable, this test would see the
    /// backoff snap back to 1s after every connect-then-drop cycle instead
    /// of continuing to grow — exactly the pattern that would drive one
    /// connection attempt per second into Binance's rate limit.
    #[test]
    fn a_short_lived_connection_does_not_reset_the_thrash_loop() {
        let mut backoff = Backoff::new();
        let mut observed = Vec::new();

        // Simulate five cycles of "connect succeeds, then drops almost
        // immediately" — well under the 30s stability window each time.
        for _ in 0..5 {
            observed.push(backoff.next_delay());

            let now = Instant::now();
            let connected_at = now; // connection held for ~0s before dropping
            let judged_at = connected_at + Duration::from_millis(50); // dropped fast
            backoff.reset_if_stable(connected_at, judged_at);
        }

        // The whole point: each cycle's delay keeps advancing rather than
        // resetting to 1s every time.
        assert_eq!(
            observed,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
            ]
        );
    }

    /// Catches a stability window that effectively never fires — a
    /// connection held well past `STABILITY_WINDOW` must reset the backoff,
    /// so the *next* disconnect starts back at 1s rather than continuing to
    /// climb the curve.
    #[test]
    fn a_stable_connection_does_reset() {
        let mut backoff = Backoff::new();

        // Advance the backoff a few cycles first, as if it had already been
        // struggling to connect.
        backoff.next_delay();
        backoff.next_delay();
        backoff.next_delay();

        // Now simulate a connection that's held well past the stability
        // window before it eventually drops.
        let connected_at = Instant::now();
        let judged_at = connected_at + STABILITY_WINDOW + Duration::from_secs(1);
        backoff.reset_if_stable(connected_at, judged_at);

        // The next delay after the reset must start back at 1s.
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    /// Catches both "jitter not applied at all" (every value equal to the
    /// nominal delay) and "applied but out of range" (a value outside
    /// 0.5x-1.5x) — sampling many times to make either failure mode
    /// vanishingly unlikely to pass by chance.
    #[test]
    fn jitter_stays_within_range() {
        let nominal = Duration::from_secs(10);
        let lower = nominal.mul_f64(0.5);
        let upper = nominal.mul_f64(1.5);

        let mut saw_below_nominal = false;
        let mut saw_above_nominal = false;
        for _ in 0..1_000 {
            let jittered = Backoff::jittered(nominal);
            assert!(
                jittered >= lower && jittered <= upper,
                "jittered delay {jittered:?} outside [{lower:?}, {upper:?}]"
            );
            if jittered < nominal {
                saw_below_nominal = true;
            }
            if jittered > nominal {
                saw_above_nominal = true;
            }
        }
        // Confirms jitter is actually being applied (not a no-op that always
        // returns the nominal value) by seeing variation on both sides of it.
        assert!(
            saw_below_nominal,
            "never saw a jittered delay below nominal"
        );
        assert!(
            saw_above_nominal,
            "never saw a jittered delay above nominal"
        );
    }
}
