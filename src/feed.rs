//! Generic feed driver loop: connect, optionally subscribe, read frames,
//! parse into a `Book`, send down `tx` to the aggregator. One loop shared
//! by every `Exchange` impl. Reconnects forever with jittered backoff.

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

/// A connection counts as "stable" once held open longer than this.
const STABILITY_WINDOW: Duration = Duration::from_secs(30);

/// Jittered exponential backoff: 1s, 2s, 4s, 8s, 16s, then capped at 30s.
struct Backoff {
    attempt: u32,
}

impl Backoff {
    fn new() -> Self {
        Backoff { attempt: 0 }
    }

    /// 2^attempt seconds, capped at 30s, then advances to the next attempt.
    fn next_delay(&mut self) -> Duration {
        let secs = 1u64.checked_shl(self.attempt).unwrap_or(u64::MAX);
        let capped = secs.min(30);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs(capped)
    }

    /// Jitters `delay` by 0.5x-1.5x so clients disconnected by the same
    /// outage don't retry in lockstep.
    fn jittered(delay: Duration) -> Duration {
        let multiplier: f64 = rand::random_range(0.5..1.5);
        delay.mul_f64(multiplier)
    }

    /// Only resets once the connection held open longer than
    /// `STABILITY_WINDOW`, not just on connect success — otherwise a venue
    /// that connects then drops instantly never lets the backoff engage.
    fn reset_if_stable(&mut self, connected_at: Instant, now: Instant) {
        if now.duration_since(connected_at) > STABILITY_WINDOW {
            self.attempt = 0;
        }
    }
}

/// A per-venue ceiling on connection attempts: `capacity` tokens, refilling
/// at `refill_per_sec`, never exceeding `capacity`. Backoff decides *when*
/// to retry; this decides whether a retry is allowed at all — an absolute
/// cap matching Binance's documented 300 attempts / 5 minutes limit.
struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: f64, refill_per_sec: f64) -> Self {
        TokenBucket {
            capacity,
            tokens: capacity,
            refill_per_sec,
            last_refill: Instant::now(),
        }
    }

    /// Adds tokens for elapsed time, capped at `capacity`.
    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
    }

    /// Waits until a token is available, then spends it.
    async fn acquire(&mut self) {
        loop {
            self.refill(Instant::now());
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                return;
            }
            let deficit = 1.0 - self.tokens;
            let wait = Duration::from_secs_f64(deficit / self.refill_per_sec);
            tokio::time::sleep(wait).await;
        }
    }
}

/// Connects to `exchange` for `pair`, reads frames, sends each parsed
/// `Book` down `tx`. Never returns except on panic: reconnects forever
/// with backoff and a per-venue rate limit.
pub async fn run_feed<E: Exchange>(
    exchange: E,
    pair: String,
    tx: mpsc::Sender<(Venue, Book)>,
) -> Result<()> {
    let mut backoff = Backoff::new();
    let (capacity, refill_per_sec) = exchange.venue().connect_rate();
    let mut bucket = TokenBucket::new(capacity, refill_per_sec);

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

        // None means the connect attempt itself failed — nothing to judge.
        if let Some(connected_at) = connected_at {
            backoff.reset_if_stable(connected_at, Instant::now());
        }

        let wait = Backoff::jittered(backoff.next_delay());
        info!(venue = %exchange.venue(), wait_secs = wait.as_secs_f64(), "reconnecting after backoff");
        tokio::time::sleep(wait).await;

        bucket.acquire().await;
    }
}

/// One connect-subscribe-read cycle. `connected_at` is `Some` once the
/// handshake completed, `None` if the connect attempt itself failed.
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

/// The connect-subscribe-read body.
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

    // Subscribe inside the reconnect loop — Bitstamp's subscription is
    // per-connection and must be resent every reconnect. Binance returns
    // None here since its subscription is baked into the URL.
    if let Some(msg) = exchange.subscribe_message(pair) {
        ws.send(Message::Text(msg.into()))
            .await
            .map_err(|e| (Some(connected_at), anyhow::Error::from(e)))?;
    }

    // Single next() loop, no .split() — no need for a mutex on the socket.
    // Kraken needs an application-level ping if idle 60s; ours fires at 30s.
    let kraken_idle_ping = (exchange.venue() == Venue::Kraken).then_some(Duration::from_secs(30));

    loop {
        let message = match kraken_idle_ping {
            Some(idle) => match tokio::time::timeout(idle, ws.next()).await {
                Ok(next) => next,
                Err(_) => {
                    debug!(
                        venue = %exchange.venue(),
                        idle_secs = idle.as_secs(),
                        "idle past threshold, sending client-initiated ping"
                    );
                    ws.send(Message::Text(r#"{"method":"ping"}"#.into()))
                        .await
                        .map_err(|e| {
                            (
                                Some(connected_at),
                                anyhow::Error::from(e).context("failed to send kraken idle ping"),
                            )
                        })?;
                    continue;
                }
            },
            None => ws.next().await,
        };
        let Some(message) = message else {
            break;
        };
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
                        // debug, not info: Binance alone pushes ~10/s and
                        // would flood stdout at info level.
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
                    // Bounded send backpressures the feed if the aggregator
                    // falls behind. A failed send is fine — the aggregator
                    // ending already kills the process via JoinSet.
                    let _ = tx.send((exchange.venue(), book)).await;
                }
            }
            // tungstenite answers pings automatically.
            Message::Ping(_) | Message::Pong(_) => {}
            // Ends this connect-cycle; run_feed's outer loop reconnects.
            Message::Close(_) => {
                info!(venue = %exchange.venue(), "connection closed");
                break;
            }
            Message::Binary(_) => {
                debug!("ignoring unexpected binary message");
            }
            // Never actually produced by the client-API stream, but needed
            // for an exhaustive match.
            Message::Frame(_) => {
                debug!("ignoring raw frame message");
            }
        }
    }

    Ok(connected_at)
}

/// Parses `(host, port)` out of a `wss://host[:port]/path` URL for the
/// proxy CONNECT tunnel — the `Exchange` trait only exposes the full URL.
fn parse_connect_target(url: &str) -> (String, u16) {
    let without_scheme = url.strip_prefix("wss://").unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(443)),
        None => (authority.to_string(), 443),
    }
}

/// Reads `HTTPS_PROXY`/`HTTP_PROXY` into a `(host, port)` pair. Returns
/// `None` if unset, empty, or unparseable — a bad proxy value logs a
/// warning and falls back to connecting directly rather than crashing.
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

    /// The sequence must be exactly 1s, 2s, 4s, 8s, 16s, then 30s forever.
    #[test]
    fn backoff_grows_and_caps() {
        let mut backoff = Backoff::new();
        let expected_secs = [1, 2, 4, 8, 16, 30, 30, 30, 30];
        for &expected in &expected_secs {
            assert_eq!(backoff.next_delay(), Duration::from_secs(expected));
        }
        for _ in 0..1_000 {
            assert!(backoff.next_delay() <= Duration::from_secs(30));
        }
    }

    /// A connect-then-instant-drop cycle must not reset the backoff, or it
    /// would drive one connection attempt per second into a rate limit.
    #[test]
    fn a_short_lived_connection_does_not_reset_the_thrash_loop() {
        let mut backoff = Backoff::new();
        let mut observed = Vec::new();

        for _ in 0..5 {
            observed.push(backoff.next_delay());

            let now = Instant::now();
            let connected_at = now;
            let judged_at = connected_at + Duration::from_millis(50);
            backoff.reset_if_stable(connected_at, judged_at);
        }

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

    /// A connection held past `STABILITY_WINDOW` must reset the backoff.
    #[test]
    fn a_stable_connection_does_reset() {
        let mut backoff = Backoff::new();

        backoff.next_delay();
        backoff.next_delay();
        backoff.next_delay();

        let connected_at = Instant::now();
        let judged_at = connected_at + STABILITY_WINDOW + Duration::from_secs(1);
        backoff.reset_if_stable(connected_at, judged_at);

        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    /// Jitter must land within 0.5x-1.5x, and actually vary rather than
    /// being a no-op that always returns the nominal value.
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
        assert!(
            saw_below_nominal,
            "never saw a jittered delay below nominal"
        );
        assert!(
            saw_above_nominal,
            "never saw a jittered delay above nominal"
        );
    }

    /// Tokens refill proportional to elapsed time at the configured rate.
    #[test]
    fn bucket_empties_and_refills() {
        let mut bucket = TokenBucket::new(5.0, 1.0); // capacity 5, 1 token/sec
        let t0 = Instant::now();
        bucket.last_refill = t0;
        bucket.tokens = 0.0;

        // 2.5 seconds at 1 token/sec should refill 2.5 tokens.
        bucket.refill(t0 + Duration::from_millis(2_500));
        assert!(
            (bucket.tokens - 2.5).abs() < 1e-9,
            "expected ~2.5 tokens, got {}",
            bucket.tokens
        );
    }

    /// A long idle period must not let tokens accumulate past capacity.
    #[test]
    fn bucket_does_not_exceed_capacity() {
        let mut bucket = TokenBucket::new(5.0, 1.0); // capacity 5, 1 token/sec
        let t0 = Instant::now();
        bucket.last_refill = t0;
        bucket.tokens = 0.0;

        bucket.refill(t0 + Duration::from_secs(3_600));
        assert_eq!(bucket.tokens, 5.0);
    }
}
